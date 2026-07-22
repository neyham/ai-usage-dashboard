//! Codex usage fetcher. Reads the bearer token from `~/.codex/auth.json` and
//! queries the ChatGPT/Codex web backend. That endpoint is not a stable public
//! API, so malformed responses fall back to last-known-good cached data.

use super::{send, send_with_one_retry, Resp};
use crate::config::Config;
use crate::models::CodexService;
use crate::util::{clamp_percent, fmt_local, local_label, parse_datetime};
use anyhow::{anyhow, bail, Context};
use chrono::Utc;
use reqwest::Client;
use serde_json::Map;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const RESET_CREDITS_TIMEOUT: Duration = Duration::from_secs(4);
const LONG_WINDOW_THRESHOLD_SECONDS: u64 = 24 * 60 * 60;
type Window<'a> = &'a Map<String, Value>;
type WindowPair<'a> = (Option<Window<'a>>, Option<Window<'a>>);

struct CodexAuth {
    token: String,
    account_id: Option<String>,
}

struct ResetCreditSummary {
    available: u64,
    earliest_expiry_local: Option<String>,
}

pub async fn fetch(config: &Config, client: &Client) -> anyhow::Result<CodexService> {
    let auth = read_auth(config)?;

    let resp: Resp = send_with_one_retry(|| authenticated_get(client, USAGE_URL, &auth)).await?;

    if !resp.is_success() {
        bail!("Codex usage HTTP {}", resp.status);
    }
    let mut service = parse_usage(&resp.body)?;

    let request = authenticated_get(client, RESET_CREDITS_URL, &auth)
        .timeout(RESET_CREDITS_TIMEOUT)
        .header("Accept", "application/json")
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "ai-usage-dashboard");
    let reset_summary = match send(request).await {
        Ok(resp) if resp.is_success() => parse_reset_credits(&resp.body).ok(),
        _ => None,
    };
    if let Some(summary) = reset_summary {
        service.reset_credits_available = Some(summary.available);
        service.reset_credits_expire_local = summary.earliest_expiry_local;
    }

    Ok(service)
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
        reset_credits_available: reset_credit_count(&root),
        reset_credits_expire_local: None,
    })
}

fn reset_credit_count(root: &Value) -> Option<u64> {
    root.get("rate_limit_reset_credits")?
        .get("available_count")?
        .as_u64()
}

fn parse_reset_credits(body: &str) -> anyhow::Result<ResetCreditSummary> {
    let root: Value = serde_json::from_str(body).context("parse Codex reset credits body")?;
    let available = root
        .get("available_count")
        .and_then(Value::as_u64)
        .context("Codex reset credits missing available_count")?;
    let now = Utc::now();
    let earliest_expiry_local = root
        .get("credits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|credit| credit.get("status").and_then(Value::as_str) == Some("available"))
        .filter_map(|credit| credit.get("expires_at").and_then(parse_datetime))
        .filter(|expiry| *expiry > now)
        .min()
        .map(fmt_local);

    Ok(ResetCreditSummary {
        available,
        earliest_expiry_local,
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

fn authenticated_get(client: &Client, url: &str, auth: &CodexAuth) -> reqwest::RequestBuilder {
    let request = client
        .get(url)
        .header("Authorization", format!("Bearer {}", auth.token));
    match &auth.account_id {
        Some(account_id) => request.header("ChatGPT-Account-Id", account_id),
        None => request,
    }
}

fn read_auth(config: &Config) -> anyhow::Result<CodexAuth> {
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
    parse_auth(&text)
}

fn parse_auth(text: &str) -> anyhow::Result<CodexAuth> {
    let root: Value = serde_json::from_str(text).context("parse Codex auth.json")?;
    let tokens = &root["tokens"];
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    for key in ["access_token", "id_token"] {
        if let Some(t) = tokens.get(key).and_then(Value::as_str) {
            if !t.is_empty() {
                return Ok(CodexAuth {
                    token: t.to_string(),
                    account_id,
                });
            }
        }
    }
    bail!("Codex token missing in auth.json")
}

#[cfg(test)]
mod tests {
    use super::{parse_auth, parse_reset_credits, parse_usage};
    use crate::util::local_label;
    use serde_json::json;

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
            },
            "rate_limit_reset_credits": { "available_count": 4 }
        }"#;

        let usage = parse_usage(body).expect("valid Codex usage");

        assert_eq!(usage.five_hour_percent, Some(1.0));
        assert_eq!(usage.seven_day_percent, Some(38.0));
        assert_eq!(usage.reset_credits_available, Some(4));
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

    #[test]
    fn codex_reset_credits_keep_count_and_earliest_available_expiry() {
        let body = r#"{
            "available_count": 3,
            "credits": [
                {"status":"available","expires_at":"2099-08-03T09:00:00Z"},
                {"status":"redeemed","expires_at":"2099-07-01T09:00:00Z"},
                {"status":"available","expires_at":null},
                {"status":"available","expires_at":"2099-08-01T09:00:00Z"}
            ]
        }"#;

        let summary = parse_reset_credits(body).expect("valid reset-credit inventory");

        assert_eq!(summary.available, 3);
        assert_eq!(
            summary.earliest_expiry_local,
            local_label(&json!("2099-08-01T09:00:00Z"))
        );
    }

    #[test]
    fn codex_auth_keeps_account_scope_without_exposing_it_to_the_service() {
        let auth = parse_auth(r#"{"tokens":{"access_token":"secret","account_id":"account-123"}}"#)
            .expect("valid auth");

        assert_eq!(auth.token, "secret");
        assert_eq!(auth.account_id.as_deref(), Some("account-123"));
    }
}
