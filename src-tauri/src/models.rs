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
}

impl Default for EnabledProviders {
    fn default() -> Self {
        Self {
            codex: true,
            claude: true,
            deepseek: true,
            grok: false,
        }
    }
}

impl EnabledProviders {
    pub fn count(self) -> usize {
        [self.codex, self.claude, self.deepseek, self.grok]
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
    pub five_hour_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub five_hour_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seven_day_reset_local: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extra_usage_percent: Option<f64>,
}

impl Default for ClaudeService {
    fn default() -> Self {
        Self {
            status: "AWAITING DATA".into(),
            from_cache: false,
            data_may_be_stale: false,
            cooldown_until_local: None,
            five_hour_percent: None,
            seven_day_percent: None,
            five_hour_reset_local: None,
            seven_day_reset_local: None,
            extra_usage_percent: None,
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
