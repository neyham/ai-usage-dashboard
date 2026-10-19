//! Sanitized DTOs sent to the renderer. These are the ONLY shapes that cross
//! the IPC boundary — no tokens, keys, credential contents, or raw error bodies.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EnabledProviders {
    pub codex: bool,
    pub claude: bool,
    pub deepseek: bool,
    pub grok: bool,
    pub cursor: bool,
    pub antigravity: bool,
    pub opencodego: bool,
}

impl EnabledProviders {
    /// Grok and Cursor share one X Subscription pool, so either panel is
    /// enough reason to read it.
    pub fn x_pool(&self) -> bool {
        self.grok || self.cursor
    }
}

impl Default for EnabledProviders {
    fn default() -> Self {
        Self {
            codex: true,
            claude: true,
            deepseek: true,
            grok: false,
            cursor: false,
            antigravity: false,
            opencodego: false,
        }
    }
}

impl EnabledProviders {
    pub fn count(self) -> usize {
        [
            self.codex,
            self.claude,
            self.deepseek,
            self.grok,
            self.cursor,
            self.antigravity,
            self.opencodego,
        ]
        .into_iter()
        .filter(|enabled| *enabled)
        .count()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeService {
    pub status: String,
    pub from_cache: bool,
    pub data_may_be_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_until_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub five_hour_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub five_hour_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extra_usage_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset_credits_available: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset_credits_expire_local: Option<String>,
}

impl Default for ClaudeService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            cooldown_until_local: None,
            plan: None,
            five_hour_percent: None,
            seven_day_percent: None,
            five_hour_reset_local: None,
            seven_day_reset_local: None,
            extra_usage_percent: None,
            reset_credits_available: None,
            reset_credits_expire_local: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexService {
    pub status: String,
    pub from_cache: bool,
    pub data_may_be_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub five_hour_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub five_hour_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset_credits_available: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset_credits_expire_local: Option<String>,
}

impl Default for CodexService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            plan: None,
            five_hour_percent: None,
            seven_day_percent: None,
            five_hour_reset_local: None,
            seven_day_reset_local: None,
            reset_credits_available: None,
            reset_credits_expire_local: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekService {
    pub status: String,
    pub from_cache: bool,
    pub data_may_be_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub balance: Option<String>,
}

impl Default for DeepSeekService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            currency: None,
            balance: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokService {
    pub status: String,
    pub from_cache: bool,
    pub data_may_be_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_caption: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub monthly_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub monthly_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset_credits_available: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reset_credits_expire_local: Option<String>,
    /// The weekly pool is shared beyond Grok Build (The X Subscription).
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub unified: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub product_usage: Vec<ProductUsage>,
    /// Prepaid Additional Usage credit in USD cents.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prepaid_balance_cents: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductUsage {
    pub label: String,
    pub percent: f64,
}

impl Default for GrokService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            plan: None,
            usage_percent: None,
            period_label: None,
            period_caption: None,
            usage_reset_local: None,
            monthly_percent: None,
            monthly_reset_local: None,
            reset_credits_available: None,
            reset_credits_expire_local: None,
            unified: false,
            product_usage: Vec::new(),
            prepaid_balance_cents: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorService {
    pub status: String,
    pub from_cache: bool,
    pub data_may_be_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub included_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub api_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub on_demand_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub grok_bot_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub grok_bot_reset_local: Option<String>,
}

impl Default for CursorService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            plan: None,
            usage_percent: None,
            usage_reset_local: None,
            included_percent: None,
            api_percent: None,
            on_demand_percent: None,
            grok_bot_percent: None,
            grok_bot_reset_local: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityService {
    pub status: String,
    pub from_cache: bool,
    pub data_may_be_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub five_hour_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub five_hour_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_reset_local: Option<String>,
    /// Antigravity's separate "Claude and GPT models" quota group.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claude_five_hour_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claude_seven_day_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claude_five_hour_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claude_seven_day_reset_local: Option<String>,
    /// Separate weekly Gemini 4 Argon quota on paid Pro plans.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub argon_seven_day_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub argon_seven_day_reset_local: Option<String>,
}

impl Default for AntigravityService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            plan: None,
            five_hour_percent: None,
            seven_day_percent: None,
            five_hour_reset_local: None,
            seven_day_reset_local: None,
            claude_five_hour_percent: None,
            claude_seven_day_percent: None,
            claude_five_hour_reset_local: None,
            claude_seven_day_reset_local: None,
            argon_seven_day_percent: None,
            argon_seven_day_reset_local: None,
        }
    }
}

/// OpenCode Go subscription windows from the Zen Go usage API. The API does
/// not say whether the seat is Go or Go Plus, and limits are per model, so
/// only server-reported percentages are shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeGoService {
    pub status: String,
    pub from_cache: bool,
    pub data_may_be_stale: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rolling_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub weekly_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub monthly_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rolling_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub weekly_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub monthly_reset_local: Option<String>,
}

impl Default for OpenCodeGoService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            rolling_percent: None,
            weekly_percent: None,
            monthly_percent: None,
            rolling_reset_local: None,
            weekly_reset_local: None,
            monthly_reset_local: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Services {
    pub codex: CodexService,
    pub claude: ClaudeService,
    pub deepseek: DeepSeekService,
    pub grok: GrokService,
    pub cursor: CursorService,
    pub antigravity: AntigravityService,
    #[serde(default)]
    pub opencodego: OpenCodeGoService,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub refreshed_at: Option<String>,
    /// "idle" | "ok" | "refreshing" | "partial" | "error"
    pub status: String,
    pub enabled_providers: EnabledProviders,
    pub services: Services,
}

impl UsageSummary {
    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self {
            refreshed_at: None,
            status: "idle".into(),
            enabled_providers: EnabledProviders::default(),
            services: Services::default(),
        }
    }
}
