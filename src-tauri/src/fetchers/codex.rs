//! Codex usage fetcher. Reads the bearer token from `~/.codex/auth.json` and
//! queries the ChatGPT/Codex web backend. That endpoint is not a stable public
//! API, so parsing tolerates missing fields and the caller tolerates failure.

use super::{send_with_one_retry, Resp};
use crate::config::Config;
use crate::models::CodexService;
use crate::util::{local_label, normalize_percent};
use anyhow::{anyhow, bail, Context};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

pub async fn fetch(config: &Config, client: &Client) -> anyhow::Result<CodexService> {
    let token = read_token(config)?;

    let resp: Resp = send_with_one_retry(|| {
        client
            .get(USAGE_URL)
            .header("Authorization", format!("Bearer {token}"))
    })
    .await?;

    if !resp.is_success() {
        bail!("Codex usage HTTP {}", resp.status);
    }
    parse_usage(&resp.body)
}

fn parse_usage(body: &str) -> anyhow::Result<CodexService> {
    let root: Value = serde_json::from_str(body).context("parse Codex usage body")?;
    let rate = &root["rate_limit"];
    let primary = &rate["primary_window"];
    let secondary = &rate["secondary_window"];

    Ok(CodexService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        plan: root.get("plan_type").and_then(Value::as_str).map(str::to_string),
        five_hour_percent: primary.get("used_percent").and_then(Value::as_f64).map(normalize_percent),
        seven_day_percent: secondary.get("used_percent").and_then(Value::as_f64).map(normalize_percent),
        five_hour_reset_local: primary.get("reset_at").and_then(local_label),
        seven_day_reset_local: secondary.get("reset_at").and_then(local_label),
    })
}

fn read_token(config: &Config) -> anyhow::Result<String> {
    let path = if config.codex_auth_path.trim().is_empty() {
        dirs::home_dir()
            .ok_or_else(|| anyhow!("no home dir"))?
            .join(".codex")
            .join("auth.json")
    } else {
        PathBuf::from(config.codex_auth_path.trim())
    };

    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read Codex auth.json at {}", path.display()))?;
    let root: Value = serde_json::from_str(&text).context("parse Codex auth.json")?;
    let tokens = &root["tokens"];

    for key in ["access_token", "id_token"] {
        if let Some(t) = tokens.get(key).and_then(Value::as_str) {
            if !t.is_empty() {
                return Ok(t.to_string());
            }
        }
    }
    bail!("Codex token missing in auth.json")
}
