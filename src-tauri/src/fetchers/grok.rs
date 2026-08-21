//! Grok Build usage fetcher. Credentials are read from the official CLI's
//! `auth.json` and are never cached or sent to the renderer. When an access
//! token expires, the dashboard may ask the official CLI to renew its own
//! session; the dashboard only checks that a refresh token is non-empty and
//! never extracts it into app state, sends it, logs it, or passes it to the
//! child process. It does not write the credential file itself. The billing
//! endpoints are not public APIs, so any incompatible response degrades to
//! last-known-good sanitized data.

use super::{send, send_with_one_retry, FetchError, Resp};
use crate::cache::cache_dir;
use crate::config::Config;
use crate::fs_util::{atomic_write, OsFileLock};
use crate::models::GrokService;
use crate::util::{clamp_percent, local_label, parse_datetime_str};
use anyhow::{anyhow, bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration as StdDuration;
use std::time::Instant;

// Current Grok Build 0.2.111 returns Method not found for CodexBar's
// `x.ai/billing` ACP probe. These read-only JSON shapes were live-verified with
// the same official CLI OIDC session and avoid launching a large process every
// refresh.
const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const CREDITS_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const SETTINGS_URL: &str = "https://cli-chat-proxy.grok.com/v1/settings";
const GROK_CLIENT_SURFACE: &str = "grok-build";
const LOGIN_REQUIRED: &str = "GROK LOGIN REQUIRED";
const SESSION_EXPIRED: &str = "GROK SESSION EXPIRED";
const ACCESS_UNAVAILABLE: &str = "GROK ACCESS UNAVAILABLE";
const TEAM_UNAVAILABLE: &str = "TEAM USAGE UNAVAILABLE";
const AUTH_READ_TIMEOUT: StdDuration = StdDuration::from_secs(16);
const AUTH_REFRESH_TIMEOUT: StdDuration = StdDuration::from_secs(20);
const AUTH_REFRESH_BACKOFF: Duration = Duration::minutes(15);
const AUTH_REFRESH_STATE_FILE: &str = "grok-auth-refresh-attempt";
const AUTH_REFRESH_LOCK_FILE: &str = "grok-auth-refresh.lock";
const AUTH_REFRESH_LOCK_WAIT: StdDuration = StdDuration::from_secs(30);
const SETTINGS_TIMEOUT: StdDuration = StdDuration::from_secs(2);
const MAX_AUTH_BYTES: usize = 64 * 1024;
const MAX_JWT_PAYLOAD_BYTES: usize = 16 * 1024;

struct GrokAuth {
    token: String,
    principal_is_team: bool,
    plan: Option<String>,
    can_refresh: bool,
}

enum AuthState {
    Active(GrokAuth),
    Expired { refreshable: bool },
}

struct AuthRenewal {
    state: AuthState,
    cli_attempted: bool,
}

struct AuthCandidate {
    token: String,
    principal_is_team: bool,
    scope_rank: u8,
    created_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    has_refresh_token: bool,
}

struct PeriodUsage {
    percent: f64,
    label: &'static str,
    caption: &'static str,
    reset_local: Option<String>,
}

struct MonthlyUsage {
    percent: f64,
    reset_local: Option<String>,
}

pub async fn fetch(config: &Config, client: &Client) -> Result<GrokService, FetchError> {
    let mut auth = match read_auth(config).await {
        Ok(AuthState::Active(auth)) => auth,
        Ok(AuthState::Expired { .. }) => {
            return Err(FetchError::Auth {
                message: SESSION_EXPIRED,
            });
        }
        Err(_) => {
            return Err(FetchError::Auth {
                message: LOGIN_REQUIRED,
            });
        }
    };

    let (first_result, unauthorized) = fetch_for_auth(&auth, client)
        .await
        .map_err(FetchError::Other)?;
    if unauthorized && auth.can_refresh {
        if let Some(renewal) = renew_auth(config, cache_dir(), true).await {
            if let AuthState::Active(refreshed) = renewal.state {
                let token_changed = refreshed.token != auth.token;
                if renewal.cli_attempted || token_changed {
                    auth = refreshed;
                    return fetch_for_auth(&auth, client)
                        .await
                        .map_err(FetchError::Other)?
                        .0;
                }
            }
        }
    }
    first_result
}

async fn fetch_for_auth(
    auth: &GrokAuth,
    client: &Client,
) -> anyhow::Result<(Result<GrokService, FetchError>, bool)> {
    let credits_task = tokio::spawn(request_grok_api(
        client.clone(),
        auth.token.clone(),
        CREDITS_URL,
    ));
    let monthly_task = tokio::spawn(request_grok_api(
        client.clone(),
        auth.token.clone(),
        BILLING_URL,
    ));
    let settings_task = tokio::spawn(request_optional_settings(
        client.clone(),
        auth.token.clone(),
    ));
    let credits_result = credits_task
        .await
        .map_err(|err| anyhow!("join Grok credits request: {err}"))?;
    let monthly_result = monthly_task
        .await
        .map_err(|err| anyhow!("join Grok monthly request: {err}"))?;
    let settings_result = settings_task
        .await
        .map_err(|err| anyhow!("join Grok settings request: {err}"))?;

    let unauthorized =
        response_has_status(&credits_result, 401) || response_has_status(&monthly_result, 401);
    Ok((
        service_from_responses(
            auth.principal_is_team,
            auth.plan.as_deref(),
            &credits_result,
            &monthly_result,
            &settings_result,
        ),
        unauthorized,
    ))
}

fn service_from_responses(
    principal_is_team: bool,
    fallback_plan: Option<&str>,
    credits_result: &anyhow::Result<Resp>,
    monthly_result: &anyhow::Result<Resp>,
    settings_result: &anyhow::Result<Resp>,
) -> Result<GrokService, FetchError> {
    let period = successful_body(credits_result)
        .map(parse_credits)
        .transpose()
        .ok()
        .flatten();
    let monthly = successful_body(monthly_result)
        .map(parse_monthly)
        .transpose()
        .ok()
        .flatten()
        .flatten();
    let plan = successful_body(settings_result)
        .and_then(|body| parse_settings_plan(body).ok().flatten())
        .or_else(|| fallback_plan.map(str::to_string));

    if period.is_none() && monthly.is_none() {
        if response_has_status(credits_result, 401) || response_has_status(monthly_result, 401) {
            return Err(FetchError::Auth {
                message: if principal_is_team {
                    TEAM_UNAVAILABLE
                } else {
                    LOGIN_REQUIRED
                },
            });
        }
        if response_has_status(credits_result, 403) || response_has_status(monthly_result, 403) {
            return Err(FetchError::Auth {
                message: if principal_is_team {
                    TEAM_UNAVAILABLE
                } else {
                    ACCESS_UNAVAILABLE
                },
            });
        }
        if let Some(retry_after) =
            response_retry_after(credits_result).or_else(|| response_retry_after(monthly_result))
        {
            return Err(FetchError::RateLimited { retry_after });
        }
        return if principal_is_team {
            Err(FetchError::Auth {
                message: TEAM_UNAVAILABLE,
            })
        } else {
            Err(anyhow!("Grok billing returned no usable usage windows").into())
        };
    }

    Ok(service_from_usage(period, monthly, plan))
}

async fn request_grok_api(
    client: Client,
    token: String,
    url: &'static str,
) -> anyhow::Result<Resp> {
    send_with_one_retry(|| grok_api_request(&client, &token, url)).await
}

async fn request_optional_settings(client: Client, token: String) -> anyhow::Result<Resp> {
    finish_optional_settings(
        send(grok_api_request(&client, &token, SETTINGS_URL)),
        SETTINGS_TIMEOUT,
    )
    .await
}

async fn finish_optional_settings<F>(request: F, timeout: StdDuration) -> anyhow::Result<Resp>
where
    F: Future<Output = anyhow::Result<Resp>>,
{
    tokio::time::timeout(timeout, request)
        .await
        .context("Grok settings request timed out")?
}

fn grok_api_request(client: &Client, token: &str, url: &str) -> RequestBuilder {
    // The live endpoint accepts the installed client's surface without a
    // version header. Avoid pinning a fake Grok version that can drift.
    client
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            concat!("AI-Usage-Dashboard/", env!("CARGO_PKG_VERSION")),
        )
        .header("x-grok-client-surface", GROK_CLIENT_SURFACE)
}

