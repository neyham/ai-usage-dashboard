//! App config from %APPDATA%\AiUsageDashboard\config.json (Roaming on Windows,
//! XDG config dir elsewhere). Mirrors the WinForms prototype's config schema.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub refresh_interval_minutes: u64,
    pub network_timeout_seconds: u64,
    /// Optional plaintext fallback for the DeepSeek key (lowest priority).
    pub deep_seek_api_key: String,
    /// Credential Manager target name (Windows) / keyring service tag.
    pub deep_seek_credential_target: String,
    /// Optional override path / "wsl:<distro>:<path>" spec for Claude creds.
    pub claude_credentials_path: String,
    /// Optional override path for Codex auth.json.
    pub codex_auth_path: String,
    /// "" for live; "normal" | "claude429" | "failures" for mock mode.
    pub mock_mode: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_interval_minutes: 30,
            network_timeout_seconds: 15,
            deep_seek_api_key: String::new(),
            deep_seek_credential_target: "AiUsageDashboard/DeepSeekApiKey".into(),
            claude_credentials_path: String::new(),
            codex_auth_path: String::new(),
            mock_mode: String::new(),
        }
    }
}

impl Config {
    /// Clamp to the same minimums the WinForms prototype enforced.
    fn clamp(mut self) -> Self {
        if self.refresh_interval_minutes < 15 {
            self.refresh_interval_minutes = 15;
        }
        if self.network_timeout_seconds < 5 {
            self.network_timeout_seconds = 15;
        }
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
  "refreshIntervalMinutes": 30,
  "networkTimeoutSeconds": 15,
  "deepSeekApiKey": "",
  "deepSeekCredentialTarget": "AiUsageDashboard/DeepSeekApiKey",
  "claudeCredentialsPath": "",
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
        let _ = std::fs::write(&path, DEFAULT_CONFIG_JSON);
        return Config::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Config>(&text)
            .unwrap_or_default()
            .clamp(),
        Err(_) => Config::default(),
    }
}
