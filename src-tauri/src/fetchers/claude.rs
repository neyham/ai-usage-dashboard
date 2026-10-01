//! Claude usage fetcher. Reads OAuth credentials from the local
//! `.claude/.credentials.json`. Native credential files are read-only. An expired
//! access token is renewed by starting Claude Code so it can rewrite its own
//! file. The dashboard never posts the refresh token. Usage 429s are signaled
//! back to the orchestrator so it can enter cooldown.

use super::{send_with_one_retry, FetchError};
use crate::cache::cache_dir;
use crate::config::{
    Config, MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS, MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
};
use crate::fs_util::{atomic_write, OsFileLock};
use crate::models::ClaudeService;
use crate::util::{clamp_percent, local_label, parse_datetime};
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Duration, Utc};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use reqwest::Client;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Stdio;
use std::thread::JoinHandle;
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
// Only override probe behavior. Preserve user connection settings (proxy, CA,
// etc.); managed policy remains under Claude Code's control.
const CLAUDE_CODE_TOUCH_SETTINGS: &str = r#"{"remoteControlAtStartup":false,"disableAllHooks":true,"disableDeepLinkRegistration":"disable"}"#;

#[derive(Debug)]
enum RefreshError {
    MissingRefreshToken,
    DirectRefreshUnavailable,
    #[allow(dead_code)]
    Other(anyhow::Error),
}