pub(crate) fn parse_usage(credits_body: &str, monthly_body: &str) -> anyhow::Result<GrokService> {
    let period = Some(parse_credits(credits_body)?);
    let monthly = parse_monthly(monthly_body)?;
    Ok(service_from_usage(period, monthly, None))
}

fn service_from_usage(
    period: Option<PeriodUsage>,
    monthly: Option<MonthlyUsage>,
    plan: Option<String>,
) -> GrokService {
    GrokService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        plan,
        usage_percent: period.as_ref().map(|usage| usage.percent),
        period_label: period.as_ref().map(|usage| usage.label.into()),
        period_caption: period.as_ref().map(|usage| usage.caption.into()),
        usage_reset_local: period.and_then(|usage| usage.reset_local),
        monthly_percent: monthly.as_ref().map(|usage| usage.percent),
        monthly_reset_local: monthly.and_then(|usage| usage.reset_local),
    }
}

fn response_has_status(result: &anyhow::Result<Resp>, status: u16) -> bool {
    matches!(result, Ok(resp) if resp.status == status)
}

fn response_retry_after(result: &anyhow::Result<Resp>) -> Option<Option<u64>> {
    match result {
        Ok(resp) if resp.status == 429 => Some(resp.retry_after),
        _ => None,
    }
}

fn successful_body(result: &anyhow::Result<Resp>) -> Option<&str> {
    match result {
        Ok(resp) if resp.is_success() => Some(resp.body.as_str()),
        _ => None,
    }
}

fn parse_credits(body: &str) -> anyhow::Result<PeriodUsage> {
    let root: Value = serde_json::from_str(body).context("parse Grok credits body")?;
    let config = root
        .get("config")
        .and_then(Value::as_object)
        .context("Grok credits missing config")?;

    let product_percent = config
        .get("productUsage")
        .and_then(Value::as_array)
        .and_then(|products| {
            products.iter().find_map(|product| {
                let is_build = product
                    .get("product")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.eq_ignore_ascii_case("GrokBuild"));
                is_build
                    .then(|| product.get("usagePercent").and_then(number_value))
                    .flatten()
            })
        });
    let percent = product_percent
        .or_else(|| config.get("creditUsagePercent").and_then(number_value))
        .context("Grok credits missing usage percentage")?;

    let period = config.get("currentPeriod").and_then(Value::as_object);
    let (label, caption) = period_display(
        period
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str),
    );
    let reset_local = period
        .and_then(|value| value.get("end"))
        .or_else(|| config.get("billingPeriodEnd"))
        .and_then(local_label);

    Ok(PeriodUsage {
        percent: clamp_percent(percent),
        label,
        caption,
        reset_local,
    })
}

fn parse_monthly(body: &str) -> anyhow::Result<Option<MonthlyUsage>> {
    let root: Value = serde_json::from_str(body).context("parse Grok billing body")?;
    let config = root
        .get("config")
        .and_then(Value::as_object)
        .context("Grok billing missing config")?;
    let limit = config.get("monthlyLimit").and_then(money_value);
    let used = config.get("used").and_then(money_value);
    let (Some(limit), Some(used)) = (limit, used) else {
        return Ok(None);
    };
    if !limit.is_finite() || limit <= 0.0 || !used.is_finite() {
        return Ok(None);
    }

    Ok(Some(MonthlyUsage {
        percent: clamp_percent((used / limit) * 100.0),
        reset_local: config.get("billingPeriodEnd").and_then(local_label),
    }))
}

fn parse_settings_plan(body: &str) -> anyhow::Result<Option<String>> {
    let root: Value = serde_json::from_str(body).context("parse Grok settings body")?;
    Ok(root
        .get("subscription_tier_display")
        .and_then(Value::as_str)
        .and_then(canonical_plan_label)
        .map(str::to_string))
}

