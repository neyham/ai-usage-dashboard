//! Claude usage fetcher. Reads OAuth credentials from the local
//! `.claude/.credentials.json` (or, on Windows, falls back to reading the WSL
//! home credentials via `wsl.exe`). Refreshes on expiry / 401 and signals 429
//! back to the orchestrator so it can enter cooldown.

use super::{send_with_one_retry, FetchError};
use crate::config::Config;
use crate::models::ClaudeService;
use crate::util::{local_label, normalize_percent, parse_datetime};
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration as StdDuration, Instant};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_URL: &str = "https://claude.ai/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
const MSG_LOGIN_REQUIRED: &str = "LOGIN REQUIRED";
const MSG_AUTH_EXPIRED: &str = "AUTH EXPIRED";
const MSG_REFRESH_BLOCKED: &str = "REFRESH BLOCKED";
const MSG_AUTH_CHECK_FAILED: &str = "AUTH CHECK FAILED";

pub async fn fetch(config: &Config, client: &Client) -> Result<ClaudeService, FetchError> {
    let mut creds = load(config).map_err(load_error)?;

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
        Err(err) => {
            let message = auth_message_from_refresh_error(&err);
            if config.claude_code_refresh_enabled
                && try_claude_code_refresh(config, creds).is_ok()
            {
                return Ok(());
            }
            Err(FetchError::Auth { message })
        }
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

fn auth_message_from_refresh_error(err: &anyhow::Error) -> &'static str {
    let text = err.to_string();
    if text.contains("HTTP 403") {
        MSG_REFRESH_BLOCKED
    } else if text.contains("HTTP 400")
        || text.contains("HTTP 401")
        || text.contains("refresh token missing")
    {
        MSG_AUTH_EXPIRED
    } else {
        MSG_AUTH_CHECK_FAILED
    }
}

fn usage_request(client: &Client, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", ANTHROPIC_BETA)
}

fn refresh_request(client: &Client, refresh_token: &str) -> reqwest::RequestBuilder {
    client
        .post(TOKEN_URL)
        .header("Origin", "https://claude.ai")
        .header("Referer", "https://claude.ai/")
        .json(&json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
        }))
}

fn parse_usage(body: &str) -> anyhow::Result<ClaudeService> {
    let root: Value = serde_json::from_str(body).context("parse Claude usage body")?;
    let five = &root["five_hour"];
    let seven = &root["seven_day"];

    Ok(ClaudeService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        cooldown_until_local: None,
        five_hour_percent: five.get("utilization").and_then(Value::as_f64).map(normalize_percent),
        seven_day_percent: seven.get("utilization").and_then(Value::as_f64).map(normalize_percent),
        five_hour_reset_local: five.get("resets_at").and_then(local_label),
        seven_day_reset_local: seven.get("resets_at").and_then(local_label),
    })
}

// ---------- Credentials ----------

enum CredSource {
    File(PathBuf),
    #[cfg(windows)]
    Wsl { distro: String, path: String },
}

impl CredSource {
    #[cfg(windows)]
    fn wsl_distro(&self) -> Option<String> {
        match self {
            CredSource::Wsl { distro, .. } => Some(distro.clone()),
            CredSource::File(path) => {
                let text = path.to_string_lossy();
                for prefix in ["\\\\wsl.localhost\\", "\\\\wsl$\\"] {
                    if let Some(rest) = text.strip_prefix(prefix) {
                        return rest
                            .split(['\\', '/'])
                            .next()
                            .filter(|s| !s.is_empty())
                            .map(str::to_string);
                    }
                }
                None
            }
        }
    }
}

struct Creds {
    root: Value,
    access_token: String,
    refresh_token: String,
    expires_at: Option<DateTime<Utc>>,
    source: CredSource,
}

impl Creds {
    fn is_expired_soon(&self) -> bool {
        match self.expires_at {
            Some(exp) => Utc::now() + Duration::minutes(2) >= exp,
            None => false,
        }
    }

    async fn refresh(&mut self, client: &Client) -> anyhow::Result<()> {
        if self.refresh_token.is_empty() {
            bail!("Claude refresh token missing");
        }

        let resp = send_with_one_retry(|| refresh_request(client, &self.refresh_token)).await?;
        if !resp.is_success() {
            bail!("Claude token refresh HTTP {}", resp.status);
        }

        let data: Value = serde_json::from_str(&resp.body).context("parse refresh response")?;
        let access = str_field(&data, &["access_token", "accessToken"]);
        let refresh = str_field(&data, &["refresh_token", "refreshToken"]);

        let oauth = self
            .root
            .get_mut("claudeAiOauth")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow!("credentials missing claudeAiOauth object"))?;

        if !access.is_empty() {
            self.access_token = access.clone();
            oauth.insert("accessToken".into(), json!(access));
        }
        if !refresh.is_empty() {
            self.refresh_token = refresh.clone();
            oauth.insert("refreshToken".into(), json!(refresh));
        }

        // expires_at may come as a timestamp or as an expires_in duration.
        let mut expires = parse_datetime(&data["expires_at"]);
        if expires.is_none() {
            let secs = data
                .get("expires_in")
                .or_else(|| data.get("expiresIn"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if secs > 0 {
                expires = Some(Utc::now() + Duration::seconds(secs));
            }
        }
        if let Some(exp) = expires {
            self.expires_at = Some(exp);
            oauth.insert("expiresAt".into(), json!(exp.to_rfc3339()));
        }

        self.write_back()
    }

    fn write_back(&self) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(&self.root)?;
        match &self.source {
            CredSource::File(p) => std::fs::write(p, text).context("write Claude credentials")?,
            #[cfg(windows)]
            CredSource::Wsl { distro, path } => wsl_write(distro, path, &text)?,
        }
        Ok(())
    }
}

