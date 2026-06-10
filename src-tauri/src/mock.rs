//! Mock mode — produces deterministic summaries from the bundled mock payloads
//! so the three required states can be verified without network access:
//!   "normal"    — all services healthy
//!   "claude429" — Claude rate-limited, showing cached data + cooldown
//!   "failures"  — Codex + DeepSeek failed, showing cached data; UI stays up

use crate::models::{ClaudeService, CodexService, DeepSeekService, Services, UsageSummary};
use crate::util::{clamp_percent, fmt_local, local_label, normalize_percent};
use chrono::{Duration, Utc};
use serde_json::Value;

const CLAUDE_JSON: &str = include_str!("../../mocks/claude_normal.json");
const CODEX_JSON: &str = include_str!("../../mocks/codex_normal.json");
const DEEPSEEK_JSON: &str = include_str!("../../mocks/deepseek_normal.json");

pub fn summary(mode: &str) -> UsageSummary {
    let mode = mode.trim().to_lowercase();

    let mut claude = parse_claude();
    let mut codex = parse_codex();
    let mut deepseek = parse_deepseek();

    match mode.as_str() {
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
        _ => {}
    }

    let status = if mode == "normal" || mode.is_empty() {
        "ok"
    } else {
        "partial"
    };

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

fn parse_claude() -> ClaudeService {
    let v: Value = serde_json::from_str(CLAUDE_JSON).unwrap_or(Value::Null);
    let five = &v["five_hour"];
    let seven = &v["seven_day"];
    ClaudeService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        cooldown_until_local: None,
        five_hour_percent: five.get("utilization").and_then(Value::as_f64).map(normalize_percent),
        seven_day_percent: seven.get("utilization").and_then(Value::as_f64).map(normalize_percent),
        five_hour_reset_local: five.get("resets_at").and_then(local_label),
        seven_day_reset_local: seven.get("resets_at").and_then(local_label),
    }
}

fn parse_codex() -> CodexService {
    let v: Value = serde_json::from_str(CODEX_JSON).unwrap_or(Value::Null);
    let primary = &v["rate_limit"]["primary_window"];
    let secondary = &v["rate_limit"]["secondary_window"];
    CodexService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        plan: v.get("plan_type").and_then(Value::as_str).map(str::to_string),
        five_hour_percent: primary.get("used_percent").and_then(Value::as_f64).map(clamp_percent),
        seven_day_percent: secondary.get("used_percent").and_then(Value::as_f64).map(clamp_percent),
        five_hour_reset_local: primary.get("reset_at").and_then(local_label),
        seven_day_reset_local: secondary.get("reset_at").and_then(local_label),
    }
}

fn parse_deepseek() -> DeepSeekService {
    let v: Value = serde_json::from_str(DEEPSEEK_JSON).unwrap_or(Value::Null);
    let infos = v.get("balance_infos").and_then(Value::as_array).cloned().unwrap_or_default();
    let selected = infos
        .iter()
        .find(|i| {
            i.get("currency")
                .and_then(Value::as_str)
                .map(|c| c.eq_ignore_ascii_case("CNY"))
                .unwrap_or(false)
        })
        .or_else(|| infos.first());

    let (currency, balance) = match selected {
        Some(info) => (
            info.get("currency").and_then(Value::as_str).map(str::to_string),
            info.get("total_balance").and_then(Value::as_str).map(str::to_string),
        ),
        None => (None, None),
    };

    DeepSeekService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        currency,
        balance,
    }
}
