//! Mock mode — produces deterministic summaries from the bundled mock payloads
//! so required states can be verified without network access:
//!   "normal"    — all services healthy
//!   "claude429" — Claude rate-limited, showing cached data + cooldown
//!   "failures"  — Codex + DeepSeek + Grok failed, showing cached data; UI stays up

use crate::fetchers::{antigravity, claude, codex, cursor, deepseek, grok};
use crate::models::{EnabledProviders, Services, UsageSummary};
use crate::util::fmt_local;
use chrono::{Duration, Utc};

const CLAUDE_JSON: &str = include_str!("../../mocks/claude_normal.json");
const CODEX_JSON: &str = include_str!("../../mocks/codex_normal.json");
const DEEPSEEK_JSON: &str = include_str!("../../mocks/deepseek_normal.json");
const GROK_CREDITS_JSON: &str = include_str!("../../mocks/grok_credits_normal.json");
const GROK_MONTHLY_JSON: &str = include_str!("../../mocks/grok_monthly_normal.json");
const CURSOR_JSON: &str = include_str!("../../mocks/cursor_usage_normal.json");
const ANTIGRAVITY_JSON: &str = include_str!("../../mocks/antigravity_quota_normal.json");

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
        GROK_CREDITS_JSON,
        GROK_MONTHLY_JSON,
        CURSOR_JSON,
        ANTIGRAVITY_JSON,
        enabled,
    ))
}

#[allow(clippy::too_many_arguments)]
fn summary_from_payloads(
    mode: &str,
    claude_json: &str,
    codex_json: &str,
    deepseek_json: &str,
    grok_credits_json: &str,
    grok_monthly_json: &str,
    cursor_json: &str,
    antigravity_json: &str,
    enabled: EnabledProviders,
) -> UsageSummary {
    let Some((mut claude, mut codex, mut deepseek, mut grok, mut cursor, mut antigravity)) =
        claude::parse_usage(claude_json)
            .ok()
            .zip(codex::parse_usage(codex_json).ok())
            .zip(deepseek::parse_balance(deepseek_json).ok())
            .zip(grok::parse_usage(grok_credits_json, grok_monthly_json).ok())
            .zip(cursor::parse_usage(cursor_json).ok())
            .zip(antigravity::parse_usage(antigravity_json).ok())
            .map(
                |(((((claude, codex), deepseek), grok), cursor), antigravity)| {
                    (claude, codex, deepseek, grok, cursor, antigravity)
                },
            )
    else {
        return mock_data_error_summary(enabled);
    };
    if codex.reset_credits_available.unwrap_or(0) > 0 {
        codex.reset_credits_expire_local = Some(fmt_local(Utc::now() + Duration::days(21)));
    }
    grok.plan = Some("SuperGrok Heavy".into());

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
            grok.from_cache = true;
            grok.status = "API ERROR".into();
            cursor.from_cache = true;
            cursor.status = "API ERROR".into();
            antigravity.from_cache = true;
            antigravity.status = "API ERROR".into();
        }
        "normal" => {}
        _ => return invalid_mode_summary(enabled),
    }

    crate::fetchers::assemble(
        Services {
            codex,
            claude,
            deepseek,
            grok,
            cursor,
            antigravity,
        },
        EnabledProviders {
            codex: mode == "failures",
            claude: mode == "claude429",
            deepseek: mode == "failures",
            grok: mode == "failures",
            cursor: mode == "failures",
            antigravity: mode == "failures",
        },
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
    services.grok.status = message.into();
    services.cursor.status = message.into();
    services.antigravity.status = message.into();
    services.codex.data_may_be_stale = true;
    services.claude.data_may_be_stale = true;
    services.deepseek.data_may_be_stale = true;
    services.grok.data_may_be_stale = true;
    services.cursor.data_may_be_stale = true;
    services.antigravity.data_may_be_stale = true;

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
    use super::{
        summary, summary_from_payloads, ANTIGRAVITY_JSON, CODEX_JSON, CURSOR_JSON, DEEPSEEK_JSON,
        GROK_CREDITS_JSON, GROK_MONTHLY_JSON,
    };
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
        assert_eq!(summary.services.codex.five_hour_percent, Some(18.0));
        assert_eq!(summary.services.codex.seven_day_percent, Some(64.0));
        assert_eq!(summary.services.codex.reset_credits_available, Some(3));
        assert!(summary.services.codex.reset_credits_expire_local.is_some());
        assert_eq!(summary.services.grok.usage_percent, Some(36.0));
        assert_eq!(summary.services.grok.period_label.as_deref(), Some("7D"));
        assert_eq!(
            summary.services.grok.plan.as_deref(),
            Some("SuperGrok Heavy")
        );
        assert!((summary.services.grok.monthly_percent.unwrap_or_default() - 28.0).abs() < 1e-9);
        assert_eq!(summary.services.cursor.usage_percent, Some(8.0));
        assert_eq!(summary.services.cursor.included_percent, Some(24.0));
        assert_eq!(summary.services.cursor.api_percent, Some(41.0));
        assert_eq!(summary.services.cursor.plan.as_deref(), Some("Ultra"));
        assert!(
            (summary
                .services
                .antigravity
                .five_hour_percent
                .unwrap_or_default()
                - 27.0)
                .abs()
                < 0.01
        );
        assert!(
            (summary
                .services
                .antigravity
                .seven_day_percent
                .unwrap_or_default()
                - 21.0)
                .abs()
                < 0.01
        );
    }

    #[test]
    fn malformed_mock_payload_is_visible_and_never_selects_live_mode() {
        let summary = summary_from_payloads(
            "normal",
            "{}",
            CODEX_JSON,
            DEEPSEEK_JSON,
            GROK_CREDITS_JSON,
            GROK_MONTHLY_JSON,
            CURSOR_JSON,
            ANTIGRAVITY_JSON,
            EnabledProviders::default(),
        );

        assert_eq!(summary.status, "error");
        assert_eq!(summary.services.codex.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.claude.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.deepseek.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.grok.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.cursor.status, "MOCK DATA ERROR");
        assert_eq!(summary.services.antigravity.status, "MOCK DATA ERROR");

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
                grok: false,
                cursor: false,
                antigravity: false,
            },
        )
        .expect("known mock mode");

        assert_eq!(summary.status, "ok");
    }
}
