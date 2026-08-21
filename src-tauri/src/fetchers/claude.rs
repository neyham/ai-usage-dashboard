//! Claude usage fetcher. Reads OAuth credentials from the local
//! `.claude/.credentials.json`. Native credential files are read-only; expired
//! sessions can optionally be recovered through the official Claude Code CLI.
//! Usage 429s are signaled back to the orchestrator so it can enter cooldown.

use super::{send_with_one_retry, FetchError};
use crate::cache::cache_dir;
use crate::config::{
    Config, DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD, MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
    MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS, MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
    MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
};
use crate::fs_util::{atomic_write, OsFileLock};
use crate::models::ClaudeService;
use crate::util::{clamp_percent, local_label, parse_datetime};
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration as StdDuration, Instant};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
const MSG_LOGIN_REQUIRED: &str = "LOGIN REQUIRED";
const MSG_AUTH_EXPIRED: &str = "AUTH EXPIRED";
const MSG_REFRESH_BLOCKED: &str = "REFRESH BLOCKED";
const MSG_AUTH_CHECK_FAILED: &str = "AUTH CHECK FAILED";
const CLAUDE_CODE_RECOVERY_THROTTLE: Duration = Duration::minutes(30);
const CLAUDE_CODE_RECOVERY_LOCK_FILE: &str = "claude-code-recovery.lock";
const CLAUDE_CODE_RECOVERY_STATE_FILE: &str = "claude-code-recovery-attempt.txt";
const RECOVERY_LOCK_WAIT: StdDuration = StdDuration::from_secs(5);

#[derive(Debug)]
enum RefreshError {
    MissingRefreshToken,
    DirectRefreshUnavailable,
    #[allow(dead_code)]
    Other(anyhow::Error),
}

pub async fn fetch(config: &Config, client: &Client) -> Result<ClaudeService, FetchError> {
    let load_config = config.clone();
    let mut creds = tokio::task::spawn_blocking(move || load(&load_config))
        .await
        .map_err(|err| FetchError::Other(anyhow!("Claude credential task failed: {err}")))?
        .map_err(load_error)?;

    if creds.is_expired_soon() {
        refresh_or_recover(config, client, &mut creds).await?;
    }

    let mut resp = send_with_one_retry(|| usage_request(client, &creds.access_token))
        .await
        .map_err(FetchError::Other)?;

    if resp.status == 401 {
        refresh_or_recover(config, client, &mut creds).await?;
        resp = send_with_one_retry(|| usage_request(client, &creds.access_token))
            .await
            .map_err(FetchError::Other)?;
    }

    if resp.status == 429 {
        return Err(FetchError::RateLimited {
            retry_after: resp.retry_after,
        });
    }
    if !resp.is_success() {
        if resp.status == 401 {
            return Err(FetchError::Auth {
                message: MSG_AUTH_EXPIRED,
            });
        }
        return Err(FetchError::Other(anyhow!(
            "Claude usage HTTP {}",
            resp.status
        )));
    }

    parse_usage(&resp.body).map_err(FetchError::Other)
}

async fn refresh_or_recover(
    config: &Config,
    client: &Client,
    creds: &mut Creds,
) -> Result<(), FetchError> {
    match creds.refresh(client).await {
        Ok(()) => Ok(()),
        Err(err) => recover_allowed_error(config, creds, err).await,
    }
}

async fn recover_allowed_error(
    config: &Config,
    creds: &mut Creds,
    err: RefreshError,
) -> Result<(), FetchError> {
    let (allow_claude_code, message) = recovery_policy(&err)?;

    if allow_claude_code
        && config.claude_code_refresh_enabled
        && try_claude_code_refresh(config, creds).await.is_ok()
    {
        return Ok(());
    }
    Err(FetchError::Auth { message })
}

fn recovery_policy(err: &RefreshError) -> Result<(bool, &'static str), FetchError> {
    match err {
        RefreshError::MissingRefreshToken => Ok((true, MSG_AUTH_EXPIRED)),
        RefreshError::DirectRefreshUnavailable => Ok((true, MSG_REFRESH_BLOCKED)),
        RefreshError::Other(_) => Ok((false, MSG_AUTH_CHECK_FAILED)),
    }
}

