//! Mock mode — produces deterministic summaries from the bundled mock payloads
//! so the three required states can be verified without network access:
//!   "normal"    — all services healthy
//!   "claude429" — Claude rate-limited, showing cached data + cooldown
//!   "failures"  — Codex + DeepSeek failed, showing cached data; UI stays up

use crate::fetchers::{claude, codex, deepseek};
use crate::models::{EnabledProviders, Services, UsageSummary};
use crate::util::fmt_local;
use chrono::{Duration, Utc};

const CLAUDE_JSON: &str = include_str!("../../mocks/claude_normal.json");
const CODEX_JSON: &str = include_str!("../../mocks/codex_normal.json");
const DEEPSEEK_JSON: &str = include_str!("../../mocks/deepseek_normal.json");

pub fn summary(mode: &str, enabled: EnabledProviders) -> Option<UsageSummary> {
    let mode = mode.trim().to_lowercase();
    if mode.is_empty() {
        return None;
    }
    if !matches!(mode.as_str(), "normal" | "claude429" | "failures") {
        return Some(invalid_mode_summary(enabled));
    }

    Some(summary_from_payloads(
        &mode,
        CLAUDE_JSON,
        CODEX_JSON,
        DEEPSEEK_JSON,
        enabled,
    ))
}

fn summary_from_payloads(
    mode: &str,
    claude_json: &str,
    codex_json: &str,
    deepseek_json: &str,
    enabled: EnabledProviders,
) -> UsageSummary {
    let Some((mut claude, mut codex, mut deepseek)) = claude::parse_usage(claude_json)
        .ok()
        .zip(codex::parse_usage(codex_json).ok())
        .zip(deepseek::parse_balance(deepseek_json).ok())
        .map(|((claude, codex), deepseek)| (claude, codex, deepseek))
    else {
        return mock_data_error_summary(enabled);
    };
    if codex.reset_credits_available.unwrap_or(0) > 0 {
        codex.reset_credits_expire_local = Some(fmt_local(Utc::now() + Duration::days(21)));
    }

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
        _ => return invalid_mode_summary(enabled),
    }

    crate::fetchers::assemble(
        claude,
        codex,
        deepseek,
        mode == "claude429",
        mode == "failures",
        mode == "failures",
        enabled,
    )
}

fn invalid_mode_summary(enabled: EnabledProviders) -> UsageSummary {
    error_summary("INVALID MOCK MODE", enabled)
}

fn mock_data_error_summary(enabled: EnabledProviders) -> UsageSummary {
    error_summary("MOCK DATA ERROR", enabled)
}

fn error_summary(message: &str, enabled: EnabledProviders) -> UsageSummary {
    let mut services = Services::default();
    services.codex.status = message.into();
    services.claude.status = message.into();
    services.deepseek.status = message.into();
    services.codex.data_may_be_stale = true;
    services.claude.data_may_be_stale = true;
    services.deepseek.data_may_be_stale = true;

    UsageSummary {
        refreshed_at: None,
        status: if enabled.count() == 0 {
            "idle".into()
        } else {
            "error".into()
        },
        enabled_providers: enabled,
        services,
    }
}

#[cfg(test)]
mod tests {
    use super::{summary, summary_from_payloads, CODEX_JSON, DEEPSEEK_JSON};
    use crate::models::EnabledProviders;

    #[test]
    fn empty_mock_mode_uses_live_mode() {
        assert!(summary("", EnabledProviders::default()).is_none());
    }

    #[test]
    fn unknown_mock_mode_is_visible_and_does_not_use_live_mode() {
        let summary = summary("normla", EnabledProviders::default()).expect("invalid mode summary");

        assert_eq!(summary.status, "error");
        assert_eq!(summary.services.claude.status, "INVALID MOCK MODE");
    }

    #[test]
    fn normal_mock_uses_production_parsers() {
        let summary = summary("normal", EnabledProviders::default()).expect("known mock mode");

        assert_eq!(summary.status, "ok");
        assert_eq!(summary.services.claude.five_hour_percent, Some(42.0));
        assert_eq!(summary.services.claude.seven_day_percent, Some(73.0));
        assert_eq!(summary.services.codex.five_hour_percent, None);
        assert_eq!(summary.services.codex.seven_day_percent, Some(64.0));
        assert_eq!(summary.services.codex.reset_credits_available, Some(3));
        assert!(summary.services.codex.reset_credits_expire_local.is_some());
    }

    #[test]
    fn malformed_mock_payload_is_visible_and_never_selects_live_mode() {
        let summary = summary_from_payloads(
            "normal",
            "{}",
            CODEX_JSON,
            DEEPSEEK_JSON,
            EnabledProviders::default(),
        );

        assert_eq!(summary.status, "error");
        assert_eq!(summary.services.codex.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.claude.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.deepseek.status, "MOCK DATA ERROR");

        assert!(super::summary("", EnabledProviders::default()).is_none());
        assert!(super::summary("normal", EnabledProviders::default()).is_some());
        assert!(super::summary("unknown", EnabledProviders::default()).is_some());
    }

    #[test]
    fn disabled_mock_failure_is_excluded_from_aggregate_status() {
        let summary = summary(
            "claude429",
            EnabledProviders {
                codex: true,
                claude: false,
                deepseek: true,
            },
        )
        .expect("known mock mode");

        assert_eq!(summary.status, "ok");
    }
}
