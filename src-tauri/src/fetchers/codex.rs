//! Codex usage fetcher. Reads the bearer token from `~/.codex/auth.json` and
//! queries the ChatGPT/Codex web backend. That endpoint is not a stable public
//! API, so malformed responses fall back to last-known-good cached data.

use super::{send_with_one_retry, Resp};
use crate::config::Config;
use crate::models::CodexService;
use crate::util::{clamp_percent, local_label};
use anyhow::{anyhow, bail, Context};
use reqwest::Client;
use serde_json::Map;
use serde_json::Value;
use std::path::PathBuf;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const LONG_WINDOW_THRESHOLD_SECONDS: u64 = 24 * 60 * 60;
type Window<'a> = &'a Map<String, Value>;
type WindowPair<'a> = (Option<Window<'a>>, Option<Window<'a>>);

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

pub(crate) fn parse_usage(body: &str) -> anyhow::Result<CodexService> {
    let root: Value = serde_json::from_str(body).context("parse Codex usage body")?;
    let rate = root
        .get("rate_limit")
        .and_then(Value::as_object)
        .context("Codex usage missing rate_limit")?;
    let primary = rate
        .get("primary_window")
        .and_then(Value::as_object)
        .context("Codex usage missing primary_window")?;
    let secondary = rate.get("secondary_window").and_then(Value::as_object);
    let (five_hour, seven_day) = classify_windows(primary, secondary);

    Ok(CodexService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        plan: root
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        five_hour_percent: window_percent(five_hour, "five-hour")?,
        seven_day_percent: window_percent(seven_day, "seven-day")?,
        five_hour_reset_local: window_reset(five_hour),
        seven_day_reset_local: window_reset(seven_day),
    })
}

fn classify_windows<'a>(primary: Window<'a>, secondary: Option<Window<'a>>) -> WindowPair<'a> {
    let primary_is_long = is_long_window(primary);
    match secondary {
        Some(secondary) if primary_is_long && !is_long_window(secondary) => {
            (Some(secondary), Some(primary))
        }
        Some(secondary) => (Some(primary), Some(secondary)),
        None if primary_is_long => (None, Some(primary)),
        None => (Some(primary), None),
    }
}

fn is_long_window(window: &Map<String, Value>) -> bool {
    window
        .get("limit_window_seconds")
        .and_then(Value::as_u64)
        .is_some_and(|seconds| seconds >= LONG_WINDOW_THRESHOLD_SECONDS)
}

fn window_percent(window: Option<&Map<String, Value>>, label: &str) -> anyhow::Result<Option<f64>> {
    window
        .map(|window| {
            window
                .get("used_percent")
                .and_then(Value::as_f64)
                .map(clamp_percent)
                .with_context(|| format!("Codex usage missing {label} used_percent"))
        })
        .transpose()
}

fn window_reset(window: Option<&Map<String, Value>>) -> Option<String> {
    window?.get("reset_at").and_then(local_label)
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

#[cfg(test)]
mod tests {
    use super::parse_usage;

    #[test]
    fn codex_used_percent_is_already_percent_scale() {
        let body = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 1,
                    "reset_at": 1780813800
                },
                "secondary_window": {
                    "used_percent": 38,
                    "reset_at": 1781121600
                }
            }
        }"#;

        let usage = parse_usage(body).expect("valid Codex usage");

        assert_eq!(usage.five_hour_percent, Some(1.0));
        assert_eq!(usage.seven_day_percent, Some(38.0));
    }

    #[test]
    fn codex_usage_accepts_a_single_classified_window() {
        let body = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 12,
                    "limit_window_seconds": 604800,
                    "reset_at": 1784502445
                },
                "secondary_window": null
            }
        }"#;

        let usage = parse_usage(body).expect("valid weekly-only Codex usage");

        assert_eq!(usage.five_hour_percent, None);
        assert_eq!(usage.seven_day_percent, Some(12.0));
        assert_eq!(usage.five_hour_reset_local, None);
        assert!(usage.seven_day_reset_local.is_some());
    }

    #[test]
    fn codex_usage_rejects_missing_windows_and_percentages() {
        for body in [
            r#"{}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":1},"secondary_window":{}}}"#,
            r#"{"rate_limit":{"primary_window":{"limit_window_seconds":604800}}}"#,
        ] {
            assert!(parse_usage(body).is_err(), "unexpectedly accepted {body}");
        }
    }
}