fn load_error(err: anyhow::Error) -> FetchError {
    let text = err.to_string();
    if text.contains("credentials not found") || text.contains("OAuth token missing") {
        FetchError::Auth {
            message: MSG_LOGIN_REQUIRED,
        }
    } else {
        FetchError::Other(err)
    }
}

fn usage_request(client: &Client, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", ANTHROPIC_BETA)
}

pub(crate) fn parse_usage(body: &str) -> anyhow::Result<ClaudeService> {
    let root: Value = serde_json::from_str(body).context("parse Claude usage body")?;
    let five = usage_window(&root, "five_hour");
    let seven = weekly_usage_window(&root);
    let extra_usage_percent = extra_usage_percent(&root);

    if five.is_none() && seven.is_none() && extra_usage_percent.is_none() {
        bail!("Claude usage has no usable windows");
    }

    Ok(ClaudeService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        cooldown_until_local: None,
        five_hour_percent: five.as_ref().map(|window| window.0),
        seven_day_percent: seven.as_ref().map(|window| window.0),
        five_hour_reset_local: five.and_then(|window| window.1),
        seven_day_reset_local: seven.and_then(|window| window.1),
        extra_usage_percent,
    })
}

fn usage_window(root: &Value, key: &str) -> Option<(f64, Option<String>)> {
    let window = root.get(key)?.as_object()?;
    let percent = flexible_number(window.get("utilization")?).map(clamp_percent)?;
    let reset = window.get("resets_at").and_then(local_label);
    Some((percent, reset))
}

fn weekly_usage_window(root: &Value) -> Option<(f64, Option<String>)> {
    for key in [
        "seven_day",
        "seven_day_oauth_apps",
        "seven_day_sonnet",
        "seven_day_opus",
    ] {
        if let Some(window) = usage_window(root, key) {
            return Some(window);
        }
    }
    weekly_limit_window(root)
}

fn weekly_limit_window(root: &Value) -> Option<(f64, Option<String>)> {
    let weekly = root
        .get("limits")?
        .as_array()?
        .iter()
        .filter(|entry| {
            entry
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.contains("weekly"))
                || entry
                    .get("group")
                    .and_then(Value::as_str)
                    .is_some_and(|group| group.eq_ignore_ascii_case("weekly"))
        })
        .filter(|entry| entry.get("percent").and_then(flexible_number).is_some())
        .collect::<Vec<_>>();
    let entry = weekly
        .iter()
        .find(|entry| is_all_models_limit(entry))
        .copied()
        .or_else(|| weekly.first().copied())?;
    let percent = flexible_number(entry.get("percent")?).map(clamp_percent)?;
    let reset = entry.get("resets_at").and_then(local_label);
    Some((percent, reset))
}

fn is_all_models_limit(entry: &Value) -> bool {
    let Some(model) = entry.get("scope").and_then(|scope| scope.get("model")) else {
        return true;
    };
    if model.is_null() {
        return true;
    }
    model
        .get("display_name")
        .and_then(Value::as_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("all models"))
}

