//! HTTP helpers, error types, and the per-cycle orchestration that turns three
//! independent fetches into one sanitized `UsageSummary`.

pub mod claude;
pub mod codex;
pub mod deepseek;

use crate::cache::CacheState;
use crate::config::Config;
use crate::models::{EnabledProviders, Services, UsageSummary};
use crate::util::fmt_local;
use chrono::{DateTime, Duration, Utc};
use reqwest::{Client, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration as StdDuration;

pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125 Safari/537.36";
const MAX_RETRY_AFTER_SECONDS: u64 = 86_400;

/// A flattened HTTP response we actually care about.
pub struct Resp {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<u64>,
}

impl Resp {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Debug)]
pub enum FetchError {
    /// HTTP 429 — carries the parsed `retry-after` seconds if present.
    RateLimited { retry_after: Option<u64> },
    /// Authentication or OAuth refresh failure, sanitized for the UI.
    Auth { message: &'static str },
    #[allow(dead_code)]
    Other(anyhow::Error),
}

impl From<anyhow::Error> for FetchError {
    fn from(e: anyhow::Error) -> Self {
        FetchError::Other(e)
    }
}

pub fn build_client(timeout_seconds: u64) -> anyhow::Result<Client> {
    Ok(Client::builder()
        .timeout(StdDuration::from_secs(timeout_seconds))
        .connect_timeout(StdDuration::from_secs(timeout_seconds))
        .user_agent(BROWSER_UA)
        .build()?)
}

pub async fn send(req: RequestBuilder) -> anyhow::Result<Resp> {
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|value| parse_retry_after_at(value, Utc::now()));
    let body = resp.text().await.unwrap_or_default();
    Ok(Resp {
        status,
        body,
        retry_after,
    })
}

fn parse_retry_after_at(value: &str, now: DateTime<Utc>) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.min(MAX_RETRY_AFTER_SECONDS));
    }

    let retry_at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    let millis = retry_at.signed_duration_since(now).num_milliseconds();
    if millis <= 0 {
        Some(0)
    } else {
        Some((millis as u64).div_ceil(1000).min(MAX_RETRY_AFTER_SECONDS))
    }
}

/// Send once; retry a single time on transport error or 408/5xx, mirroring the
/// WinForms `RequestWithOneRetry`.
pub async fn send_with_one_retry<F>(make: F) -> anyhow::Result<Resp>
where
    F: Fn() -> RequestBuilder,
{
    match send(make()).await {
        Ok(r) if r.status == 408 || r.status >= 500 => {
            tokio::time::sleep(StdDuration::from_millis(500)).await;
            send(make()).await
        }
        Ok(r) => Ok(r),
        Err(_) => {
            tokio::time::sleep(StdDuration::from_millis(500)).await;
            send(make()).await
        }
    }
}

// ---------- Cache fallback construction ----------

/// Build a service DTO from cached data (or a bare status when no cache exists),
/// flipping the `fromCache` / `dataMayBeStale` / `status` flags as needed.
fn cached_service<T: DeserializeOwned + Default>(
    cache: &CacheState,
    key: &str,
    message: &str,
    cooldown_local: Option<String>,
) -> T {
    if let Some(card) = cache.get(key) {
        let stale = card.is_stale();
        let mut v = card.data.clone();
        if let Some(obj) = v.as_object_mut() {
            obj.insert("fromCache".into(), json!(true));
            obj.insert("status".into(), json!(message));
            obj.insert("dataMayBeStale".into(), json!(stale));
            if let Some(c) = cooldown_local.clone() {
                obj.insert("cooldownUntilLocal".into(), json!(c));
            }
        }
        return serde_json::from_value(v).unwrap_or_default();
    }

    // No cache at all: synthesize a minimal object carrying just the status.
    let mut obj = serde_json::Map::new();
    obj.insert("status".into(), json!(message));
    obj.insert("fromCache".into(), json!(false));
    obj.insert("dataMayBeStale".into(), json!(true));
    if let Some(c) = cooldown_local {
        obj.insert("cooldownUntilLocal".into(), json!(c));
    }
    serde_json::from_value(Value::Object(obj)).unwrap_or_default()
}

const MSG_RATE_LIMITED: &str = "RATE LIMITED";
const MSG_FAILED: &str = "API ERROR";
const MSG_CACHED: &str = "LAST KNOWN";

