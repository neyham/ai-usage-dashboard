//! Grok Build usage fetcher. Credentials are read from the official CLI's
//! `auth.json` and are never refreshed, rewritten, cached, or sent to the
//! renderer. The billing endpoints are not public APIs, so any incompatible
//! response degrades to last-known-good sanitized data.

use super::{send, send_with_one_retry, FetchError, Resp};
use crate::config::Config;
use crate::models::GrokService;
use crate::util::{clamp_percent, local_label, parse_datetime_str};
use anyhow::{anyhow, bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use std::cmp::Ordering;
use std::future::Future;
use std::io::Read;
use std::path::Path;
use std::time::Duration as StdDuration;

#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
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
const TEAM_UNAVAILABLE: &str = "TEAM USAGE UNAVAILABLE";
const AUTH_READ_TIMEOUT: StdDuration = StdDuration::from_secs(16);
const SETTINGS_TIMEOUT: StdDuration = StdDuration::from_secs(2);
const MAX_AUTH_BYTES: usize = 64 * 1024;
const MAX_JWT_PAYLOAD_BYTES: usize = 16 * 1024;
#[cfg(windows)]
const WSL_PROCESS_TIMEOUT: StdDuration = StdDuration::from_secs(15);

struct GrokAuth {
    token: String,
    principal_is_team: bool,
    plan: Option<String>,
}

struct AuthCandidate {
    token: String,
    principal_is_team: bool,
    scope_rank: u8,
    created_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
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
    let auth = read_auth(config).await.map_err(|_| FetchError::Auth {
        message: LOGIN_REQUIRED,
    })?;

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

    service_from_responses(
        auth.principal_is_team,
        auth.plan.as_deref(),
        &credits_result,
        &monthly_result,
        &settings_result,
    )
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
        if response_is_auth_failure(credits_result) || response_is_auth_failure(monthly_result) {
            return Err(FetchError::Auth {
                message: if principal_is_team {
                    TEAM_UNAVAILABLE
                } else {
                    LOGIN_REQUIRED
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

fn response_is_auth_failure(result: &anyhow::Result<Resp>) -> bool {
    matches!(result, Ok(resp) if resp.status == 401 || resp.status == 403)
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

async fn read_auth(config: &Config) -> anyhow::Result<GrokAuth> {
    let configured = config.grok_credentials_path.trim().to_string();
    let text = tokio::time::timeout(
        AUTH_READ_TIMEOUT,
        tokio::task::spawn_blocking(move || read_auth_text(&configured)),
    )
    .await
    .context("Grok credential read timed out")?
    .context("join Grok credential reader")??;
    parse_auth_at(&text, Utc::now())
}

fn read_auth_text(configured: &str) -> anyhow::Result<String> {
    if !configured.is_empty() {
        if let Some(spec) = parse_wsl_spec(configured)? {
            #[cfg(windows)]
            {
                return read_wsl_path(&spec.distro, &spec.path);
            }
            #[cfg(not(windows))]
            {
                let _ = spec;
                bail!("Grok wsl: credential paths require Windows");
            }
        }
        return read_file_limited(Path::new(configured));
    }

    let path = dirs::home_dir()
        .ok_or_else(|| anyhow!("no home directory"))?
        .join(".grok")
        .join("auth.json");
    #[cfg(windows)]
    {
        match std::fs::metadata(&path) {
            Ok(_) => read_file_limited(&path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => read_default_wsl_auth(),
            Err(err) => {
                Err(err).with_context(|| format!("inspect Grok auth at {}", path.display()))
            }
        }
    }
    #[cfg(not(windows))]
    {
        read_file_limited(&path)
    }
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

#[derive(Debug, PartialEq, Eq)]
struct WslSpec {
    distro: String,
    path: String,
}

fn parse_wsl_spec(value: &str) -> anyhow::Result<Option<WslSpec>> {
    let Some(spec) = value.strip_prefix("wsl:") else {
        return Ok(None);
    };
    let (raw_distro, raw_path) = spec
        .split_once(':')
        .context("Grok WSL path must be wsl:<distro>:<absolute-path>")?;
    if raw_distro.chars().any(char::is_control) || raw_path.chars().any(char::is_control) {
        bail!("Grok WSL credential spec contains control characters");
    }
    let distro = raw_distro.trim();
    let path = raw_path.trim();
    if distro.is_empty() {
        bail!("Grok WSL distribution is invalid");
    }
    if !path.starts_with('/') {
        bail!("Grok WSL credential path must be absolute");
    }
    Ok(Some(WslSpec {
        distro: distro.to_string(),
        path: path.to_string(),
    }))
}

#[cfg(windows)]
fn read_wsl_path(distro: &str, path: &str) -> anyhow::Result<String> {
    let mut command = Command::new("wsl.exe");
    command.args(["-d", distro, "--", "cat", "--", path]);
    read_wsl_command(command)
}

#[cfg(windows)]
fn read_default_wsl_auth() -> anyhow::Result<String> {
    let mut command = Command::new("wsl.exe");
    command.args([
        "-d",
        "Ubuntu",
        "--",
        "sh",
        "-lc",
        "cat -- \"$HOME/.grok/auth.json\"",
    ]);
    read_wsl_command(command)
}

#[cfg(windows)]
fn read_wsl_command(mut command: Command) -> anyhow::Result<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("start WSL Grok credential reader")?;
    let stdout = child
        .stdout
        .take()
        .context("capture WSL Grok credential output")?;
    let reader = std::thread::spawn(move || read_bounded_utf8(stdout));
    let deadline = Instant::now() + WSL_PROCESS_TIMEOUT;

    let status = loop {
        if let Some(status) = child.try_wait().context("poll WSL Grok reader")? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("WSL Grok credential reader timed out");
        }
        std::thread::sleep(StdDuration::from_millis(25));
    };

    let text = reader
        .join()
        .map_err(|_| anyhow!("join WSL Grok credential output reader"))??;
    if !status.success() {
        bail!("WSL Grok credential reader failed");
    }
    Ok(text)
}

fn parse_auth_at(text: &str, now: DateTime<Utc>) -> anyhow::Result<GrokAuth> {
    let root: Value = serde_json::from_str(text).context("parse Grok auth.json")?;
    let entries = root
        .as_object()
        .context("Grok auth.json must contain an account map")?;

    let selected = entries
        .iter()
        .filter_map(|(scope, value)| auth_candidate(scope, value, now))
        .max_by(compare_candidates)
        .context("Grok auth.json has no usable login")?;
    let plan = plan_from_access_token(&selected.token);

    Ok(GrokAuth {
        token: selected.token,
        principal_is_team: selected.principal_is_team,
        plan,
    })
}

fn auth_candidate(scope: &str, value: &Value, now: DateTime<Utc>) -> Option<AuthCandidate> {
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
    if expires_at.is_some_and(|expires| expires <= now) {
        return None;
    }
    let created_at = value
        .get("create_time")
        .and_then(Value::as_str)
        .and_then(parse_datetime_str);
    let principal_is_team = value
        .get("principal_type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("team"));

    Some(AuthCandidate {
        token,
        principal_is_team,
        scope_rank,
        created_at,
        expires_at,
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
        finish_optional_settings, grok_api_request, parse_auth_at, parse_credits, parse_monthly,
        parse_settings_plan, parse_wsl_spec, plan_from_access_token, read_bounded_utf8,
        service_from_responses, WslSpec, BILLING_URL, CREDITS_URL, GROK_CLIENT_SURFACE,
        LOGIN_REQUIRED, MAX_AUTH_BYTES, SETTINGS_URL,
    };
    use crate::config::Config;
    use crate::fetchers::{build_client, FetchError, Resp};
    use chrono::{TimeZone, Utc};
    use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
    use std::io::Cursor;
    use std::time::{Duration, Instant};

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
    fn wsl_spec_requires_distro_and_absolute_path() {
        assert_eq!(
            parse_wsl_spec("wsl:TestDistro:/home/test-user/.grok/auth.json").expect("valid spec"),
            Some(WslSpec {
                distro: "TestDistro".into(),
                path: "/home/test-user/.grok/auth.json".into(),
            })
        );
        assert!(parse_wsl_spec("wsl::/home/test-user/.grok/auth.json").is_err());
        assert!(parse_wsl_spec("wsl:Ubuntu:relative/auth.json").is_err());
        assert!(parse_wsl_spec("wsl:Ubuntu\n:/home/test-user/.grok/auth.json").is_err());
        assert_eq!(
            parse_wsl_spec("/home/user/.grok/auth.json").expect("normal path"),
            None
        );
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