fn extra_usage_percent(root: &Value) -> Option<f64> {
    let extra = root.get("extra_usage")?.as_object()?;
    if extra.get("is_enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }

    extra
        .get("utilization")
        .and_then(flexible_number)
        .or_else(|| {
            let used = flexible_number(extra.get("used_credits")?)?;
            let limit = flexible_number(extra.get("monthly_limit")?)?;
            (limit > 0.0).then_some(used / limit * 100.0)
        })
        .map(clamp_percent)
}

fn flexible_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

// ---------- Credentials ----------

#[derive(Clone)]
struct Creds {
    access_token: String,
    refresh_token: String,
    expires_at: Option<DateTime<Utc>>,
}

impl Creds {
    fn is_expired_soon(&self) -> bool {
        match self.expires_at {
            Some(exp) => Utc::now() + Duration::minutes(2) >= exp,
            None => false,
        }
    }

    async fn refresh(&mut self, _client: &Client) -> Result<(), RefreshError> {
        if self.refresh_token.is_empty() {
            return Err(RefreshError::MissingRefreshToken);
        }
        Err(RefreshError::DirectRefreshUnavailable)
    }
}

async fn try_claude_code_refresh(config: &Config, creds: &mut Creds) -> anyhow::Result<()> {
    let refresh_config = config.clone();
    let mut candidate = creds.clone();
    let refreshed = tokio::task::spawn_blocking(move || {
        try_claude_code_refresh_blocking(&refresh_config, &mut candidate)?;
        Ok::<Creds, anyhow::Error>(candidate)
    })
    .await
    .context("join Claude Code refresh")??;
    *creds = refreshed;
    Ok(())
}

fn try_claude_code_refresh_blocking(config: &Config, creds: &mut Creds) -> anyhow::Result<()> {
    if !config.claude_code_refresh_enabled {
        bail!("Claude Code refresh disabled");
    }
    if !reserve_claude_code_recovery()? {
        bail!("Claude Code recovery is temporarily throttled");
    }

    let before_access = creds.access_token.clone();
    let before_expires = creds.expires_at;

    let mut cmd = build_claude_code_refresh_command(config);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    let timeout = StdDuration::from_secs(config.claude_code_refresh_timeout_seconds.clamp(
        MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
        MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
    ));
    let run_result = run_with_timeout(cmd, timeout);

    // Claude Code may refresh credentials before a non-zero exit, e.g. when the
    // configured budget is too low for the actual tiny prompt. Trust the file,
    // not the process status, and never expose command output to the renderer.
    let refreshed = load(config)?;
    let has_new_access =
        refreshed.access_token != before_access || refreshed.expires_at != before_expires;
    let is_fresh = match refreshed.expires_at {
        Some(exp) => exp > Utc::now() + Duration::minutes(5),
        None => !refreshed.access_token.is_empty(),
    };

    if has_new_access && is_fresh {
        *creds = refreshed;
        return Ok(());
    }

    run_result?;
    bail!("Claude Code did not refresh credentials")
}

fn reserve_claude_code_recovery() -> anyhow::Result<bool> {
    reserve_claude_code_recovery_at(&cache_dir(), Utc::now())
}

fn reserve_claude_code_recovery_at(dir: &Path, now: DateTime<Utc>) -> anyhow::Result<bool> {
    std::fs::create_dir_all(dir).context("create Claude recovery state directory")?;
    let lock_path = dir.join(CLAUDE_CODE_RECOVERY_LOCK_FILE);
    let _lock = OsFileLock::acquire(&lock_path, RECOVERY_LOCK_WAIT)?;
    let state_path = dir.join(CLAUDE_CODE_RECOVERY_STATE_FILE);

    let last_attempt = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|text| DateTime::parse_from_rfc3339(text.trim()).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc));
    if last_attempt.is_some_and(|timestamp| now - timestamp < CLAUDE_CODE_RECOVERY_THROTTLE) {
        return Ok(false);
    }

    atomic_write(&state_path, now.to_rfc3339().as_bytes())
        .context("persist Claude recovery attempt")?;
    Ok(true)
}

fn build_claude_code_refresh_command(config: &Config) -> Command {
    let mut cmd = Command::new(config.claude_code_command.trim());
    add_claude_code_refresh_args(&mut cmd, config);
    cmd
}

fn add_claude_code_refresh_args(cmd: &mut Command, config: &Config) {
    let budget = claude_code_refresh_budget(config);
    cmd.args([
        "-p",
        "OK",
        "--output-format",
        "json",
        "--tools",
        "",
        "--model",
        "claude-haiku-4-5-20251001",
        "--max-budget-usd",
        &budget,
    ]);
}

fn claude_code_refresh_budget(config: &Config) -> String {
    let configured = config.claude_code_refresh_max_budget_usd;
    if configured.is_finite() && configured > 0.0 {
        configured
            .clamp(
                MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
                MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
            )
            .to_string()
    } else {
        DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD.to_string()
    }
}

fn run_with_timeout(mut cmd: Command, timeout: StdDuration) -> anyhow::Result<()> {
    let mut child = cmd.spawn().context("spawn Claude Code refresh")?;
    let start = Instant::now();

    loop {
        if let Some(status) = child.try_wait().context("poll Claude Code refresh")? {
            if status.success() {
                return Ok(());
            }
            bail!("Claude Code refresh exited with {status}");
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Claude Code refresh timed out");
        }

        std::thread::sleep(StdDuration::from_millis(250));
    }
}

fn load(config: &Config) -> anyhow::Result<Creds> {
    parse_credentials(&resolve_and_read(config)?)
}

