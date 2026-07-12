//! App config from %APPDATA%\AiUsageDashboard\config.json (Roaming on Windows,
//! XDG config dir elsewhere). Mirrors the WinForms prototype's config schema.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_REFRESH_INTERVAL_MINUTES: u64 = 5;
pub const MIN_REFRESH_INTERVAL_MINUTES: u64 = 5;
pub const MAX_REFRESH_INTERVAL_MINUTES: u64 = 24 * 60;
pub const DEFAULT_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS: u64 = 30;
pub const MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS: u64 = 5;
pub const MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD: f64 = 0.03;
pub const MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD: f64 = 0.001;
pub const MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD: f64 = 0.10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Set when config.json exists but cannot be read or parsed. Never serialized.
    #[serde(skip)]
    pub load_error: bool,
    pub refresh_interval_minutes: u64,
    pub network_timeout_seconds: u64,
    /// Optional plaintext fallback for the DeepSeek key (lowest priority).
    pub deep_seek_api_key: String,
    /// Credential Manager target name (Windows) / keyring service tag.
    pub deep_seek_credential_target: String,
    /// Optional override path / "wsl:<distro>:<path>" spec for Claude creds.
    pub claude_credentials_path: String,
    /// Optional recovery path for Claude OAuth refresh failures. Disabled by
    /// default because it can spend a tiny amount of Claude Code usage.
    pub claude_code_refresh_enabled: bool,
    pub claude_code_command: String,
    pub claude_code_refresh_timeout_seconds: u64,
    pub claude_code_refresh_max_budget_usd: f64,
    /// Optional override path for Codex auth.json.
    pub codex_auth_path: String,
    /// "" for live; "normal" | "claude429" | "failures" for mock mode.
    pub mock_mode: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            load_error: false,
            refresh_interval_minutes: DEFAULT_REFRESH_INTERVAL_MINUTES,
            network_timeout_seconds: 15,
            deep_seek_api_key: String::new(),
            deep_seek_credential_target: "AiUsageDashboard/DeepSeekApiKey".into(),
            claude_credentials_path: String::new(),
            claude_code_refresh_enabled: false,
            claude_code_command: "claude".into(),
            claude_code_refresh_timeout_seconds: DEFAULT_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
            claude_code_refresh_max_budget_usd: DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
            codex_auth_path: String::new(),
            mock_mode: String::new(),
        }
    }
}

impl Config {
    fn with_load_error() -> Self {
        Self {
            load_error: true,
            ..Self::default()
        }
    }

    /// Keep user-provided timing values inside operationally safe bounds.
    fn clamp(mut self) -> Self {
        self.refresh_interval_minutes = self
            .refresh_interval_minutes
            .clamp(MIN_REFRESH_INTERVAL_MINUTES, MAX_REFRESH_INTERVAL_MINUTES);
        if self.network_timeout_seconds < 5 {
            self.network_timeout_seconds = 15;
        }
        if self.claude_code_command.trim().is_empty() {
            self.claude_code_command = "claude".into();
        }
        self.claude_code_refresh_timeout_seconds = self.claude_code_refresh_timeout_seconds.clamp(
            MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
            MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
        );
        self.claude_code_refresh_max_budget_usd =
            if self.claude_code_refresh_max_budget_usd.is_finite()
                && self.claude_code_refresh_max_budget_usd > 0.0
            {
                self.claude_code_refresh_max_budget_usd.clamp(
                    MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
                    MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
                )
            } else {
                DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD
            };
        self
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AiUsageDashboard")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

const DEFAULT_CONFIG_JSON: &str = r#"{
  "refreshIntervalMinutes": 5,
  "networkTimeoutSeconds": 15,
  "deepSeekApiKey": "",
  "deepSeekCredentialTarget": "AiUsageDashboard/DeepSeekApiKey",
  "claudeCredentialsPath": "",
  "claudeCodeRefreshEnabled": false,
  "claudeCodeCommand": "claude",
  "claudeCodeRefreshTimeoutSeconds": 30,
  "claudeCodeRefreshMaxBudgetUsd": 0.03,
  "codexAuthPath": "",
  "mockMode": ""
}
"#;

/// Ensure the config file exists, then load it (falling back to defaults).
pub fn load_or_create() -> Config {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = config_path();
    if !path.exists() {
        return if std::fs::write(&path, DEFAULT_CONFIG_JSON).is_ok() {
            Config::default()
        } else {
            Config::with_load_error()
        };
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Config>(text.trim_start_matches('\u{feff}'))
            .map(Config::clamp)
            .unwrap_or_else(|_| Config::with_load_error()),
        Err(_) => Config::with_load_error(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Config, DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD, DEFAULT_REFRESH_INTERVAL_MINUTES,
        MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD, MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
        MAX_REFRESH_INTERVAL_MINUTES, MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
        MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS, MIN_REFRESH_INTERVAL_MINUTES,
    };

    #[test]
    fn invalid_config_is_not_treated_as_live_defaults() {
        let config = serde_json::from_str::<Config>("not json")
            .map(Config::clamp)
            .unwrap_or_else(|_| Config::with_load_error());

        assert!(config.load_error);
    }

    #[test]
    fn valid_config_does_not_set_load_error() {
        let config: Config =
            serde_json::from_str(r#"{"mockMode":"normal"}"#).expect("valid partial config");

        assert!(!config.load_error);
        assert_eq!(config.mock_mode, "normal");
        assert_eq!(
            config.refresh_interval_minutes,
            DEFAULT_REFRESH_INTERVAL_MINUTES
        );
    }

    #[test]
    fn refresh_interval_is_bounded() {
        let too_fast: Config =
            serde_json::from_str(r#"{"refreshIntervalMinutes":1}"#).expect("valid low interval");
        let too_slow: Config =
            serde_json::from_str(&format!(r#"{{"refreshIntervalMinutes":{}}}"#, u64::MAX))
                .expect("valid high interval");

        assert_eq!(
            too_fast.clamp().refresh_interval_minutes,
            MIN_REFRESH_INTERVAL_MINUTES
        );
        assert_eq!(
            too_slow.clamp().refresh_interval_minutes,
            MAX_REFRESH_INTERVAL_MINUTES
        );
    }

    #[test]
    fn claude_code_recovery_cost_and_timeout_are_bounded() {
        let too_low = Config {
            claude_code_refresh_timeout_seconds: 1,
            claude_code_refresh_max_budget_usd: 0.000_01,
            ..Config::default()
        }
        .clamp();
        assert_eq!(
            too_low.claude_code_refresh_timeout_seconds,
            MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS
        );
        assert_eq!(
            too_low.claude_code_refresh_max_budget_usd,
            MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD
        );

        let too_high = Config {
            claude_code_refresh_timeout_seconds: u64::MAX,
            claude_code_refresh_max_budget_usd: 500.0,
            ..Config::default()
        }
        .clamp();
        assert_eq!(
            too_high.claude_code_refresh_timeout_seconds,
            MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS
        );
        assert_eq!(
            too_high.claude_code_refresh_max_budget_usd,
            MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD
        );

        let invalid = Config {
            claude_code_refresh_max_budget_usd: f64::NAN,
            ..Config::default()
        }
        .clamp();
        assert_eq!(
            invalid.claude_code_refresh_max_budget_usd,
            DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD
        );
    }
}
