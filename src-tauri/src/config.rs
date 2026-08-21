//! App config from %APPDATA%\AiUsageDashboard\config.json (Roaming on Windows,
//! XDG config dir elsewhere). Mirrors the WinForms prototype's config schema.

use crate::fs_util::{atomic_write, atomic_write_private, restrict_file_to_owner};
use crate::models::EnabledProviders;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const DEFAULT_REFRESH_INTERVAL_MINUTES: u64 = 5;
pub const MIN_REFRESH_INTERVAL_MINUTES: u64 = 5;
pub const MAX_REFRESH_INTERVAL_MINUTES: u64 = 24 * 60;
pub const DEFAULT_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS: u64 = 30;
pub const MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS: u64 = 5;
pub const MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD: f64 = 0.03;
pub const MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD: f64 = 0.001;
pub const MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD: f64 = 0.10;

// Intentionally no `Debug`: this type can hold a plaintext DeepSeek key.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Set when config.json exists but cannot be read or parsed. Never serialized.
    #[serde(skip)]
    pub load_error: bool,
    pub refresh_interval_minutes: u64,
    pub network_timeout_seconds: u64,
    /// Providers displayed and refreshed by the dashboard.
    pub enabled_providers: EnabledProviders,
    /// Optional plaintext DeepSeek key saved directly from Display Settings.
    pub deep_seek_api_key: String,
    /// Optional override path for Claude credential files.
    pub claude_credentials_path: String,
    /// Optional recovery path for Claude OAuth refresh failures. Disabled by
    /// default because it can spend a tiny amount of Claude Code usage.
    pub claude_code_refresh_enabled: bool,
    pub claude_code_command: String,
    pub claude_code_refresh_timeout_seconds: u64,
    pub claude_code_refresh_max_budget_usd: f64,
    /// Optional override path for Codex auth.json. Empty uses the native home
    /// path. Codex credentials are always read-only.
    pub codex_auth_path: String,
    /// Optional path to Grok Build auth.json. Grok credentials are always
    /// read-only.
    pub grok_credentials_path: String,
    /// Optional path to Cursor CLI `auth.json` or Cursor desktop `state.vscdb`.
    /// Cursor credentials are always read-only.
    pub cursor_credentials_path: String,
    /// Optional path to Antigravity / `agy` OAuth token file. Empty uses the
    /// official file, then the OS keyring item `agy` writes. Antigravity
    /// credentials are always read-only.
    pub antigravity_credentials_path: String,
    /// "" for live; "normal" | "claude429" | "failures" for mock mode.
    pub mock_mode: String,
    /// Preferred window chrome when not launched with CLI override:
    /// "normal" (windowed) | "fullscreen". Screensaver remains CLI-only.
    pub window_mode: String,
    /// Reduce continuous WebView work for low-power devices by disabling
    /// decorative motion/effects and lowering the renderer clock cadence.
    pub low_power_mode: bool,
    /// Visual language: "ring" (circular gauge HUD) or "ip" (IP-as-logo slabs).
    pub art_skin: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            load_error: false,
            refresh_interval_minutes: DEFAULT_REFRESH_INTERVAL_MINUTES,
            network_timeout_seconds: 15,
            enabled_providers: EnabledProviders::default(),
            deep_seek_api_key: String::new(),
            claude_credentials_path: String::new(),
            claude_code_refresh_enabled: false,
            claude_code_command: "claude".into(),
            claude_code_refresh_timeout_seconds: DEFAULT_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
            claude_code_refresh_max_budget_usd: DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
            codex_auth_path: String::new(),
            grok_credentials_path: String::new(),
            cursor_credentials_path: String::new(),
            antigravity_credentials_path: String::new(),
            mock_mode: String::new(),
            window_mode: "normal".into(),
            low_power_mode: false,
            art_skin: "ip".into(),
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
        let mode = self.window_mode.trim().to_ascii_lowercase();
        self.window_mode = if mode == "fullscreen" {
            "fullscreen".into()
        } else {
            "normal".into()
        };
        let skin = self.art_skin.trim().to_ascii_lowercase();
        self.art_skin = if skin == "ring" {
            "ring".into()
        } else {
            "ip".into()
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

pub fn judge_demo_config_path() -> PathBuf {
    config_dir().join("judge-demo.json")
}

const DEFAULT_CONFIG_JSON: &str = r#"{
  "refreshIntervalMinutes": 5,
  "networkTimeoutSeconds": 15,
  "enabledProviders": {
    "codex": true,
    "claude": true,
    "deepseek": true,
    "grok": false,
    "cursor": false,
    "antigravity": false
  },
  "deepSeekApiKey": "",
  "claudeCredentialsPath": "",
  "claudeCodeRefreshEnabled": false,
  "claudeCodeCommand": "claude",
  "claudeCodeRefreshTimeoutSeconds": 30,
  "claudeCodeRefreshMaxBudgetUsd": 0.03,
  "codexAuthPath": "",
  "grokCredentialsPath": "",
  "cursorCredentialsPath": "",
  "antigravityCredentialsPath": "",
  "mockMode": "",
  "windowMode": "normal",
  "lowPowerMode": false,
  "artSkin": "ip"
}
"#;

/// Ensure the config file exists, then load it (falling back to defaults).
pub fn load_or_create() -> Config {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = config_path();
    if !path.exists() {
        return if atomic_write_private(&path, DEFAULT_CONFIG_JSON.as_bytes()).is_ok() {
            Config::default()
        } else {
            Config::with_load_error()
        };
    }
    if restrict_file_to_owner(&path).is_err() {
        return Config::with_load_error();
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_config_text(&text),
        Err(_) => Config::with_load_error(),
    }
}