fn try_claude_code_refresh(config: &Config, creds: &mut Creds) -> anyhow::Result<()> {
    if !config.claude_code_refresh_enabled {
        bail!("Claude Code refresh disabled");
    }

    let before_access = creds.access_token.clone();
    let before_expires = creds.expires_at;

    let mut cmd = build_claude_code_refresh_command(config, &creds.source);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    let timeout = StdDuration::from_secs(config.claude_code_refresh_timeout_seconds.max(5));
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

fn build_claude_code_refresh_command(config: &Config, _source: &CredSource) -> Command {
    #[cfg(windows)]
    {
        if let Some(distro) = _source.wsl_distro() {
            let mut cmd = Command::new("wsl.exe");
            let shell = claude_code_refresh_shell(config);
            cmd.args(["-d", &distro, "--", "bash", "-lc", &shell]);
            return cmd;
        }
    }

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
    config
        .claude_code_refresh_max_budget_usd
        .max(0.001)
        .to_string()
}

#[cfg(windows)]
fn claude_code_refresh_shell(config: &Config) -> String {
    format!(
        "PATH=\"$HOME/.local/bin:$PATH\"; {} -p OK --output-format json --tools '' --model claude-haiku-4-5-20251001 --max-budget-usd {}",
        shell_quote(config.claude_code_command.trim()),
        shell_quote(&claude_code_refresh_budget(config))
    )
}

#[cfg(windows)]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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

fn str_field(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn load(config: &Config) -> anyhow::Result<Creds> {
    let (text, source) = resolve_and_read(config)?;
    let root: Value = serde_json::from_str(&text).context("parse Claude credentials JSON")?;
    let oauth = &root["claudeAiOauth"];
    let access = oauth.get("accessToken").and_then(Value::as_str).unwrap_or("").to_string();
    let refresh = oauth.get("refreshToken").and_then(Value::as_str).unwrap_or("").to_string();
    let expires_at = parse_datetime(&oauth["expiresAt"]);
    if access.is_empty() && refresh.is_empty() {
        bail!("Claude OAuth token missing");
    }
    Ok(Creds {
        root,
        access_token: access,
        refresh_token: refresh,
        expires_at,
        source,
    })
}

fn resolve_and_read(config: &Config) -> anyhow::Result<(String, CredSource)> {
    let configured = config.claude_credentials_path.trim();

    if !configured.is_empty() {
        if let Some(spec) = configured.strip_prefix("wsl:") {
            #[cfg(windows)]
            {
                if let Some((distro, path)) = spec.split_once(':') {
                    if let Some(text) = wsl_read(distro, path) {
                        return Ok((
                            text,
                            CredSource::Wsl {
                                distro: distro.to_string(),
                                path: path.to_string(),
                            },
                        ));
                    }
                }
                bail!("Claude WSL credentials not readable: {spec}");
            }
            #[cfg(not(windows))]
            {
                let _ = spec; // No WSL bridge off Windows; fall through to local paths.
            }
        } else {
            let p = PathBuf::from(expand(configured));
            if p.exists() {
                let text = std::fs::read_to_string(&p)?;
                return Ok((text, CredSource::File(p)));
            }
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".claude").join(".credentials.json"));
        candidates.push(home.join(".claude").join("credentials.json"));
    }
    for p in candidates {
        if let Ok(text) = std::fs::read_to_string(&p) {
            if !text.trim().is_empty() {
                return Ok((text, CredSource::File(p)));
            }
        }
    }

    #[cfg(windows)]
    {
        for relative_path in [".claude/.credentials.json", ".claude/credentials.json"] {
            if let Some((text, path)) = wsl_read_home_file("Ubuntu", relative_path) {
                return Ok((
                    text,
                    CredSource::Wsl {
                        distro: "Ubuntu".into(),
                        path,
                    },
                ));
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

#[cfg(windows)]
fn wsl_read(distro: &str, path: &str) -> Option<String> {
    for attempt in 0..2 {
        let out = std::process::Command::new("wsl.exe")
            .args(["-d", distro, "--", "cat", path])
            .output()
            .ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
        if attempt == 0 {
            std::thread::sleep(StdDuration::from_millis(250));
        }
    }
    None
}

#[cfg(windows)]
fn wsl_read_home_file(distro: &str, relative_path: &str) -> Option<(String, String)> {
    let shell = format!(
        "p=\"$HOME\"/{}; [ -s \"$p\" ] || exit 1; printf '%s\\n' \"$p\"; cat \"$p\"",
        shell_quote(relative_path)
    );
    for attempt in 0..2 {
        let out = std::process::Command::new("wsl.exe")
            .args(["-d", distro, "--", "bash", "-lc", &shell])
            .output()
            .ok()?;
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            if let Some((path, text)) = stdout.split_once('\n') {
                if !text.trim().is_empty() {
                    return Some((text.to_string(), path.to_string()));
                }
            }
        }
        if attempt == 0 {
            std::thread::sleep(StdDuration::from_millis(250));
        }
    }
    None
}

#[cfg(windows)]
fn wsl_write(distro: &str, path: &str, text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("wsl.exe")
        .args(["-d", distro, "--", "tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn wsl.exe tee")?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow!("no stdin for wsl tee"))?
        .write_all(text.as_bytes())?;

    if !child.wait()?.success() {
        bail!("wsl.exe tee failed writing Claude credentials");
    }
    Ok(())
}