pub async fn fetch(config: &Config, client: &Client) -> Result<ClaudeService, FetchError> {
    let load_config = config.clone();
    let (config, mut creds) = tokio::task::spawn_blocking(move || load_selected(&load_config))
        .await
        .map_err(|err| FetchError::Other(anyhow!("Claude credential task failed: {err}")))?
        .map_err(load_error)?;

    if creds.is_expired_at(Utc::now()) {
        refresh_or_recover(&config, client, &mut creds).await?;
    }

    let mut resp = send_with_one_retry(|| usage_request(client, &creds.access_token))
        .await
        .map_err(FetchError::Other)?;

    if resp.status == 401 {
        refresh_or_recover(&config, client, &mut creds).await?;
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

    let mut service = parse_usage(&resp.body).map_err(FetchError::Other)?;
    service.plan = creds.plan.clone();
    Ok(service)
}

async fn refresh_or_recover(
    config: &Config,
    client: &Client,
    creds: &mut Creds,
) -> Result<(), FetchError> {
    // Another Claude process may have rotated the credential while our request
    // was in flight. Re-read before checking refresh eligibility or cooldown.
    let read_config = config.clone();
    if let Ok(Ok(fresh)) = tokio::task::spawn_blocking(move || load(&read_config)).await {
        if adopt_refreshed_credentials(creds, fresh) {
            return Ok(());
        }
    }
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
    // `claudeCodeRefreshEnabled` used to opt into a billed prompt. Renewal now
    // starts Claude Code itself and does not consult that flag.
    if delegates_to_claude_code(&err) && try_claude_code_refresh(config, creds).await.is_ok() {
        return Ok(());
    }
    Err(FetchError::Auth {
        message: auth_failure_message(&err),
    })
}

fn delegates_to_claude_code(err: &RefreshError) -> bool {
    matches!(err, RefreshError::DirectRefreshUnavailable)
}

fn auth_failure_message(err: &RefreshError) -> &'static str {
    match err {
        RefreshError::MissingRefreshToken => MSG_AUTH_EXPIRED,
        RefreshError::DirectRefreshUnavailable => MSG_REFRESH_BLOCKED,
        RefreshError::Other(_) => MSG_AUTH_CHECK_FAILED,
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
        plan: None,
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
    /// Allowlisted subscription label. Raw tier strings are not kept.
    plan: Option<String>,
}

impl Creds {
    fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        // A still-valid token can answer the read-only usage request. Do not
        // turn a speculative early renewal failure into a dashboard outage.
        self.access_token.is_empty() || self.expires_at.is_some_and(|exp| exp <= now)
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
    try_claude_code_refresh_blocking_at(config, creds, &cache_dir())
}

fn try_claude_code_refresh_blocking_at(
    config: &Config,
    creds: &mut Creds,
    recovery_dir: &Path,
) -> anyhow::Result<()> {
    if adopt_refreshed_credentials(creds, load(config)?) {
        return Ok(());
    }
    // Only the canonical Claude-owned filename can be renewed by the CLI.
    // Imported/renamed files remain readable but must not refresh another login.
    claude_profile_dir(config)?;
    if !reserve_claude_code_recovery_at(recovery_dir, Utc::now())? {
        if adopt_refreshed_credentials(creds, load(config)?) {
            return Ok(());
        }
        bail!("Claude Code recovery is temporarily throttled");
    }

    let before_access = creds.access_token.clone();
    let before_expires = creds.expires_at;
    let timeout = StdDuration::from_secs(config.claude_code_refresh_timeout_seconds.clamp(
        MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
        MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
    ));
    // Trust the credential file Claude Code writes. Discard the terminal: it can
    // contain account details, and a non-zero exit can still follow a refresh.
    let run_result = run_claude_code_touch(config, recovery_dir, timeout, || {
        load(config)
            .ok()
            .is_some_and(|fresh| credentials_refreshed(&before_access, before_expires, &fresh))
    });

    let refreshed = load(config)?;
    if credentials_refreshed(&before_access, before_expires, &refreshed) {
        *creds = refreshed;
        return Ok(());
    }

    run_result?;
    bail!("Claude Code did not refresh credentials")
}

fn credentials_refreshed(
    before_access: &str,
    before_expires: Option<DateTime<Utc>>,
    fresh: &Creds,
) -> bool {
    let has_new_access = fresh.access_token != before_access || fresh.expires_at != before_expires;
    has_new_access && !fresh.is_expired_at(Utc::now())
}

fn adopt_refreshed_credentials(creds: &mut Creds, fresh: Creds) -> bool {
    if credentials_refreshed(&creds.access_token, creds.expires_at, &fresh) {
        *creds = fresh;
        true
    } else {
        false
    }
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

fn claude_code_touch_arguments() -> [&'static str; 5] {
    // Let Claude Code allocate a fresh session: --session-id does not resume
    // an existing conversation and a reused id can prevent startup.
    [
        "--tools",
        "",
        "--strict-mcp-config",
        "--settings",
        CLAUDE_CODE_TOUCH_SETTINGS,
    ]
}

fn prepare_claude_probe_dir(recovery_dir: &Path) -> anyhow::Result<PathBuf> {
    let dir = recovery_dir.join("claude-code-probe");
    let claude_dir = dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).context("create Claude Code probe directory")?;
    atomic_write(
        &claude_dir.join("settings.local.json"),
        br#"{"disableDeepLinkRegistration":"disable"}"#,
    )
    .context("write Claude Code probe settings")?;
    Ok(dir)
}

/// Start Claude Code in a private terminal so it refreshes its own login.
/// `/status` is typed only if startup has not already rewritten the file.
/// Terminal output is discarded.
fn run_claude_code_touch(
    config: &Config,
    recovery_dir: &Path,
    timeout: StdDuration,
    mut refreshed: impl FnMut() -> bool,
) -> anyhow::Result<()> {
    if refreshed() {
        return Ok(());
    }

    let profile = claude_profile_dir(config)?;
    let probe = prepare_claude_probe_dir(recovery_dir)?;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 32,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("open Claude Code terminal")?;
    let mut cmd = CommandBuilder::new(config.claude_code_command.trim());
    cmd.args(claude_code_touch_arguments());
    cmd.cwd(&probe);
    // The reader and CLI must address the same profile. Keep the default
    // profile's environment unchanged when no override was requested.
    let default_profile = dirs::home_dir().map(|home| home.join(".claude"));
    if std::env::var_os("CLAUDE_CONFIG_DIR").is_some()
        || default_profile.as_deref() != Some(profile.as_path())
    {
        cmd.env("CLAUDE_CONFIG_DIR", &profile);
    }
    cmd.env("DISABLE_AUTOUPDATER", "1");
    if let Some(path) = probe.as_os_str().to_str() {
        cmd.env("PWD", path);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .context("spawn Claude Code refresh")?;
    let mut session = RunningClaudeTouch {
        child: Some(child),
        writer: None,
        master: None,
        reader: None,
    };
    drop(pair.slave);
    session.writer = Some(
        pair.master
            .take_writer()
            .context("Claude Code terminal writer")?,
    );
    let reader = pair
        .master
        .try_clone_reader()
        .context("Claude Code terminal reader")?;
    session.reader = Some(discard_pty_output(reader));
    session.master = Some(pair.master);
    session.wait_until(timeout, &mut refreshed)
}

struct RunningClaudeTouch {
    child: Option<Box<dyn Child + Send + Sync>>,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn MasterPty + Send>>,
    reader: Option<JoinHandle<()>>,
}

impl RunningClaudeTouch {
    fn wait_until(
        &mut self,
        timeout: StdDuration,
        refreshed: &mut impl FnMut() -> bool,
    ) -> anyhow::Result<()> {
        let start = Instant::now();
        let mut sent_status = false;
        let mut last_enter = start;
        loop {
            if refreshed() {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                bail!("Claude Code refresh timed out");
            }
            if let Some(writer) = self.writer.as_mut() {
                if !sent_status && start.elapsed() >= StdDuration::from_millis(2500) {
                    let _ = writer.write_all(b"/status\r");
                    let _ = writer.flush();
                    sent_status = true;
                    last_enter = Instant::now();
                } else if sent_status && last_enter.elapsed() >= StdDuration::from_millis(800) {
                    let _ = writer.write_all(b"\r");
                    let _ = writer.flush();
                    last_enter = Instant::now();
                }
            }
            if let Some(child) = self.child.as_mut() {
                if child
                    .try_wait()
                    .context("poll Claude Code refresh")?
                    .is_some()
                {
                    if refreshed() {
                        return Ok(());
                    }
                    bail!("Claude Code exited before refreshing credentials");
                }
            }
            std::thread::sleep(StdDuration::from_millis(200));
        }
    }
}

impl Drop for RunningClaudeTouch {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_claude_touch(&mut child);
        }
        self.writer.take();
        self.master.take();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

fn discard_pty_output(mut reader: Box<dyn Read + Send>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

fn terminate_claude_touch(child: &mut Box<dyn Child + Send + Sync>) {
    #[cfg(unix)]
    let pid = child.process_id();
    let _ = child.kill();
    // setsid() makes the child its own process group. SIGHUP from kill() does
    // not cover grandchildren if Claude Code handles that signal.
    #[cfg(unix)]
    if let Some(pid) = pid {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.wait();
}

fn load(config: &Config) -> anyhow::Result<Creds> {
    parse_credentials(&resolve_and_read(config)?)
}

fn load_selected(config: &Config) -> anyhow::Result<(Config, Creds)> {
    let (path, text) = resolve_and_read_with_source(config)?;
    let creds = parse_credentials(&text)?;
    let mut selected = config.clone();
    selected.claude_credentials_path = path
        .to_str()
        .context("Claude credential path is not UTF-8")?
        .to_owned();
    Ok((selected, creds))
}

fn claude_profile_dir(config: &Config) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(expand(config.claude_credentials_path.trim()));
    if path.file_name().and_then(|name| name.to_str()) != Some(".credentials.json") {
        bail!("Claude renewal requires the native .credentials.json file");
    }
    let path = std::path::absolute(path).context("resolve Claude credential path")?;
    Ok(path
        .parent()
        .context("Claude credential directory missing")?
        .to_path_buf())
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
        plan: claude_plan_label(
            oauth.get("subscriptionType").and_then(Value::as_str),
            oauth.get("rateLimitTier").and_then(Value::as_str),
        )
        .map(str::to_string),
    })
}

/// Subscription badge from Claude Code's credential file. The usage endpoint
/// does not return a plan. Unknown values stay hidden.
fn claude_plan_label(
    subscription_type: Option<&str>,
    rate_limit_tier: Option<&str>,
) -> Option<&'static str> {
    match normalize_plan_token(rate_limit_tier).as_str() {
        "default_claude_max_20x" => return Some("Max 20x"),
        "default_claude_max_5x" => return Some("Max 5x"),
        _ => {}
    }
    match normalize_plan_token(subscription_type).as_str() {
        "pro" => Some("Pro"),
        "max" => Some("Max"),
        "team" => Some("Team"),
        "enterprise" => Some("Enterprise"),
        _ => None,
    }
}

