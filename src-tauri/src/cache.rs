//! Last-known-good cache in %LOCALAPPDATA%\AiUsageDashboard\state.json.
//! Each service keeps its last successful payload plus a timestamp so the UI
//! can degrade gracefully and mark data older than 6 hours as possibly stale.

use crate::fs_util::atomic_write;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

pub const STALE_AFTER_HOURS: i64 = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedCard {
    pub updated_at: DateTime<Utc>,
    /// The successful service DTO, stored verbatim as JSON.
    pub data: Value,
}

impl CachedCard {
    pub fn is_stale(&self) -> bool {
        Utc::now() - self.updated_at > Duration::hours(STALE_AFTER_HOURS)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CacheState {
    pub updated_at: Option<DateTime<Utc>>,
    pub claude_cooldown_until: Option<DateTime<Utc>>,
    /// Provider-specific Retry-After deadlines, persisted across restarts.
    pub provider_cooldowns: HashMap<String, DateTime<Utc>>,
    pub services: HashMap<String, CachedCard>,
}

pub fn cache_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AiUsageDashboard")
}

pub fn cache_path() -> PathBuf {
    cache_dir().join("state.json")
}

impl CacheState {
    pub fn load() -> Self {
        match std::fs::read_to_string(cache_path()) {
            Ok(text) => parse_cache_text(&text),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let dir = cache_dir();
        std::fs::create_dir_all(&dir)?;
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        atomic_write(&cache_path(), &text)
    }

    pub fn get(&self, key: &str) -> Option<&CachedCard> {
        self.services.get(key)
    }

    /// Store a fresh successful payload for a service.
    pub fn put(&mut self, key: &str, data: Value) {
        self.services.insert(
            key.to_string(),
            CachedCard {
                updated_at: Utc::now(),
                data,
            },
        );
    }

    pub fn cooldown_active(&self) -> Option<DateTime<Utc>> {
        match self.claude_cooldown_until {
            Some(until) if Utc::now() < until => Some(until),
            _ => None,
        }
    }

    pub fn provider_cooldown_active(&self, key: &str) -> Option<DateTime<Utc>> {
        self.provider_cooldowns
            .get(key)
            .filter(|until| Utc::now() < **until)
            .cloned()
    }

    pub fn set_provider_cooldown(&mut self, key: &str, until: DateTime<Utc>) {
        self.provider_cooldowns.insert(key.to_string(), until);
    }

    pub fn clear_provider_cooldown(&mut self, key: &str) {
        self.provider_cooldowns.remove(key);
    }
}

fn parse_cache_text(text: &str) -> CacheState {
    // Windows editors may rewrite state.json with a UTF-8 BOM.
    serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::parse_cache_text;

    #[test]
    fn utf8_bom_prefixed_cache_keeps_state() {
        let cache = parse_cache_text(
            "\u{feff}{\"updatedAt\":\"2099-08-01T00:00:00Z\",\"providerCooldowns\":{\"codex\":\"2099-08-01T01:00:00Z\"}}",
        );

        assert!(cache.updated_at.is_some());
        assert!(cache.provider_cooldowns.contains_key("codex"));
    }
}