fn canonical_plan_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => Some("Free"),
        "supergrok" => Some("SuperGrok"),
        "supergrok heavy" | "supergrokpro" | "supergrok_heavy" => Some("SuperGrok Heavy"),
        "supergrok lite" | "supergrok_lite" => Some("SuperGrok Lite"),
        "x basic" | "x_basic" => Some("X Basic"),
        "x premium" | "x_premium" => Some("X Premium"),
        "x premium+" | "x_premium_plus" => Some("X Premium+"),
        _ => None,
    }
}

fn plan_from_access_token(token: &str) -> Option<String> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty()
        || payload.is_empty()
        || signature.is_empty()
        || payload.len() > MAX_JWT_PAYLOAD_BYTES
    {
        return None;
    }

    // This claim is display-only. The authenticated billing response remains
    // the authority for whether the token can produce a service result.
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    if decoded.len() > MAX_JWT_PAYLOAD_BYTES {
        return None;
    }
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    let tier = claims.get("tier").and_then(Value::as_u64)?;
    plan_from_tier(tier).map(str::to_string)
}

fn plan_from_tier(tier: u64) -> Option<&'static str> {
    // Mirrors the allowlisted tier values in the official xAI Grok Build CLI.
    match tier {
        0 => Some("Free"),
        1 => Some("SuperGrok"),
        2 => Some("X Basic"),
        3 => Some("X Premium"),
        4 => Some("X Premium+"),
        5 => Some("SuperGrok Heavy"),
        6 => Some("SuperGrok Lite"),
        _ => None,
    }
}

fn period_display(period_type: Option<&str>) -> (&'static str, &'static str) {
    match period_type
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "USAGE_PERIOD_TYPE_WEEKLY" | "WEEKLY" => ("7D", "WEEKLY WINDOW"),
        "USAGE_PERIOD_TYPE_MONTHLY" | "MONTHLY" => ("MONTH", "MONTHLY WINDOW"),
        "USAGE_PERIOD_TYPE_DAILY" | "DAILY" => ("24H", "DAILY WINDOW"),
        "USAGE_PERIOD_TYPE_HOURLY" | "HOURLY" => ("1H", "HOURLY WINDOW"),
        _ => ("PERIOD", "CREDIT WINDOW"),
    }
}

fn money_value(value: &Value) -> Option<f64> {
    value
        .get("val")
        .unwrap_or(value)
        .as_f64()
        .or_else(|| value.get("val").and_then(Value::as_str)?.parse().ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn number_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|number| number.is_finite())
}

async fn read_auth(config: &Config) -> anyhow::Result<AuthState> {
    read_auth_with_refresh_dir(config, cache_dir()).await
}

async fn read_auth_with_refresh_dir(
    config: &Config,
    refresh_state_dir: PathBuf,
) -> anyhow::Result<AuthState> {
    let initial = read_auth_state(config.grok_credentials_path.trim()).await?;
    if !matches!(&initial, AuthState::Expired { refreshable: true }) {
        return Ok(initial);
    }

    Ok(renew_auth(config, refresh_state_dir, false)
        .await
        .map(|renewal| renewal.state)
        .unwrap_or(initial))
}

async fn read_auth_state(configured: &str) -> anyhow::Result<AuthState> {
    let configured = configured.to_string();
    let text = tokio::time::timeout(
        AUTH_READ_TIMEOUT,
        tokio::task::spawn_blocking(move || read_auth_text(&configured)),
    )
    .await
    .context("Grok credential read timed out")?
    .context("join Grok credential reader")??;
    parse_auth_state_at(&text, Utc::now())
}

async fn renew_auth(
    config: &Config,
    refresh_state_dir: PathBuf,
    force: bool,
) -> Option<AuthRenewal> {
    let configured = config.grok_credentials_path.trim().to_string();
    tokio::task::spawn_blocking(move || {
        renew_auth_with_official_cli(&configured, &refresh_state_dir, force)
    })
    .await
    .ok()
    .and_then(Result::ok)
}

fn renew_auth_with_official_cli(
    configured: &str,
    refresh_state_dir: &Path,
    force: bool,
) -> anyhow::Result<AuthRenewal> {
    std::fs::create_dir_all(refresh_state_dir).context("create Grok refresh state directory")?;
    let (lock_path, state_path) = auth_refresh_state_paths(refresh_state_dir, configured);
    let _lock = OsFileLock::acquire(&lock_path, AUTH_REFRESH_LOCK_WAIT)
        .context("acquire Grok refresh state lock")?;

    // Another dashboard process may have renewed the same source while this
    // process waited for the lock. Always trust the latest bounded reread.
    let current = parse_auth_state_at(&read_auth_text(configured)?, Utc::now())?;
    let can_attempt = match &current {
        AuthState::Active(auth) => force && auth.can_refresh,
        AuthState::Expired { refreshable } => *refreshable,
    };
    if !can_attempt || !auth_refresh_allowed_at(&state_path, Utc::now()) {
        return Ok(AuthRenewal {
            state: current,
            cli_attempted: false,
        });
    }

    mark_auth_refresh_attempt(&state_path, Utc::now())?;
    let command = build_auth_refresh_command(configured)?;
    let _ = run_auth_refresh_command(command, AUTH_REFRESH_TIMEOUT);

    // The official CLI can update auth.json before a later models request
    // fails. Accept a fresh file regardless of the process exit status.
    let state = read_auth_text(configured)
        .and_then(|text| parse_auth_state_at(&text, Utc::now()))
        .unwrap_or(current);
    Ok(AuthRenewal {
        state,
        cli_attempted: true,
    })
}

fn auth_refresh_state_paths(dir: &Path, configured: &str) -> (PathBuf, PathBuf) {
    let mut hasher = DefaultHasher::new();
    configured.hash(&mut hasher);
    let key = hasher.finish();
    (
        dir.join(format!("{AUTH_REFRESH_LOCK_FILE}-{key:016x}")),
        dir.join(format!("{AUTH_REFRESH_STATE_FILE}-{key:016x}")),
    )
}