/// Build a summary purely from cached data, used to seed the UI on startup so
/// it never begins blank while the first live refresh is in flight.
pub fn summary_from_cache(cache: &CacheState, enabled: EnabledProviders) -> UsageSummary {
    use crate::models::{ClaudeService, CodexService, DeepSeekService};

    let cooldown_label = enabled
        .claude
        .then(|| cache.cooldown_active())
        .flatten()
        .map(fmt_local);

    let claude: ClaudeService = if enabled.claude && cache.get("claude").is_some() {
        let msg = if cooldown_label.is_some() {
            MSG_RATE_LIMITED
        } else {
            MSG_CACHED
        };
        cached_service(cache, "claude", msg, cooldown_label)
    } else {
        ClaudeService::default()
    };
    let codex: CodexService = if enabled.codex && cache.get("codex").is_some() {
        cached_service(cache, "codex", MSG_CACHED, None)
    } else {
        CodexService::default()
    };
    let deepseek: DeepSeekService = if enabled.deepseek && cache.get("deepseek").is_some() {
        cached_service(cache, "deepseek", MSG_CACHED, None)
    } else {
        DeepSeekService::default()
    };

    let any = (enabled.claude && cache.get("claude").is_some())
        || (enabled.codex && cache.get("codex").is_some())
        || (enabled.deepseek && cache.get("deepseek").is_some());

    UsageSummary {
        refreshed_at: any
            .then_some(cache.updated_at)
            .flatten()
            .map(|t| t.to_rfc3339()),
        status: if any { "partial".into() } else { "idle".into() },
        enabled_providers: enabled,
        services: Services {
            codex,
            claude,
            deepseek,
        },
    }
}

/// Run all three fetches and assemble the summary. Updates `cache` in place
/// (callers should persist it). Never returns an error — failures degrade to
/// cached/empty service cards.
pub async fn collect_summary(config: &Config, cache: &mut CacheState) -> UsageSummary {
    use crate::models::{ClaudeService, CodexService, DeepSeekService};

    let enabled = config.enabled_providers;
    if enabled.count() == 0 {
        return summary_from_cache(cache, enabled);
    }

    let client = match build_client(config.network_timeout_seconds) {
        Ok(c) => c,
        Err(_) => {
            // Without a client we can only show whatever is cached.
            let claude = if enabled.claude {
                cached_service::<ClaudeService>(cache, "claude", MSG_FAILED, None)
            } else {
                ClaudeService::default()
            };
            let codex = if enabled.codex {
                cached_service::<CodexService>(cache, "codex", MSG_FAILED, None)
            } else {
                CodexService::default()
            };
            let deepseek = if enabled.deepseek {
                cached_service::<DeepSeekService>(cache, "deepseek", MSG_FAILED, None)
            } else {
                DeepSeekService::default()
            };
            return assemble(
                claude,
                codex,
                deepseek,
                enabled.claude,
                enabled.codex,
                enabled.deepseek,
                enabled,
            );
        }
    };

    // ----- Claude (honor any active cooldown before hitting the network) -----
    let (claude, claude_cached): (ClaudeService, bool) = if !enabled.claude {
        (ClaudeService::default(), false)
    } else if let Some(until) = cache.cooldown_active() {
        (
            cached_service(cache, "claude", MSG_RATE_LIMITED, Some(fmt_local(until))),
            true,
        )
    } else {
        match claude::fetch(config, &client).await {
            Ok(svc) => {
                cache.claude_cooldown_until = None;
                if let Ok(v) = serde_json::to_value(&svc) {
                    cache.put("claude", v);
                }
                (svc, false)
            }
            Err(FetchError::RateLimited { retry_after }) => {
                let secs = retry_after
                    .map(|s| s.saturating_add(30).min(MAX_RETRY_AFTER_SECONDS))
                    .unwrap_or(1800);
                let until = Utc::now() + Duration::seconds(secs as i64);
                cache.claude_cooldown_until = Some(until);
                (
                    cached_service(cache, "claude", MSG_RATE_LIMITED, Some(fmt_local(until))),
                    true,
                )
            }
            Err(FetchError::Auth { message }) => {
                (cached_service(cache, "claude", message, None), true)
            }
            Err(FetchError::Other(_)) => (cached_service(cache, "claude", MSG_FAILED, None), true),
        }
    };

    // ----- Codex -----
    let (codex, codex_cached): (CodexService, bool) = if enabled.codex {
        match codex::fetch(config, &client).await {
            Ok(svc) => {
                if let Ok(v) = serde_json::to_value(&svc) {
                    cache.put("codex", v);
                }
                (svc, false)
            }
            Err(_) => (cached_service(cache, "codex", MSG_FAILED, None), true),
        }
    } else {
        (CodexService::default(), false)
    };

    // ----- DeepSeek -----
    let (deepseek, deepseek_cached): (DeepSeekService, bool) = if enabled.deepseek {
        match deepseek::fetch(config, &client).await {
            Ok(svc) => {
                if let Ok(v) = serde_json::to_value(&svc) {
                    cache.put("deepseek", v);
                }
                (svc, false)
            }
            Err(_) => (cached_service(cache, "deepseek", MSG_FAILED, None), true),
        }
    } else {
        (DeepSeekService::default(), false)
    };

    cache.updated_at = Some(Utc::now());
    assemble(
        claude,
        codex,
        deepseek,
        claude_cached,
        codex_cached,
        deepseek_cached,
        enabled,
    )
}

