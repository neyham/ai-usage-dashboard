//! HTTP helpers, error types, and the per-cycle orchestration that turns three
//! independent fetches into one sanitized `UsageSummary`.

pub mod claude;
pub mod codex;
pub mod deepseek;

use crate::cache::CacheState;
use crate::config::Config;
use crate::models::{Services, UsageSummary};
use crate::util::fmt_local;
use chrono::{DateTime, Duration, Utc};
use reqwest::{Client, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration as StdDuration;

pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125 Safari/537.36";

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
    RateLimited {
        retry_after: Option<u64>,
    },
    /// Authentication or OAuth refresh failure, sanitized for the UI.
    Auth {
        message: &'static str,
    },
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
        return Some(seconds);
    }

    let retry_at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    let millis = retry_at.signed_duration_since(now).num_milliseconds();
    if millis <= 0 {
        Some(0)
    } else {
        Some((millis as u64).div_ceil(1000))
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
pub fn summary_from_cache(cache: &CacheState) -> UsageSummary {
    use crate::models::{ClaudeService, CodexService, DeepSeekService};

    let cooldown_label = cache.cooldown_active().map(fmt_local);

    let claude: ClaudeService = if cache.get("claude").is_some() {
        let msg = if cooldown_label.is_some() {
            MSG_RATE_LIMITED
        } else {
            MSG_CACHED
        };
        cached_service(cache, "claude", msg, cooldown_label)
    } else {
        ClaudeService::default()
    };
    let codex: CodexService = if cache.get("codex").is_some() {
        cached_service(cache, "codex", MSG_CACHED, None)
    } else {
        CodexService::default()
    };
    let deepseek: DeepSeekService = if cache.get("deepseek").is_some() {
        cached_service(cache, "deepseek", MSG_CACHED, None)
    } else {
        DeepSeekService::default()
    };

    let any = cache.get("claude").is_some()
        || cache.get("codex").is_some()
        || cache.get("deepseek").is_some();

    UsageSummary {
        refreshed_at: cache.updated_at.map(|t| t.to_rfc3339()),
        status: if any { "partial".into() } else { "idle".into() },
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

    let client = match build_client(config.network_timeout_seconds) {
        Ok(c) => c,
        Err(_) => {
            // Without a client we can only show whatever is cached.
            let claude = cached_service::<ClaudeService>(cache, "claude", MSG_FAILED, None);
            let codex = cached_service::<CodexService>(cache, "codex", MSG_FAILED, None);
            let deepseek = cached_service::<DeepSeekService>(cache, "deepseek", MSG_FAILED, None);
            return assemble(claude, codex, deepseek, true, true, true);
        }
    };

    // ----- Claude (honor any active cooldown before hitting the network) -----
    let (claude, claude_cached): (ClaudeService, bool) =
        if let Some(until) = cache.cooldown_active() {
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
                    let secs = retry_after.map(|s| s + 30).unwrap_or(1800);
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
                Err(FetchError::Other(err)) => {
                    let _ = err.to_string();
                    (cached_service(cache, "claude", MSG_FAILED, None), true)
                }
            }
        };

    // ----- Codex -----
    let (codex, codex_cached): (CodexService, bool) = match codex::fetch(config, &client).await {
        Ok(svc) => {
            if let Ok(v) = serde_json::to_value(&svc) {
                cache.put("codex", v);
            }
            (svc, false)
        }
        Err(_) => (cached_service(cache, "codex", MSG_FAILED, None), true),
    };

    // ----- DeepSeek -----
    let (deepseek, deepseek_cached): (DeepSeekService, bool) =
        match deepseek::fetch(config, &client).await {
            Ok(svc) => {
                if let Ok(v) = serde_json::to_value(&svc) {
                    cache.put("deepseek", v);
                }
                (svc, false)
            }
            Err(_) => (cached_service(cache, "deepseek", MSG_FAILED, None), true),
        };

    cache.updated_at = Some(Utc::now());
    assemble(
        claude,
        codex,
        deepseek,
        claude_cached,
        codex_cached,
        deepseek_cached,
    )
}

fn assemble(
    claude: crate::models::ClaudeService,
    codex: crate::models::CodexService,
    deepseek: crate::models::DeepSeekService,
    claude_cached: bool,
    codex_cached: bool,
    deepseek_cached: bool,
) -> UsageSummary {
    let any_cache = claude_cached || codex_cached || deepseek_cached;
    let all_cache = claude_cached && codex_cached && deepseek_cached;
    let unhealthy_services = [&claude.status, &codex.status, &deepseek.status]
        .into_iter()
        .filter(|status| status.as_str() != "NOMINAL")
        .count();
    let status = if all_cache || unhealthy_services == 3 {
        "error"
    } else if any_cache || unhealthy_services > 0 {
        "partial"
    } else {
        "ok"
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

#[cfg(test)]
mod tests {
    use super::{assemble, parse_retry_after_at};
    use crate::models::{ClaudeService, CodexService, DeepSeekService};
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

        let summary = assemble(claude, codex, deepseek, false, false, false);

        assert_eq!(summary.status, "partial");
    }

    #[test]
    fn all_nominal_live_services_are_ok() {
        let (claude, codex, deepseek) = nominal_services();

        let summary = assemble(claude, codex, deepseek, false, false, false);

        assert_eq!(summary.status, "ok");
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
}