fn auth_refresh_allowed_at(state_path: &Path, now: DateTime<Utc>) -> bool {
    let last_attempt = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|text| DateTime::parse_from_rfc3339(text.trim()).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc));
    last_attempt.is_none_or(|timestamp| now - timestamp >= AUTH_REFRESH_BACKOFF)
}

fn mark_auth_refresh_attempt(state_path: &Path, now: DateTime<Utc>) -> anyhow::Result<()> {
    atomic_write(state_path, now.to_rfc3339().as_bytes()).context("persist Grok refresh attempt")
}

#[cfg(test)]
fn reserve_auth_refresh_at(
    dir: &Path,
    configured: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<bool> {
    std::fs::create_dir_all(dir).context("create Grok refresh state directory")?;
    let (lock_path, state_path) = auth_refresh_state_paths(dir, configured);
    let _lock = OsFileLock::acquire(&lock_path, AUTH_REFRESH_LOCK_WAIT)
        .context("acquire Grok refresh state lock")?;
    if !auth_refresh_allowed_at(&state_path, now) {
        return Ok(false);
    }

    mark_auth_refresh_attempt(&state_path, now)?;
    Ok(true)
}

fn build_auth_refresh_command(configured: &str) -> anyhow::Result<Command> {
    if !configured.is_empty() {
        return build_native_auth_refresh_command(Path::new(configured));
    }

    build_native_auth_refresh_command(&default_auth_path()?)
}

fn default_auth_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("no home directory"))?
        .join(".grok")
        .join("auth.json"))
}

fn grok_home_from_auth_path(path: &Path) -> anyhow::Result<&Path> {
    let grok_dir = path
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == ".grok"))
        .context("Grok auth path is not the official .grok/auth.json location")?;
    if path.file_name().is_none_or(|name| name != "auth.json") {
        bail!("Grok auth path is not the official .grok/auth.json location");
    }
    grok_dir
        .parent()
        .context("Grok auth path has no home directory")
}

fn build_native_auth_refresh_command(auth_path: &Path) -> anyhow::Result<Command> {
    let home = grok_home_from_auth_path(auth_path)?;
    let cli_name = if cfg!(windows) { "grok.exe" } else { "grok" };
    let cli = home.join(".grok").join("bin").join(cli_name);
    if !cli.is_file() {
        bail!("official Grok CLI is unavailable");
    }

    let mut command = Command::new(cli);
    command.env("HOME", home);
    #[cfg(windows)]
    command.env("USERPROFILE", home);
    add_auth_refresh_args(&mut command);
    Ok(command)
}

fn add_auth_refresh_args(command: &mut Command) {
    command.args(["--no-auto-update", "models"]);
}

fn run_auth_refresh_command(mut command: Command, timeout: StdDuration) -> anyhow::Result<()> {
    hide_command_window(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start official Grok credential renewal")?;
    let deadline = Instant::now() + timeout;

    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err).context("poll official Grok credential renewal");
            }
        };
        if let Some(status) = status {
            if status.success() {
                return Ok(());
            }
            bail!("official Grok credential renewal failed");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("official Grok credential renewal timed out");
        }
        std::thread::sleep(StdDuration::from_millis(50));
    }
}

#[cfg(windows)]
fn hide_command_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_command_window(_command: &mut Command) {}

fn read_auth_text(configured: &str) -> anyhow::Result<String> {
    if !configured.is_empty() {
        return read_file_limited(Path::new(configured));
    }

    read_file_limited(&default_auth_path()?)
}

fn read_file_limited(path: &Path) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("read Grok auth at {}", path.display()))?;
    read_bounded_utf8(file)
}

fn read_bounded_utf8(reader: impl Read) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_AUTH_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read Grok credential data")?;
    if bytes.len() > MAX_AUTH_BYTES {
        bail!("Grok auth.json exceeds {MAX_AUTH_BYTES} bytes");
    }
    String::from_utf8(bytes).context("Grok auth.json is not UTF-8")
}

#[cfg(test)]
fn parse_auth_at(text: &str, now: DateTime<Utc>) -> anyhow::Result<GrokAuth> {
    match parse_auth_state_at(text, now)? {
        AuthState::Active(auth) => Ok(auth),
        AuthState::Expired { .. } => bail!("Grok session expired"),
    }
}

fn parse_auth_state_at(text: &str, now: DateTime<Utc>) -> anyhow::Result<AuthState> {
    let root: Value = serde_json::from_str(text).context("parse Grok auth.json")?;
    let entries = root
        .as_object()
        .context("Grok auth.json must contain an account map")?;

    let mut candidates = entries
        .iter()
        .filter_map(|(scope, value)| auth_candidate(scope, value))
        .collect::<Vec<_>>();
    let active_index = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.expires_at.is_none_or(|expires| expires > now))
        .max_by(|left, right| compare_candidates(left.1, right.1))
        .map(|(index, _)| index);
    let Some(active_index) = active_index else {
        let has_expired = candidates
            .iter()
            .any(|candidate| candidate.expires_at.is_some_and(|expires| expires <= now));
        if has_expired {
            return Ok(AuthState::Expired {
                refreshable: candidates.iter().any(|candidate| {
                    candidate.has_refresh_token
                        && candidate.expires_at.is_some_and(|expires| expires <= now)
                }),
            });
        }
        bail!("Grok auth.json has no usable login");
    };
    let selected = candidates.swap_remove(active_index);
    let plan = plan_from_access_token(&selected.token);

    Ok(AuthState::Active(GrokAuth {
        token: selected.token,
        principal_is_team: selected.principal_is_team,
        plan,
        can_refresh: selected.has_refresh_token,
    }))
}

