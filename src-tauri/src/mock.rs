//! Mock mode — produces deterministic summaries from the bundled mock payloads
//! so the three required states can be verified without network access:
//!   "normal"    — all services healthy
//!   "claude429" — Claude rate-limited, showing cached data + cooldown
//!   "failures"  — Codex + DeepSeek failed, showing cached data; UI stays up

use crate::fetchers::{claude, codex, deepseek};
use crate::models::{Services, UsageSummary};
use crate::util::fmt_local;
use chrono::{Duration, Utc};

const CLAUDE_JSON: &str = include_str!("../../mocks/claude_normal.json");
const CODEX_JSON: &str = include_str!("../../mocks/codex_normal.json");
const DEEPSEEK_JSON: &str = include_str!("../../mocks/deepseek_normal.json");

pub fn summary(mode: &str) -> Option<UsageSummary> {
    let mode = mode.trim().to_lowercase();
    if mode.is_empty() {
        return None;
    }
    if !matches!(mode.as_str(), "normal" | "claude429" | "failures") {
        return Some(invalid_mode_summary());
    }

    Some(summary_from_payloads(
        &mode,
        CLAUDE_JSON,
        CODEX_JSON,
        DEEPSEEK_JSON,
    ))
}

fn summary_from_payloads(
    mode: &str,
    claude_json: &str,
    codex_json: &str,
    deepseek_json: &str,
) -> UsageSummary {
    let Some((mut claude, mut codex, mut deepseek)) = claude::parse_usage(claude_json)
        .ok()
        .zip(codex::parse_usage(codex_json).ok())
        .zip(deepseek::parse_balance(deepseek_json).ok())
        .map(|((claude, codex), deepseek)| (claude, codex, deepseek))
    else {
        return mock_data_error_summary();
    };

    match mode {
        "claude429" => {
            claude.from_cache = true;
            claude.status = "RATE LIMITED".into();
            claude.cooldown_until_local = Some(fmt_local(Utc::now() + Duration::minutes(31)));
        }
        "failures" => {
            codex.from_cache = true;
            codex.status = "API ERROR".into();
            deepseek.from_cache = true;
            deepseek.status = "API ERROR".into();
        }
        "normal" => {}
        _ => return invalid_mode_summary(),
    }

    let status = if mode == "normal" { "ok" } else { "partial" };

    UsageSummary {
        refreshed_at: Some(Utc::now().to_rfc3339()),
        status: status.into(),
        services: Services {
            codex,
            claude,
            deepseek,
        },
    }
}

fn invalid_mode_summary() -> UsageSummary {
    error_summary("INVALID MOCK MODE")
}

fn mock_data_error_summary() -> UsageSummary {
    error_summary("MOCK DATA ERROR")
}

fn error_summary(message: &str) -> UsageSummary {
    let mut services = Services::default();
    services.codex.status = message.into();
    services.claude.status = message.into();
    services.deepseek.status = message.into();
    services.codex.data_may_be_stale = true;
    services.claude.data_may_be_stale = true;
    services.deepseek.data_may_be_stale = true;

    UsageSummary {
        refreshed_at: None,
        status: "error".into(),
        services,
    }
}

#[cfg(test)]
mod tests {
    use super::{summary, summary_from_payloads, CODEX_JSON, DEEPSEEK_JSON};

    #[test]
    fn empty_mock_mode_uses_live_mode() {
        assert!(summary("").is_none());
    }

    #[test]
    fn unknown_mock_mode_is_visible_and_does_not_use_live_mode() {
        let summary = summary("normla").expect("invalid mode summary");

        assert_eq!(summary.status, "error");
        assert_eq!(summary.services.claude.status, "INVALID MOCK MODE");
    }

    #[test]
    fn normal_mock_uses_production_parsers() {
        let summary = summary("normal").expect("known mock mode");

        assert_eq!(summary.status, "ok");
        assert_eq!(summary.services.claude.five_hour_percent, Some(42.0));
        assert_eq!(summary.services.claude.seven_day_percent, Some(73.0));
    }

    #[test]
    fn malformed_mock_payload_is_visible_and_never_selects_live_mode() {
        let summary = summary_from_payloads("normal", "{}", CODEX_JSON, DEEPSEEK_JSON);

        assert_eq!(summary.status, "error");
        assert_eq!(summary.services.codex.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.claude.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.deepseek.status, "MOCK DATA ERROR");

        assert!(super::summary("").is_none());
        assert!(super::summary("normal").is_some());
        assert!(super::summary("unknown").is_some());
    }
}
