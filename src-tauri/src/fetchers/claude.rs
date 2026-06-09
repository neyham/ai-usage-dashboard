//! Claude usage fetcher. Reads OAuth credentials from the local
//! `.claude/.credentials.json` (or, on Windows, falls back to reading the WSL
//! root credentials via `wsl.exe`). Refreshes on expiry / 401 and signals 429
//! back to the orchestrator so it can enter cooldown.

use super::{send, FetchError};
use crate::config::Config;
use crate::models::ClaudeService;
use crate::util::{local_label, normalize_percent, parse_datetime};
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde_json::{json, Value};
use std::path::PathBuf;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_URL: &str = "https://claude.ai/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";

pub async fn fetch(config: &Config, client: &Client) -> Result<ClaudeService, FetchError> {
    let mut creds = load(config).map_err(FetchError::Other)?;

    if creds.is_expired_soon() {
        creds.refresh(client).await.map_err(FetchError::Other)?;
    }

    let mut resp = send(usage_request(client, &creds.access_token))
        .await
        .map_err(FetchError::Other)?;

    if resp.status == 401 {
        creds.refresh(client).await.map_err(FetchError::Other)?;
        resp = send(usage_request(client, &creds.access_token))
            .await
            .map_err(FetchError::Other)?;
    }

    if resp.status == 429 {
        return Err(FetchError::RateLimited {
            retry_after: resp.retry_after,
        });
    }
    if !resp.is_success() {
        return Err(FetchError::Other(anyhow!(
            "Claude usage HTTP {}",
            resp.status
        )));
    }

    parse_usage(&resp.body).map_err(FetchError::Other)
}

fn usage_request(client: &Client, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", ANTHROPIC_BETA)
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

        let req = client
            .post(TOKEN_URL)
            .header("Origin", "https://claude.ai")
            .header("Referer", "https://claude.ai/")
            .json(&json!({
                "grant_type": "refresh_token",
                "refresh_token": self.refresh_token,
                "client_id": CLIENT_ID,
            }));

        let resp = send(req).await?;
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
    #[cfg(windows)]
    {
        candidates.push(PathBuf::from(r"\\wsl.localhost\Ubuntu\root\.claude\.credentials.json"));
        candidates.push(PathBuf::from(r"\\wsl$\Ubuntu\root\.claude\.credentials.json"));
    }
    #[cfg(not(windows))]
    {
        candidates.push(PathBuf::from("/root/.claude/.credentials.json"));
        candidates.push(PathBuf::from("/root/.claude/credentials.json"));
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
        for path in [
            "/root/.claude/.credentials.json",
            "/root/.claude/credentials.json",
        ] {
            if let Some(text) = wsl_read("Ubuntu", path) {
                return Ok((
                    text,
                    CredSource::Wsl {
                        distro: "Ubuntu".into(),
                        path: path.into(),
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