fn auth_candidate(scope: &str, value: &Value) -> Option<AuthCandidate> {
    let scope_rank = if scope.starts_with("https://auth.x.ai::") {
        2
    } else if scope == "https://accounts.x.ai/sign-in" || scope.contains("/sign-in") {
        1
    } else {
        return None;
    };
    let token = value
        .get("key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())?
        .to_string();
    let expires_at = value
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(parse_datetime_str);
    let created_at = value
        .get("create_time")
        .and_then(Value::as_str)
        .and_then(parse_datetime_str);
    let principal_is_team = value
        .get("principal_type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("team"));
    let has_refresh_token = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.trim().is_empty());

    Some(AuthCandidate {
        token,
        principal_is_team,
        scope_rank,
        created_at,
        expires_at,
        has_refresh_token,
    })
}

fn compare_candidates(left: &AuthCandidate, right: &AuthCandidate) -> Ordering {
    left.scope_rank
        .cmp(&right.scope_rank)
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.expires_at.cmp(&right.expires_at))
}

#[cfg(test)]
mod tests {
    use super::{
        finish_optional_settings, grok_api_request, grok_home_from_auth_path, parse_auth_at,
        parse_auth_state_at, parse_credits, parse_monthly, parse_settings_plan,
        plan_from_access_token, read_bounded_utf8, reserve_auth_refresh_at, service_from_responses,
        AuthState, ACCESS_UNAVAILABLE, BILLING_URL, CREDITS_URL, GROK_CLIENT_SURFACE,
        LOGIN_REQUIRED, MAX_AUTH_BYTES, SETTINGS_URL,
    };
    #[cfg(unix)]
    use super::{read_auth_with_refresh_dir, renew_auth_with_official_cli};
    use crate::config::Config;
    use crate::fetchers::{build_client, FetchError, Resp};
    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
    use std::io::Cursor;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn auth_prefers_newest_active_oidc_then_legacy() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 24, 0, 0, 0)
            .single()
            .expect("valid test time");
        let auth = parse_auth_at(
            r#"{
                "https://accounts.x.ai/sign-in": {"key":"legacy"},
                "https://auth.x.ai::old": {
                    "key":"old",
                    "create_time":"2026-07-20T00:00:00Z",
                    "expires_at":"2026-08-01T00:00:00Z"
                },
                "https://auth.x.ai::new": {
                    "key":"new",
                    "create_time":"2026-07-23T00:00:00Z",
                    "expires_at":"2026-08-01T00:00:00Z"
                }
            }"#,
            now,
        )
        .expect("usable Grok auth");

        assert_eq!(auth.token, "new");
        assert!(!auth.principal_is_team);
        assert_eq!(auth.plan, None);
    }

    #[test]
    fn selected_auth_carries_supergrok_heavy_tier() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 24, 0, 0, 0)
            .single()
            .expect("valid test time");
        let auth = parse_auth_at(
            r#"{
                "https://auth.x.ai::current": {
                    "key":"e30.eyJ0aWVyIjo1fQ.sig",
                    "expires_at":"2026-08-01T00:00:00Z"
                }
            }"#,
            now,
        )
        .expect("usable Grok auth");

        assert_eq!(auth.plan.as_deref(), Some("SuperGrok Heavy"));
    }

    #[test]
    fn auth_skips_keyless_and_expired_oidc_and_detects_team() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 24, 0, 0, 0)
            .single()
            .expect("valid test time");
        let auth = parse_auth_at(
            r#"{
                "https://auth.x.ai::keyless": {"refresh_token":"secret"},
                "https://auth.x.ai::expired": {
                    "key":"expired",
                    "expires_at":"2026-07-23T00:00:00Z"
                },
                "https://accounts.x.ai/sign-in": {
                    "key":"legacy-team",
                    "principal_type":" Team "
                }
            }"#,
            now,
        )
        .expect("legacy fallback");

        assert_eq!(auth.token, "legacy-team");
        assert!(auth.principal_is_team);
    }

    #[test]
    fn expired_auth_is_distinct_and_refreshable_without_exposing_its_token() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 25, 1, 0, 0)
            .single()
            .expect("valid test time");
        let state = parse_auth_state_at(
            r#"{
                "https://auth.x.ai::current": {
                    "key":"expired-access",
                    "refresh_token":"fake-refresh-value",
                    "expires_at":"2026-07-25T00:00:00Z"
                }
            }"#,
            now,
        )
        .expect("recognized expired Grok session");

        assert!(matches!(state, AuthState::Expired { refreshable: true }));
        assert!(parse_auth_at(
            r#"{
                "https://auth.x.ai::current": {
                    "key":"expired-access",
                    "refresh_token":"fake-refresh-value",
                    "expires_at":"2026-07-25T00:00:00Z"
                }
            }"#,
            now
        )
        .is_err());
    }

    #[test]
    fn expired_auth_without_refresh_token_is_not_refreshable() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 25, 1, 0, 0)
            .single()
            .expect("valid test time");
        let state = parse_auth_state_at(
            r#"{
                "https://auth.x.ai::current": {
                    "key":"expired-access",
                    "expires_at":"2026-07-25T00:00:00Z"
                }
            }"#,
            now,
        )
        .expect("recognized expired Grok session");

        assert!(matches!(state, AuthState::Expired { refreshable: false }));
    }

    #[test]
    fn auth_refresh_attempts_are_throttled_for_fifteen_minutes() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-grok-refresh-{}-{unique}",
            std::process::id()
        ));
        let now = Utc
            .with_ymd_and_hms(2026, 7, 25, 1, 0, 0)
            .single()
            .expect("valid test time");
        let source = "/home/test-user/.grok/auth.json";

        assert!(reserve_auth_refresh_at(&dir, source, now).expect("reserve first Grok refresh"));
        assert!(
            !reserve_auth_refresh_at(&dir, source, now + ChronoDuration::minutes(14))
                .expect("throttle early Grok refresh")
        );
        assert!(
            reserve_auth_refresh_at(&dir, source, now + ChronoDuration::minutes(15))
                .expect("reserve Grok refresh after backoff")
        );
        assert!(
            reserve_auth_refresh_at(&dir, "/home/other/.grok/auth.json", now)
                .expect("different Grok source has an independent backoff")
        );

        std::fs::remove_dir_all(dir).expect("remove Grok refresh throttle test directory");
    }

    #[cfg(unix)]
    #[test]
    fn expired_native_auth_is_renewed_by_the_official_cli_helper() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Barrier};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-grok-renew-{}-{unique}",
            std::process::id()
        ));
        let grok_dir = home.join(".grok");
        let bin_dir = grok_dir.join("bin");
        let auth_path = grok_dir.join("auth.json");
        let cli_path = bin_dir.join("grok");
        let refresh_state_dir = home.join("dashboard-state");
        std::fs::create_dir_all(&bin_dir).expect("create fake Grok home");
        std::fs::write(
            &auth_path,
            r#"{
                "https://auth.x.ai::current": {
                    "key":"expired-access",
                    "refresh_token":"fake-refresh-token",
                    "expires_at":"2020-01-01T00:00:00Z"
                }
            }"#,
        )
        .expect("write expired fake Grok auth");
        std::fs::write(
            &cli_path,
            r#"#!/bin/sh
test "$1" = "--no-auto-update" || exit 2
test "$2" = "models" || exit 3
printf 'run\n' >> "$HOME/cli-runs"
printf 'fake-cli-output-secret\n'
printf 'fake-cli-error-secret\n' >&2
sleep 0.2
cat > "$HOME/.grok/auth.json" <<'EOF'
{
  "https://auth.x.ai::current": {
    "key": "e30.eyJ0aWVyIjo1fQ.sig",
    "refresh_token": "fake-refreshed-token",
    "expires_at": "2099-01-01T00:00:00Z"
  }
}
EOF
exit 7
"#,
        )
        .expect("write fake official Grok helper");
        let mut permissions = std::fs::metadata(&cli_path)
            .expect("inspect fake official Grok helper")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&cli_path, permissions)
            .expect("make fake official Grok helper executable");
        let config = Config {
            grok_credentials_path: auth_path.to_string_lossy().into_owned(),
            ..Config::default()
        };
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let config = config.clone();
                let refresh_state_dir = refresh_state_dir.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("test runtime");
                    barrier.wait();
                    runtime
                        .block_on(read_auth_with_refresh_dir(&config, refresh_state_dir))
                        .expect("renew expired Grok auth")
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            let state = handle.join().expect("join concurrent Grok renewal");
            let AuthState::Active(auth) = state else {
                panic!("expected active Grok auth after renewal");
            };
            assert_eq!(auth.plan.as_deref(), Some("SuperGrok Heavy"));
        }
        let invocations = std::fs::read_to_string(home.join("cli-runs"))
            .expect("read fake Grok helper invocation count");
        assert_eq!(invocations.lines().count(), 1);
        for entry in
            std::fs::read_dir(&refresh_state_dir).expect("read Grok refresh state directory")
        {
            let contents = std::fs::read_to_string(entry.expect("refresh state entry").path())
                .unwrap_or_default();
            assert!(!contents.contains("fake-cli-output-secret"));
            assert!(!contents.contains("fake-cli-error-secret"));
        }

        std::fs::remove_dir_all(home).expect("remove fake Grok renewal home");
    }

    #[cfg(unix)]
    #[test]
    fn active_auth_can_force_one_cli_renewal_after_unauthorized_response() {
        use std::os::unix::fs::PermissionsExt;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-grok-force-renew-{}-{unique}",
            std::process::id()
        ));
        let grok_dir = home.join(".grok");
        let bin_dir = grok_dir.join("bin");
        let auth_path = grok_dir.join("auth.json");
        let cli_path = bin_dir.join("grok");
        let refresh_state_dir = home.join("dashboard-state");
        std::fs::create_dir_all(&bin_dir).expect("create fake Grok home");
        std::fs::write(
            &auth_path,
            r#"{
                "https://auth.x.ai::current": {
                    "key":"active-but-rejected",
                    "refresh_token":"fake-refresh-token",
                    "expires_at":"2099-01-01T00:00:00Z"
                }
            }"#,
        )
        .expect("write active fake Grok auth");
        std::fs::write(
            &cli_path,
            r#"#!/bin/sh
test "$1" = "--no-auto-update" || exit 2
test "$2" = "models" || exit 3
printf 'run\n' >> "$HOME/cli-runs"
cat > "$HOME/.grok/auth.json" <<'EOF'
{
  "https://auth.x.ai::current": {
    "key": "e30.eyJ0aWVyIjo1fQ.sig",
    "refresh_token": "fake-refreshed-token",
    "expires_at": "2099-02-01T00:00:00Z"
  }
}
EOF
"#,
        )
        .expect("write fake official Grok helper");
        let mut permissions = std::fs::metadata(&cli_path)
            .expect("inspect fake official Grok helper")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&cli_path, permissions)
            .expect("make fake official Grok helper executable");
        let configured = auth_path.to_string_lossy().into_owned();

        let first = renew_auth_with_official_cli(&configured, &refresh_state_dir, true)
            .expect("force Grok renewal after 401");
        assert!(first.cli_attempted);
        let AuthState::Active(first_auth) = first.state else {
            panic!("expected active Grok auth after forced renewal");
        };
        assert_eq!(first_auth.plan.as_deref(), Some("SuperGrok Heavy"));

        let second = renew_auth_with_official_cli(&configured, &refresh_state_dir, true)
            .expect("respect Grok renewal backoff");
        assert!(!second.cli_attempted);
        assert_eq!(
            std::fs::read_to_string(home.join("cli-runs"))
                .expect("read fake Grok helper invocation count")
                .lines()
                .count(),
            1
        );

        std::fs::remove_dir_all(home).expect("remove fake forced Grok renewal home");
    }

    #[cfg(unix)]
    #[test]
    fn unavailable_cli_still_consumes_the_refresh_backoff() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-grok-missing-cli-{}-{unique}",
            std::process::id()
        ));
        let grok_dir = home.join(".grok");
        let auth_path = grok_dir.join("auth.json");
        let refresh_state_dir = home.join("dashboard-state");
        std::fs::create_dir_all(&grok_dir).expect("create fake Grok home");
        std::fs::write(
            &auth_path,
            r#"{
                "https://auth.x.ai::current": {
                    "key":"expired-access",
                    "refresh_token":"fake-refresh-token",
                    "expires_at":"2020-01-01T00:00:00Z"
                }
            }"#,
        )
        .expect("write expired fake Grok auth");
        let configured = auth_path.to_string_lossy().into_owned();

        assert!(
            renew_auth_with_official_cli(&configured, &refresh_state_dir, false).is_err(),
            "the first renewal should report the missing official CLI"
        );
        let second = renew_auth_with_official_cli(&configured, &refresh_state_dir, false)
            .expect("the second renewal should be throttled before checking the CLI again");
        assert!(!second.cli_attempted);
        assert!(matches!(
            second.state,
            AuthState::Expired { refreshable: true }
        ));

        std::fs::remove_dir_all(home).expect("remove missing Grok CLI test home");
    }

    #[test]
    fn refresh_helpers_accept_only_official_auth_layouts() {
        assert_eq!(
            grok_home_from_auth_path(Path::new("/home/test-user/.grok/auth.json"))
                .expect("official native Grok path"),
            Path::new("/home/test-user")
        );
        assert!(grok_home_from_auth_path(Path::new("/home/test-user/private/auth.json")).is_err());
    }

    #[test]
    fn credits_prefer_grok_build_and_use_server_period_type() {
        let usage = parse_credits(
            r#"{
                "config": {
                    "currentPeriod": {
                        "type":"USAGE_PERIOD_TYPE_WEEKLY",
                        "end":"2030-04-14T12:00:00.123456+00:00"
                    },
                    "creditUsagePercent": 68,
                    "productUsage": [
                        {"product":"GrokChat","usagePercent":51},
                        {"product":"GrokBuild","usagePercent":37.5}
                    ]
                }
            }"#,
        )
        .expect("valid Grok credits");

        assert_eq!(usage.percent, 37.5);
        assert_eq!(usage.label, "7D");
        assert_eq!(usage.caption, "WEEKLY WINDOW");
        assert!(usage.reset_local.is_some());
    }

    #[test]
    fn credits_fall_back_to_shared_percent_and_clamp() {
        let usage = parse_credits(
            r#"{
                "config": {
                    "currentPeriod": {"type":"unexpected"},
                    "creditUsagePercent":"104.5"
                }
            }"#,
        )
        .expect("valid shared Grok credits");

        assert_eq!(usage.percent, 100.0);
        assert_eq!(usage.label, "PERIOD");
        assert_eq!(usage.caption, "CREDIT WINDOW");
    }

    #[test]
    fn monthly_allowance_is_optional_and_percent_scaled() {
        let usage = parse_monthly(
            r#"{
                "config": {
                    "monthlyLimit":{"val":"15000"},
                    "used":{"val":3750},
                    "billingPeriodEnd":"2026-08-01T00:00:00Z"
                }
            }"#,
        )
        .expect("valid Grok billing")
        .expect("monthly allowance");

        assert_eq!(usage.percent, 25.0);
        assert!(usage.reset_local.is_some());
        assert!(
            parse_monthly(r#"{"config":{"monthlyLimit":{"val":0},"used":{"val":0}}}"#)
                .expect("valid empty allowance")
                .is_none()
        );
    }

    #[test]
    fn malformed_or_missing_usage_fails_closed() {
        for body in [
            "{}",
            r#"{"config":{}}"#,
            r#"{"config":{"creditUsagePercent":"not-a-number"}}"#,
        ] {
            assert!(parse_credits(body).is_err(), "unexpectedly accepted {body}");
        }
    }

    #[test]
    fn settings_plan_is_allowlisted_and_normalized() {
        for (value, expected) in [
            ("Free", "Free"),
            ("SuperGrok", "SuperGrok"),
            ("SuperGrok Heavy", "SuperGrok Heavy"),
            ("SuperGrokPro", "SuperGrok Heavy"),
            ("SuperGrok Lite", "SuperGrok Lite"),
            ("X Basic", "X Basic"),
            ("X Premium", "X Premium"),
            ("X Premium+", "X Premium+"),
        ] {
            let body = format!(r#"{{"subscription_tier_display":"{value}"}}"#);
            assert_eq!(
                parse_settings_plan(&body).expect("valid Grok settings"),
                Some(expected.to_string())
            );
        }
        assert_eq!(
            parse_settings_plan(r#"{"subscription_tier_display":"Injected plan"}"#)
                .expect("valid unknown settings"),
            None
        );
        assert_eq!(
            parse_settings_plan("{}").expect("missing plan is optional"),
            None
        );
        assert!(parse_settings_plan("not json").is_err());
    }

    #[test]
    fn jwt_tier_fallback_maps_only_official_known_values() {
        for (payload, expected) in [
            ("eyJ0aWVyIjowfQ", "Free"),
            ("eyJ0aWVyIjoxfQ", "SuperGrok"),
            ("eyJ0aWVyIjoyfQ", "X Basic"),
            ("eyJ0aWVyIjozfQ", "X Premium"),
            ("eyJ0aWVyIjo0fQ", "X Premium+"),
            ("eyJ0aWVyIjo1fQ", "SuperGrok Heavy"),
            ("eyJ0aWVyIjo2fQ", "SuperGrok Lite"),
        ] {
            let token = format!("e30.{payload}.sig");
            assert_eq!(plan_from_access_token(&token).as_deref(), Some(expected));
        }
        for token in [
            "opaque-token",
            "e30.not-base64!.sig",
            "e30.W10.sig",
            "e30.eyJ0aWVyIjo5OX0.sig",
            "e30.eyJ0aWVyIjoiNSJ9.sig",
        ] {
            assert_eq!(plan_from_access_token(token), None);
        }
    }

    #[test]
    fn optional_monthly_and_settings_failures_do_not_discard_credits() {
        let credits = Ok(Resp {
            status: 200,
            body: r#"{
                "config": {
                    "currentPeriod": {"type":"WEEKLY"},
                    "productUsage": [{"product":"GrokBuild","usagePercent":12}]
                }
            }"#
            .into(),
            retry_after: None,
        });
        let monthly = Ok(Resp {
            status: 403,
            body: String::new(),
            retry_after: None,
        });
        let settings = Err(anyhow::anyhow!("settings unavailable"));

        let service = service_from_responses(
            false,
            Some("SuperGrok Heavy"),
            &credits,
            &monthly,
            &settings,
        )
        .expect("credits remain usable");
        assert_eq!(service.usage_percent, Some(12.0));
        assert_eq!(service.monthly_percent, None);
        assert_eq!(service.plan.as_deref(), Some("SuperGrok Heavy"));
    }

    #[test]
    fn optional_settings_timeout_is_short_and_does_not_block_usage() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        let started = Instant::now();
        let result = runtime.block_on(finish_optional_settings(
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(Resp {
                    status: 200,
                    body: "{}".into(),
                    retry_after: None,
                })
            },
            Duration::from_millis(10),
        ));

        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn settings_plan_overrides_the_token_fallback() {
        let credits = Ok(Resp {
            status: 200,
            body: r#"{
                "config": {
                    "currentPeriod": {"type":"WEEKLY"},
                    "productUsage": [{"product":"GrokBuild","usagePercent":12}]
                }
            }"#
            .into(),
            retry_after: None,
        });
        let monthly = Ok(Resp {
            status: 404,
            body: String::new(),
            retry_after: None,
        });
        let settings = Ok(Resp {
            status: 200,
            body: r#"{"subscription_tier_display":"SuperGrok Heavy"}"#.into(),
            retry_after: None,
        });

        let service =
            service_from_responses(false, Some("SuperGrok"), &credits, &monthly, &settings)
                .expect("credits remain usable");
        assert_eq!(service.plan.as_deref(), Some("SuperGrok Heavy"));
    }

    #[test]
    fn grok_build_product_name_never_becomes_the_plan() {
        let credits = Ok(Resp {
            status: 200,
            body: r#"{
                "config": {
                    "currentPeriod": {"type":"WEEKLY"},
                    "productUsage": [{"product":"GrokBuild","usagePercent":12}]
                }
            }"#
            .into(),
            retry_after: None,
        });
        let unavailable = Ok(Resp {
            status: 404,
            body: String::new(),
            retry_after: None,
        });

        let service = service_from_responses(false, None, &credits, &unavailable, &unavailable)
            .expect("credits remain usable");
        assert_eq!(service.plan, None);
    }

    #[test]
    fn auth_failure_is_reported_when_no_window_is_usable() {
        let unauthorized = Ok(Resp {
            status: 401,
            body: String::new(),
            retry_after: None,
        });
        let malformed = Ok(Resp {
            status: 200,
            body: "{}".into(),
            retry_after: None,
        });

        assert!(matches!(
            service_from_responses(false, None, &unauthorized, &malformed, &malformed),
            Err(FetchError::Auth {
                message: LOGIN_REQUIRED
            })
        ));
    }

    #[test]
    fn forbidden_access_is_not_misreported_as_a_missing_login() {
        let forbidden = Ok(Resp {
            status: 403,
            body: String::new(),
            retry_after: None,
        });
        let malformed = Ok(Resp {
            status: 200,
            body: "{}".into(),
            retry_after: None,
        });

        assert!(matches!(
            service_from_responses(false, None, &forbidden, &malformed, &malformed),
            Err(FetchError::Auth {
                message: ACCESS_UNAVAILABLE
            })
        ));
    }

    #[test]
    fn grok_api_request_uses_expected_endpoints_and_sanitized_headers() {
        let client = build_client(15).expect("HTTP client");
        for url in [CREDITS_URL, BILLING_URL, SETTINGS_URL] {
            let request = grok_api_request(&client, "fake-test-token", url)
                .build()
                .expect("Grok request");
            assert_eq!(request.method(), reqwest::Method::GET);
            assert_eq!(request.url().as_str(), url);
            let authorization = request
                .headers()
                .get(AUTHORIZATION)
                .expect("authorization header");
            assert_eq!(authorization.to_str().ok(), Some("Bearer fake-test-token"));
            assert!(authorization.is_sensitive());
            assert_eq!(
                request.headers().get(ACCEPT).and_then(|v| v.to_str().ok()),
                Some("application/json")
            );
            assert_eq!(
                request
                    .headers()
                    .get(USER_AGENT)
                    .and_then(|v| v.to_str().ok()),
                Some(concat!("AI-Usage-Dashboard/", env!("CARGO_PKG_VERSION")))
            );
            assert_eq!(
                request
                    .headers()
                    .get("x-grok-client-surface")
                    .and_then(|v| v.to_str().ok()),
                Some(GROK_CLIENT_SURFACE)
            );
            assert!(request.headers().get("x-grok-client-version").is_none());
        }
    }

    #[test]
    fn credential_reader_caps_size_and_requires_utf8() {
        assert_eq!(
            read_bounded_utf8(Cursor::new(vec![b'a'; MAX_AUTH_BYTES]))
                .expect("maximum credential size")
                .len(),
            MAX_AUTH_BYTES
        );
        assert!(read_bounded_utf8(Cursor::new(vec![b'a'; MAX_AUTH_BYTES + 1])).is_err());
        assert!(read_bounded_utf8(Cursor::new(vec![0xff, 0xfe])).is_err());
    }

    #[test]
    #[ignore = "requires GROK_LIVE_AUTH_PATH and a live Grok Build account"]
    fn live_fetch_uses_only_sanitized_output() {
        let path = std::env::var("GROK_LIVE_AUTH_PATH")
            .expect("set GROK_LIVE_AUTH_PATH to an auth.json path");
        let config = Config {
            grok_credentials_path: path,
            ..Config::default()
        };
        let client = build_client(15).expect("HTTP client");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let service = runtime
            .block_on(super::fetch(&config, &client))
            .expect("live Grok usage");

        assert_eq!(service.status, "NOMINAL");
        assert!(service.usage_percent.is_some() || service.monthly_percent.is_some());
        assert_ne!(service.plan.as_deref(), Some("Build"));
        assert!(!service.from_cache);
        assert!(!service.data_may_be_stale);
    }
}