pub(crate) fn assemble(
    claude: crate::models::ClaudeService,
    codex: crate::models::CodexService,
    deepseek: crate::models::DeepSeekService,
    claude_cached: bool,
    codex_cached: bool,
    deepseek_cached: bool,
    enabled: EnabledProviders,
) -> UsageSummary {
    let active = [
        (enabled.claude, &claude.status, claude_cached),
        (enabled.codex, &codex.status, codex_cached),
        (enabled.deepseek, &deepseek.status, deepseek_cached),
    ]
    .into_iter()
    .filter(|(is_enabled, _, _)| *is_enabled)
    .collect::<Vec<_>>();
    let any_cache = active.iter().any(|(_, _, cached)| *cached);
    let all_cache = active.iter().all(|(_, _, cached)| *cached);
    let unhealthy_services = active
        .iter()
        .filter(|(_, status, _)| status.as_str() != "NOMINAL")
        .count();
    let status = if active.is_empty() {
        "idle"
    } else if all_cache || unhealthy_services == active.len() {
        "error"
    } else if any_cache || unhealthy_services > 0 {
        "partial"
    } else {
        "ok"
    };

    UsageSummary {
        refreshed_at: Some(Utc::now().to_rfc3339()),
        status: status.into(),
        enabled_providers: enabled,
        services: Services {
            codex,
            claude,
            deepseek,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{assemble, parse_retry_after_at};
    use crate::cache::CacheState;
    use crate::config::Config;
    use crate::models::{ClaudeService, CodexService, DeepSeekService, EnabledProviders};
    use chrono::{DateTime, Utc};

    fn nominal_services() -> (ClaudeService, CodexService, DeepSeekService) {
        let mut claude = ClaudeService::default();
        let mut codex = CodexService::default();
        let mut deepseek = DeepSeekService::default();
        claude.status = "NOMINAL".into();
        codex.status = "NOMINAL".into();
        deepseek.status = "NOMINAL".into();
        (claude, codex, deepseek)
    }

    #[test]
    fn live_service_warning_degrades_aggregate_status() {
        let (claude, codex, mut deepseek) = nominal_services();
        deepseek.status = "INSUFFICIENT BALANCE".into();

        let summary = assemble(
            claude,
            codex,
            deepseek,
            false,
            false,
            false,
            EnabledProviders::default(),
        );

        assert_eq!(summary.status, "partial");
    }

    #[test]
    fn all_nominal_live_services_are_ok() {
        let (claude, codex, deepseek) = nominal_services();

        let summary = assemble(
            claude,
            codex,
            deepseek,
            false,
            false,
            false,
            EnabledProviders::default(),
        );

        assert_eq!(summary.status, "ok");
    }

    #[test]
    fn disabled_provider_failures_do_not_degrade_enabled_services() {
        let (claude, codex, mut deepseek) = nominal_services();
        deepseek.status = "API ERROR".into();
        let enabled = EnabledProviders {
            codex: true,
            claude: true,
            deepseek: false,
        };

        let summary = assemble(claude, codex, deepseek, false, false, true, enabled);

        assert_eq!(summary.status, "ok");
    }

    #[test]
    fn no_enabled_providers_skips_the_refresh_cycle() {
        let mut cache = CacheState::default();
        let config = Config {
            enabled_providers: EnabledProviders {
                codex: false,
                claude: false,
                deepseek: false,
            },
            ..Config::default()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");

        let summary = runtime.block_on(super::collect_summary(&config, &mut cache));

        assert_eq!(summary.status, "idle");
        assert_eq!(summary.enabled_providers, config.enabled_providers);
        assert!(cache.updated_at.is_none());
        assert!(cache.services.is_empty());
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = DateTime::parse_from_rfc3339("2026-07-11T01:00:00.250Z")
            .expect("fixed timestamp")
            .with_timezone(&Utc);

        assert_eq!(parse_retry_after_at("91", now), Some(91));
        assert_eq!(
            parse_retry_after_at("Sat, 11 Jul 2026 01:02:01 GMT", now),
            Some(121)
        );
        assert_eq!(
            parse_retry_after_at("Sat, 11 Jul 2026 00:59:00 GMT", now),
            Some(0)
        );
        assert_eq!(parse_retry_after_at("later", now), None);
    }

    #[test]
    fn retry_after_is_clamped_to_one_day() {
        let now = DateTime::parse_from_rfc3339("2026-07-11T01:00:00Z")
            .expect("fixed timestamp")
            .with_timezone(&Utc);

        assert_eq!(
            parse_retry_after_at("100000000000000000", now),
            Some(86_400)
        );
        assert_eq!(
            parse_retry_after_at("Fri, 31 Dec 9999 23:59:59 GMT", now),
            Some(86_400)
        );
    }
}