/// Parse config JSON text, tolerating a leading UTF-8 BOM (common on Windows editors).
pub(crate) fn parse_config_text(text: &str) -> Config {
    serde_json::from_str::<Config>(strip_utf8_bom(text))
        .map(Config::clamp)
        .unwrap_or_else(|_| Config::with_load_error())
}

pub(crate) fn strip_utf8_bom(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

pub fn save(config: &Config) -> io::Result<()> {
    let mut text = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    text.push(b'\n');
    atomic_write_private(&config_path(), &text)
}

pub fn load_judge_demo_selection() -> EnabledProviders {
    std::fs::read_to_string(judge_demo_config_path())
        .ok()
        .and_then(|text| serde_json::from_str(strip_utf8_bom(&text)).ok())
        .unwrap_or_default()
}

pub fn save_judge_demo_selection(enabled: EnabledProviders) -> io::Result<()> {
    save_judge_demo_selection_to(&judge_demo_config_path(), enabled)
}

fn save_judge_demo_selection_to(path: &Path, enabled: EnabledProviders) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_vec_pretty(&enabled).map_err(io::Error::other)?;
    text.push(b'\n');
    atomic_write(path, &text)
}

#[cfg(test)]
mod tests {
    use super::{
        save_judge_demo_selection_to, Config, DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
        DEFAULT_REFRESH_INTERVAL_MINUTES, MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
        MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS, MAX_REFRESH_INTERVAL_MINUTES,
        MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD, MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
        MIN_REFRESH_INTERVAL_MINUTES,
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
        assert!(config.enabled_providers.codex);
        assert!(config.enabled_providers.claude);
        assert!(config.enabled_providers.deepseek);
        assert!(!config.enabled_providers.grok);
        assert!(!config.enabled_providers.cursor);
        assert!(!config.enabled_providers.antigravity);
        assert_eq!(
            config.refresh_interval_minutes,
            DEFAULT_REFRESH_INTERVAL_MINUTES
        );
    }

    #[test]
    fn utf8_bom_prefixed_config_parses_without_load_error() {
        let config = super::parse_config_text("\u{feff}{\"mockMode\":\"normal\"}\n");
        assert!(!config.load_error);
        assert_eq!(config.mock_mode, "normal");
    }

    #[test]
    fn provider_selection_accepts_partial_nested_config() {
        let config: Config =
            serde_json::from_str(r#"{"enabledProviders":{"claude":false,"deepseek":false}}"#)
                .expect("valid provider selection");

        assert!(config.enabled_providers.codex);
        assert!(!config.enabled_providers.claude);
        assert!(!config.enabled_providers.deepseek);
        assert!(!config.enabled_providers.grok);
        assert!(!config.enabled_providers.cursor);
        assert!(!config.enabled_providers.antigravity);
    }

    #[test]
    fn standalone_judge_selection_uses_safe_defaults() {
        let selection: crate::models::EnabledProviders =
            serde_json::from_str(r#"{"claude":false}"#).expect("valid demo selection");

        assert!(selection.codex);
        assert!(!selection.claude);
        assert!(selection.deepseek);
        assert!(!selection.grok);
        assert!(!selection.cursor);
        assert!(!selection.antigravity);
    }

    #[test]
    fn judge_selection_save_creates_its_isolated_directory() {
        let unique = format!(
            "ai-usage-dashboard-judge-demo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let path = root.join("nested").join("judge-demo.json");
        let selection = crate::models::EnabledProviders {
            codex: true,
            claude: false,
            deepseek: true,
            grok: true,
            cursor: false,
            antigravity: false,
        };

        save_judge_demo_selection_to(&path, selection).expect("save demo selection");
        let saved: crate::models::EnabledProviders =
            serde_json::from_slice(&std::fs::read(&path).expect("read saved demo selection"))
                .expect("parse saved demo selection");

        assert_eq!(saved, selection);
        std::fs::remove_dir_all(root).expect("remove isolated test directory");
    }

    #[test]
    fn window_mode_defaults_to_normal_and_clamps_unknown_values() {
        let defaulted: Config =
            serde_json::from_str(r#"{}"#).expect("empty object uses field defaults");
        assert_eq!(defaulted.window_mode, "normal");

        let full: Config =
            serde_json::from_str(r#"{"windowMode":"FULLSCREEN"}"#).expect("fullscreen mode");
        assert_eq!(full.clamp().window_mode, "fullscreen");

        let junk: Config = serde_json::from_str(r#"{"windowMode":"kiosk"}"#).expect("unknown mode");
        assert_eq!(junk.clamp().window_mode, "normal");
    }

    #[test]
    fn low_power_mode_defaults_off_and_accepts_opt_in() {
        let defaulted: Config =
            serde_json::from_str(r#"{}"#).expect("empty object uses field defaults");
        let enabled: Config =
            serde_json::from_str(r#"{"lowPowerMode":true}"#).expect("low-power mode opt-in");

        assert!(!defaulted.low_power_mode);
        assert!(enabled.low_power_mode);
    }

    #[test]
    fn art_skin_defaults_to_ip_and_clamps_unknown_values() {
        let defaulted: Config =
            serde_json::from_str(r#"{}"#).expect("empty object uses field defaults");
        assert_eq!(defaulted.art_skin, "ip");

        let ring: Config = serde_json::from_str(r#"{"artSkin":"RING"}"#).expect("ring skin");
        assert_eq!(ring.clamp().art_skin, "ring");

        let junk: Config = serde_json::from_str(r#"{"artSkin":"neon"}"#).expect("unknown skin");
        assert_eq!(junk.clamp().art_skin, "ip");
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