fn normalize_plan_token(value: Option<&str>) -> String {
    value.unwrap_or("").trim().to_ascii_lowercase()
}

fn resolve_and_read(config: &Config) -> anyhow::Result<String> {
    resolve_and_read_with_source(config).map(|(_, text)| text)
}

fn credential_paths(
    configured: &str,
    config_dir: Option<PathBuf>,
    home: Option<PathBuf>,
) -> anyhow::Result<Vec<PathBuf>> {
    if !configured.trim().is_empty() {
        return Ok(vec![std::path::absolute(expand(configured.trim()))?]);
    }
    let root = config_dir
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| home.map(|home| home.join(".claude")))
        .context("Claude credentials not found")?;
    let root = std::path::absolute(root)?;
    Ok(vec![
        root.join(".credentials.json"),
        root.join("credentials.json"),
    ])
}

fn resolve_and_read_with_source(config: &Config) -> anyhow::Result<(PathBuf, String)> {
    let paths = credential_paths(
        &config.claude_credentials_path,
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir(),
    )?;
    // An explicit override is authoritative, including read/parse failures.
    if !config.claude_credentials_path.trim().is_empty() {
        let path = paths.into_iter().next().expect("explicit credential path");
        let text = std::fs::read_to_string(&path).context("read configured Claude credentials")?;
        return Ok((path, text));
    }
    for p in paths {
        if let Ok(text) = std::fs::read_to_string(&p) {
            if !text.trim().is_empty() {
                return Ok((p, text));
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
        adopt_refreshed_credentials, auth_failure_message, claude_code_touch_arguments,
        claude_plan_label, claude_profile_dir, credential_paths, credentials_refreshed,
        delegates_to_claude_code, load_selected, parse_credentials, parse_usage,
        reserve_claude_code_recovery_at, resolve_and_read, try_claude_code_refresh_blocking_at,
        Creds, OsFileLock, RefreshError,
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
            plan: None,
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
    fn cli_recovery_delegates_only_an_expired_refreshable_login() {
        let expired = RefreshError::DirectRefreshUnavailable;
        assert!(delegates_to_claude_code(&expired));
        assert_eq!(auth_failure_message(&expired), "REFRESH BLOCKED");

        let missing = RefreshError::MissingRefreshToken;
        assert!(!delegates_to_claude_code(&missing));
        assert_eq!(auth_failure_message(&missing), "AUTH EXPIRED");

        for err in [
            RefreshError::Other(anyhow::anyhow!("transport failure")),
            RefreshError::Other(anyhow::anyhow!("HTTP 500")),
            RefreshError::Other(anyhow::anyhow!("parse refresh response")),
        ] {
            assert!(
                !delegates_to_claude_code(&err),
                "unexpected CLI recovery for {err:?}"
            );
            assert_eq!(auth_failure_message(&err), "AUTH CHECK FAILED");
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
    fn claude_code_touch_does_not_send_a_billed_prompt() {
        let args = claude_code_touch_arguments();
        assert!(args.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(args.contains(&"--strict-mcp-config"));
        // Hooks are suppressed locally without removing proxy/CA settings.
        assert!(!args.contains(&"--setting-sources"));
        let settings_index = args.iter().position(|arg| *arg == "--settings").unwrap();
        let settings: serde_json::Value = serde_json::from_str(args[settings_index + 1]).unwrap();
        assert_eq!(settings["disableAllHooks"], true);
        assert_eq!(settings["remoteControlAtStartup"], false);
        assert_eq!(settings["disableDeepLinkRegistration"], "disable");
        assert!(!args.contains(&"--allowed-tools"));
        assert!(!args.contains(&"--session-id"));
        assert!(!args.contains(&"--resume"));
        assert!(!args.contains(&"--continue"));
        assert!(!args.contains(&"-p"));
        assert!(!args.contains(&"--print"));
        assert!(!args.contains(&"--model"));
        assert!(!args.contains(&"--max-budget-usd"));
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
    fn usable_access_is_not_rejected_for_being_near_expiry() {
        let now = Utc::now();
        let mut creds = test_creds();
        creds.expires_at = Some(now + Duration::seconds(30));
        assert!(!creds.is_expired_at(now));
        creds.expires_at = Some(now);
        assert!(creds.is_expired_at(now));
        creds.expires_at = None;
        assert!(!creds.is_expired_at(now));
        creds.access_token.clear();
        assert!(creds.is_expired_at(now));
        creds.expires_at = Some(now + Duration::hours(8));
        assert!(creds.is_expired_at(now));
    }

    #[test]
    fn renewal_requires_a_changed_usable_access_token() {
        let original = test_creds();
        assert!(!credentials_refreshed(
            &original.access_token,
            None,
            &original
        ));
        let mut fresh = original.clone();
        fresh.access_token = "synthetic-new-access".into();
        fresh.expires_at = Some(Utc::now() + Duration::minutes(1));
        assert!(credentials_refreshed(&original.access_token, None, &fresh));
        fresh.expires_at = Some(Utc::now() - Duration::seconds(1));
        assert!(!credentials_refreshed(&original.access_token, None, &fresh));
        fresh.access_token.clear();
        fresh.expires_at = Some(Utc::now() + Duration::hours(8));
        let mut retained = original.clone();
        assert!(!adopt_refreshed_credentials(&mut retained, fresh));
        assert_eq!(retained.access_token, original.access_token);
    }

    #[test]
    fn profile_resolution_never_falls_back_to_another_home() {
        let home = test_dir("synthetic-home");
        let profile = test_dir("synthetic-profile");
        let explicit = test_dir("synthetic-explicit").join(".credentials.json");
        assert_eq!(
            credential_paths("", Some(profile.clone()), Some(home.clone())).unwrap(),
            vec![
                profile.join(".credentials.json"),
                profile.join("credentials.json")
            ]
        );
        assert_eq!(
            credential_paths(
                explicit.to_str().unwrap(),
                Some(profile),
                Some(home.clone())
            )
            .unwrap(),
            vec![explicit]
        );
        assert_eq!(
            credential_paths("", None, Some(home.clone())).unwrap()[0],
            home.join(".claude/.credentials.json")
        );
    }

    fn write_synthetic_credentials(path: &std::path::Path, access: &str) {
        std::fs::write(
            path,
            serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": access,
                    "refreshToken": "synthetic-refresh",
                    "expiresAt": (Utc::now() + Duration::hours(1)).timestamp_millis(),
                    "subscriptionType": "pro"
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn external_rotation_is_adopted_even_during_persistent_cooldown() {
        let dir = test_dir("external-rotation");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".credentials.json");
        write_synthetic_credentials(&path, "synthetic-rotated-access");
        let config = Config {
            claude_credentials_path: path.to_str().unwrap().into(),
            claude_code_command: "must-not-be-launched".into(),
            ..Config::default()
        };
        reserve_claude_code_recovery_at(&dir, Utc::now()).unwrap();
        let mut creds = test_creds();
        try_claude_code_refresh_blocking_at(&config, &mut creds, &dir).unwrap();
        assert_eq!(creds.access_token, "synthetic-rotated-access");
        assert_eq!(creds.plan.as_deref(), Some("Pro"));
        let (selected, _) = load_selected(&config).unwrap();
        assert_eq!(claude_profile_dir(&selected).unwrap(), dir);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recovery_rereads_before_rejecting_an_old_nonrefreshable_token() {
        let dir = test_dir("reread-before-auth-error");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".credentials.json");
        write_synthetic_credentials(&path, "synthetic-rotated-access");
        let config = Config {
            claude_credentials_path: path.to_str().unwrap().into(),
            claude_code_command: "must-not-be-launched".into(),
            ..Config::default()
        };
        let mut creds = test_creds();
        creds.refresh_token.clear();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime
            .block_on(super::refresh_or_recover(
                &config,
                &Client::new(),
                &mut creds,
            ))
            .unwrap();
        assert_eq!(creds.access_token, "synthetic-rotated-access");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn renamed_imports_do_not_trigger_renewal_of_an_unrelated_profile() {
        let config = Config {
            claude_credentials_path: test_dir("import")
                .join("export.json")
                .to_str()
                .unwrap()
                .into(),
            ..Config::default()
        };
        assert!(claude_profile_dir(&config).is_err());
    }

    #[cfg(unix)]
    fn fake_claude(dir: &std::path::Path, body: &str) -> Config {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let command = dir.join("fake-claude");
        std::fs::write(&command, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
        Config {
            claude_credentials_path: dir.join(".credentials.json").to_str().unwrap().into(),
            claude_code_command: command.to_str().unwrap().into(),
            ..Config::default()
        }
    }

    #[cfg(unix)]
    #[test]
    fn cli_rotation_is_read_from_the_selected_profile_even_after_nonzero_exit() {
        let dir = test_dir("pty-rotation");
        let config = fake_claude(
            &dir,
            r#"
printf '%s\n' "$@" > "$CLAUDE_CONFIG_DIR/observed-args"
cat > "$CLAUDE_CONFIG_DIR/.credentials.json" <<'JSON'
{"claudeAiOauth":{"accessToken":"synthetic-cli-access","refreshToken":"synthetic-cli-refresh","expiresAt":4102444800000}}
JSON
exit 7
"#,
        );
        let mut creds = test_creds();
        std::fs::write(
            &config.claude_credentials_path,
            r#"{"claudeAiOauth":{"accessToken":"old-access","refreshToken":"old-refresh"}}"#,
        )
        .unwrap();
        try_claude_code_refresh_blocking_at(&config, &mut creds, &dir).unwrap();
        assert_eq!(creds.access_token, "synthetic-cli-access");
        let args = std::fs::read_to_string(dir.join("observed-args")).unwrap();
        assert!(args.contains("--tools\n\n"));
        assert!(args.contains("\"disableAllHooks\":true"));
        assert!(!args.contains("--setting-sources"));
        assert!(!args.contains("--session-id"));
        assert!(!args.contains("old-access"));
        assert!(!args.contains("old-refresh"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_successful_cli_exit_without_rotation_does_not_claim_recovery() {
        let dir = test_dir("pty-no-rotation");
        let config = fake_claude(&dir, "exit 0");
        let mut creds = test_creds();
        std::fs::write(
            &config.claude_credentials_path,
            r#"{"claudeAiOauth":{"accessToken":"old-access","refreshToken":"old-refresh"}}"#,
        )
        .unwrap();
        assert!(try_claude_code_refresh_blocking_at(&config, &mut creds, &dir).is_err());
        assert_eq!(creds.access_token, "old-access");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unresponsive_cli_is_stopped_at_the_probe_deadline() {
        let dir = test_dir("pty-timeout");
        let config = fake_claude(&dir, "exec /bin/sleep 30");
        let start = std::time::Instant::now();
        assert!(
            super::run_claude_code_touch(&config, &dir, StdDuration::from_millis(200), || false)
                .is_err()
        );
        assert!(start.elapsed() < StdDuration::from_secs(3));
        std::fs::remove_dir_all(dir).unwrap();
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
    fn claude_plan_is_allowlisted_subscription_metadata() {
        assert_eq!(
            claude_plan_label(Some("pro"), Some("default_claude_ai")),
            Some("Pro")
        );
        assert_eq!(
            claude_plan_label(Some("max"), Some("default_claude_max_5x")),
            Some("Max 5x")
        );
        assert_eq!(
            claude_plan_label(Some("max"), Some("default_claude_max_20x")),
            Some("Max 20x")
        );
        assert_eq!(claude_plan_label(Some("max"), None), Some("Max"));
        assert_eq!(claude_plan_label(Some("team"), None), Some("Team"));
        assert_eq!(
            claude_plan_label(Some("enterprise"), Some("default_claude_ai")),
            Some("Enterprise")
        );
        assert_eq!(claude_plan_label(None, Some("default_claude_ai")), None);
        assert_eq!(claude_plan_label(Some("injected-plan"), None), None);
        assert_eq!(
            claude_plan_label(Some("pro"), Some("default_claude_max_20x")),
            Some("Max 20x")
        );
    }

    #[test]
    fn credentials_keep_the_allowlisted_plan_only() {
        let creds = parse_credentials(
            r#"{
                "claudeAiOauth": {
                    "accessToken": "token-access",
                    "refreshToken": "token-refresh",
                    "expiresAt": 1790788852303,
                    "subscriptionType": "pro",
                    "rateLimitTier": "default_claude_ai",
                    "scopes": ["user:inference"]
                }
            }"#,
        )
        .expect("valid Claude credentials");

        assert_eq!(creds.plan.as_deref(), Some("Pro"));
        assert!(creds
            .plan
            .as_deref()
            .is_some_and(|plan| plan != "token-access"));
        assert!(creds
            .plan
            .as_deref()
            .is_some_and(|plan| plan != "default_claude_ai"));
    }

    #[test]
    fn usage_payload_does_not_invent_a_plan() {
        let service = parse_usage(r#"{"five_hour":{"utilization":1}}"#).expect("valid usage");
        assert_eq!(service.plan, None);
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