fn parse_credentials(text: &str) -> anyhow::Result<Creds> {
    let root: Value = serde_json::from_str(text).context("parse Claude credentials JSON")?;
    let oauth = &root["claudeAiOauth"];
    let access = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let refresh = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let expires_at = parse_datetime(&oauth["expiresAt"]);
    if access.is_empty() && refresh.is_empty() {
        bail!("Claude OAuth token missing");
    }
    Ok(Creds {
        access_token: access,
        refresh_token: refresh,
        expires_at,
    })
}

fn resolve_and_read(config: &Config) -> anyhow::Result<String> {
    let configured = config.claude_credentials_path.trim();

    if !configured.is_empty() {
        let path = PathBuf::from(expand(configured));
        return std::fs::read_to_string(&path)
            .with_context(|| format!("read Claude credentials at {}", path.display()));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".claude").join(".credentials.json"));
        candidates.push(home.join(".claude").join("credentials.json"));
    }
    for p in candidates {
        if let Ok(text) = std::fs::read_to_string(&p) {
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
    }

    bail!("Claude credentials not found")
}

/// Minimal expansion of a leading `~` and `%VAR%` segments.
fn expand(p: &str) -> String {
    let mut s = p.to_string();
    if let Some(rest) = s.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            s = format!("{}{}", home.display(), rest);
        }
    }
    while let Some(start) = s.find('%') {
        if let Some(end_rel) = s[start + 1..].find('%') {
            let end = start + 1 + end_rel;
            let var = &s[start + 1..end];
            let val = std::env::var(var).unwrap_or_default();
            s = format!("{}{}{}", &s[..start], val, &s[end + 1..]);
        } else {
            break;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{
        claude_code_refresh_budget, parse_usage, recovery_policy, reserve_claude_code_recovery_at,
        resolve_and_read, Creds, OsFileLock, RefreshError,
    };
    use crate::config::Config;
    use chrono::{Duration, TimeZone, Utc};
    use reqwest::Client;
    use std::path::PathBuf;
    use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

    fn test_creds() -> Creds {
        Creds {
            access_token: "old-access".into(),
            refresh_token: "old-refresh".into(),
            expires_at: None,
        }
    }

    fn test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ai-usage-dashboard-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn cli_recovery_policy_excludes_transient_and_parse_failures() {
        for err in [
            RefreshError::MissingRefreshToken,
            RefreshError::DirectRefreshUnavailable,
        ] {
            assert!(
                recovery_policy(&err).expect("non-rate-limit policy").0,
                "expected CLI recovery for {err:?}"
            );
        }

        for err in [
            RefreshError::Other(anyhow::anyhow!("transport failure")),
            RefreshError::Other(anyhow::anyhow!("HTTP 500")),
            RefreshError::Other(anyhow::anyhow!("parse refresh response")),
        ] {
            assert!(
                !recovery_policy(&err).expect("non-rate-limit policy").0,
                "unexpected CLI recovery for {err:?}"
            );
        }
    }

    #[test]
    fn local_credentials_never_use_direct_oauth_refresh() {
        let mut creds = test_creds();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");

        let result = runtime.block_on(creds.refresh(&Client::new()));

        assert!(matches!(
            result,
            Err(RefreshError::DirectRefreshUnavailable)
        ));
    }

    #[test]
    fn cli_recovery_budget_is_defensively_capped() {
        let too_high = Config {
            claude_code_refresh_max_budget_usd: 50.0,
            ..Config::default()
        };
        let invalid = Config {
            claude_code_refresh_max_budget_usd: f64::NAN,
            ..Config::default()
        };

        assert_eq!(claude_code_refresh_budget(&too_high), "0.1");
        assert_eq!(claude_code_refresh_budget(&invalid), "0.03");
    }

    #[test]
    fn configured_credential_path_is_fail_closed() {
        let config = Config {
            claude_credentials_path: std::env::temp_dir()
                .join(format!(
                    "missing-ai-dashboard-claude-credentials-{}",
                    std::process::id()
                ))
                .display()
                .to_string(),
            ..Config::default()
        };

        assert!(resolve_and_read(&config).is_err());

        let mut config = config;
        config.claude_credentials_path = "wsl:Ubuntu:/home/user/.claude/.credentials.json".into();
        assert!(resolve_and_read(&config).is_err());
    }

    #[test]
    fn recovery_file_lock_serializes_dashboard_processes() {
        let dir = test_dir("recovery-lock");
        let path = dir.join("recovery.lock");
        let first =
            OsFileLock::acquire(&path, StdDuration::from_secs(1)).expect("first recovery lock");
        assert!(OsFileLock::acquire(&path, StdDuration::ZERO).is_err());
        drop(first);
        OsFileLock::acquire(&path, StdDuration::from_secs(1)).expect("recovery lock after release");
        std::fs::remove_dir_all(dir).expect("remove recovery lock test directory");
    }

    #[test]
    fn failed_cli_attempt_consumes_persistent_throttle_slot() {
        let dir = test_dir("recovery-throttle");
        let now = Utc
            .with_ymd_and_hms(2026, 7, 11, 10, 0, 0)
            .single()
            .expect("fixed timestamp");

        assert!(reserve_claude_code_recovery_at(&dir, now).expect("reserve first attempt"));
        assert!(
            !reserve_claude_code_recovery_at(&dir, now + Duration::minutes(29))
                .expect("throttle second attempt")
        );
        assert!(
            reserve_claude_code_recovery_at(&dir, now + Duration::minutes(30))
                .expect("reserve after throttle")
        );

        std::fs::remove_dir_all(dir).expect("remove recovery throttle test directory");
    }

    #[test]
    fn claude_utilization_is_already_percent_scale() {
        let service = parse_usage(
            r#"{
                "five_hour":{"utilization":0.42,"resets_at":"2026-07-10T08:30:00Z"},
                "seven_day":{"utilization":1,"resets_at":"2026-07-17T08:30:00Z"}
            }"#,
        )
        .expect("valid Claude usage");

        assert_eq!(service.five_hour_percent, Some(0.42));
        assert_eq!(service.seven_day_percent, Some(1.0));
        assert_eq!(service.extra_usage_percent, None);
    }

    #[test]
    fn claude_usage_accepts_either_standard_window() {
        let weekly = parse_usage(
            r#"{"five_hour":null,"seven_day":{"utilization":"37.5","resets_at":"2026-07-29T08:30:00Z"}}"#,
        )
        .expect("valid weekly-only Claude usage");
        assert_eq!(weekly.five_hour_percent, None);
        assert_eq!(weekly.seven_day_percent, Some(37.5));
        assert!(weekly.seven_day_reset_local.is_some());

        let session = parse_usage(r#"{"five_hour":{"utilization":12},"seven_day":null}"#)
            .expect("valid session-only Claude usage");
        assert_eq!(session.five_hour_percent, Some(12.0));
        assert_eq!(session.seven_day_percent, None);
    }

    #[test]
    fn claude_usage_falls_back_to_scoped_weekly_windows() {
        let legacy = parse_usage(
            r#"{"seven_day":null,"seven_day_sonnet":{"utilization":44,"resets_at":"2026-07-29T08:30:00Z"}}"#,
        )
        .expect("valid model-specific weekly Claude response");
        assert_eq!(legacy.seven_day_percent, Some(44.0));

        let limits = parse_usage(
            r#"{
                "limits":[
                    {"kind":"weekly_scoped","percent":66,"scope":{"model":{"display_name":"Fable"}}},
                    {"group":"weekly","percent":"31","resets_at":"2026-07-29T08:30:00Z","scope":{"model":{"display_name":"All models"}}}
                ]
            }"#,
        )
        .expect("valid limits-array Claude response");
        assert_eq!(limits.seven_day_percent, Some(31.0));
        assert!(limits.seven_day_reset_local.is_some());
    }

    #[test]
    fn claude_usage_accepts_extra_usage_only_accounts() {
        let service = parse_usage(
            r#"{"extra_usage":{"is_enabled":true,"used_credits":"25","monthly_limit":"200"}}"#,
        )
        .expect("valid extra-usage-only Claude response");

        assert_eq!(service.five_hour_percent, None);
        assert_eq!(service.seven_day_percent, None);
        assert_eq!(service.extra_usage_percent, Some(12.5));
    }

    #[test]
    fn claude_usage_rejects_responses_without_usable_usage() {
        for body in [
            r#"{}"#,
            r#"{"five_hour":{}}"#,
            r#"{"extra_usage":{"is_enabled":false,"utilization":10}}"#,
            r#"{"extra_usage":{"used_credits":1,"monthly_limit":0}}"#,
        ] {
            assert!(parse_usage(body).is_err(), "unexpectedly accepted {body}");
        }
    }
}
