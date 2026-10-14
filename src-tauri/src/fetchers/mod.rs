//! HTTP helpers, error types, and the per-cycle orchestration that turns
//! independent fetches into one sanitized `UsageSummary`.

pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod deepseek;
pub mod grok;
pub mod opencodego;

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
pub(crate) const MSG_KEY_MISSING: &str = "KEY MISSING";
pub(crate) const MSG_LOGIN_REQUIRED: &str = "LOGIN REQUIRED";
pub(crate) const MSG_AUTH_EXPIRED: &str = "AUTH EXPIRED";
pub(crate) const MSG_AUTH_FORBIDDEN: &str = "AUTH FORBIDDEN";
pub(crate) const MSG_CREDENTIAL_ERROR: &str = "CREDENTIAL ERROR";

pub(crate) fn require_success(resp: &Resp, label: &str) -> Result<(), FetchError> {
    match resp.status {
        200..=299 => Ok(()),
        401 => Err(FetchError::Auth {
            message: MSG_AUTH_EXPIRED,
        }),
        403 => Err(FetchError::Auth {
            message: MSG_AUTH_FORBIDDEN,
        }),
        429 => Err(FetchError::RateLimited {
            retry_after: resp.retry_after,
        }),
        status => Err(FetchError::Other(anyhow::anyhow!("{label} HTTP {status}"))),
    }
}

async fn join_optional_fetch<T>(
    task: Option<tokio::task::JoinHandle<Result<T, FetchError>>>,
) -> Option<Result<T, FetchError>> {
    match task {
        Some(handle) => Some(handle.await.unwrap_or_else(|_| {
            Err(FetchError::Other(anyhow::anyhow!(
                "provider fetch task failed"
            )))
        })),
        None => None,
    }
}

fn apply_standard_fetch<T>(
    cache: &mut CacheState,
    key: &'static str,
    enabled: bool,
    fetched_live: bool,
    fetched: Option<Result<T, FetchError>>,
) -> (T, bool)
where
    T: Default + serde::Serialize + DeserializeOwned,
{
    if !enabled {
        return (T::default(), false);
    }
    if !fetched_live {
        return (cached_service(cache, key, MSG_RATE_LIMITED, None), true);
    }
    match fetched.expect("live provider fetch was scheduled") {
        Ok(svc) => {
            cache.clear_provider_cooldown(key);
            if let Ok(v) = serde_json::to_value(&svc) {
                cache.put(key, v);
            }
            (svc, false)
        }
        Err(FetchError::Auth { message }) => (cached_service(cache, key, message, None), true),
        Err(FetchError::RateLimited { retry_after }) => {
            cache.set_provider_cooldown(key, retry_deadline(retry_after));
            (cached_service(cache, key, MSG_RATE_LIMITED, None), true)
        }
        Err(FetchError::Other(_)) => (cached_service(cache, key, MSG_FAILED, None), true),
    }
}

fn retry_deadline(retry_after: Option<u64>) -> DateTime<Utc> {
    let seconds = retry_after
        .map(|value| value.saturating_add(30).min(MAX_RETRY_AFTER_SECONDS))
        .unwrap_or(1800);
    Utc::now() + Duration::seconds(seconds as i64)
}

/// Build a summary purely from cached data, used to seed the UI on startup so
/// it never begins blank while the first live refresh is in flight.
pub fn summary_from_cache(cache: &CacheState, enabled: EnabledProviders) -> UsageSummary {
    use crate::models::{
        AntigravityService, ClaudeService, CodexService, CursorService, DeepSeekService,
        GrokService, OpenCodeGoService,
    };

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
    let grok: GrokService = if enabled.x_pool() && cache.get("grok").is_some() {
        cached_service(cache, "grok", MSG_CACHED, None)
    } else {
        GrokService::default()
    };
    let cursor: CursorService = if enabled.cursor && cache.get("cursor").is_some() {
        cached_service(cache, "cursor", MSG_CACHED, None)
    } else {
        CursorService::default()
    };
    let antigravity: AntigravityService =
        if enabled.antigravity && cache.get("antigravity").is_some() {
            cached_service(cache, "antigravity", MSG_CACHED, None)
        } else {
            AntigravityService::default()
        };
    let opencodego: OpenCodeGoService = if enabled.opencodego && cache.get("opencodego").is_some() {
        cached_service(cache, "opencodego", MSG_CACHED, None)
    } else {
        OpenCodeGoService::default()
    };

    let any = (enabled.claude && cache.get("claude").is_some())
        || (enabled.codex && cache.get("codex").is_some())
        || (enabled.deepseek && cache.get("deepseek").is_some())
        || (enabled.grok && cache.get("grok").is_some())
        || (enabled.cursor && cache.get("cursor").is_some())
        || (enabled.antigravity && cache.get("antigravity").is_some())
        || (enabled.opencodego && cache.get("opencodego").is_some());

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
            grok,
            cursor,
            antigravity,
            opencodego,
        },
    }
}

/// Run all enabled fetches and assemble the summary. Updates `cache` in place
/// (callers should persist it). Never returns an error — failures degrade to
/// cached/empty service cards.
pub async fn collect_summary(config: &Config, cache: &mut CacheState) -> UsageSummary {
    use crate::models::{
        AntigravityService, ClaudeService, CodexService, CursorService, DeepSeekService,
        GrokService, OpenCodeGoService,
    };

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
            let grok = if enabled.x_pool() {
                cached_service::<GrokService>(cache, "grok", MSG_FAILED, None)
            } else {
                GrokService::default()
            };
            let cursor = if enabled.cursor {
                cached_service::<CursorService>(cache, "cursor", MSG_FAILED, None)
            } else {
                CursorService::default()
            };
            let antigravity = if enabled.antigravity {
                cached_service::<AntigravityService>(cache, "antigravity", MSG_FAILED, None)
            } else {
                AntigravityService::default()
            };
            let opencodego = if enabled.opencodego {
                cached_service::<OpenCodeGoService>(cache, "opencodego", MSG_FAILED, None)
            } else {
                OpenCodeGoService::default()
            };
            return assemble(
                Services {
                    codex,
                    claude,
                    deepseek,
                    grok,
                    cursor,
                    antigravity,
                    opencodego,
                },
                enabled,
                enabled,
            );
        }
    };

    let claude_cooldown = enabled.claude.then(|| cache.cooldown_active()).flatten();
    let fetch_claude = enabled.claude && claude_cooldown.is_none();
    let fetch_codex = enabled.codex && cache.provider_cooldown_active("codex").is_none();
    let fetch_deepseek = enabled.deepseek && cache.provider_cooldown_active("deepseek").is_none();
    let fetch_grok = enabled.x_pool() && cache.provider_cooldown_active("grok").is_none();
    // A seat already known to be on the unified X pool shows no Cursor card,
    // so Cursor's own feed is not read. A seat that switches back is picked
    // up on the next refresh after Grok reports it.
    let cursor_folded = enabled.x_pool() && cached_x_pool(cache);
    let fetch_cursor =
        enabled.cursor && !cursor_folded && cache.provider_cooldown_active("cursor").is_none();
    let fetch_antigravity =
        enabled.antigravity && cache.provider_cooldown_active("antigravity").is_none();
    let fetch_opencodego =
        enabled.opencodego && cache.provider_cooldown_active("opencodego").is_none();

    let claude_task = fetch_claude.then(|| {
        let config = config.clone();
        let client = client.clone();
        tokio::spawn(async move { claude::fetch(&config, &client).await })
    });
    let codex_task = fetch_codex.then(|| {
        let config = config.clone();
        let client = client.clone();
        tokio::spawn(async move { codex::fetch(&config, &client).await })
    });
    let deepseek_task = fetch_deepseek.then(|| {
        let config = config.clone();
        let client = client.clone();
        tokio::spawn(async move { deepseek::fetch(&config, &client).await })
    });
    let grok_task = fetch_grok.then(|| {
        let config = config.clone();
        let client = client.clone();
        tokio::spawn(async move { grok::fetch(&config, &client).await })
    });
    let cursor_task = fetch_cursor.then(|| {
        let config = config.clone();
        let client = client.clone();
        tokio::spawn(async move { cursor::fetch(&config, &client).await })
    });
    let antigravity_task = fetch_antigravity.then(|| {
        let config = config.clone();
        let client = client.clone();
        tokio::spawn(async move { antigravity::fetch(&config, &client).await })
    });
    let opencodego_task = fetch_opencodego.then(|| {
        let config = config.clone();
        let client = client.clone();
        tokio::spawn(async move { opencodego::fetch(&config, &client).await })
    });

    let claude_res = join_optional_fetch(claude_task).await;
    let codex_res = join_optional_fetch(codex_task).await;
    let deepseek_res = join_optional_fetch(deepseek_task).await;
    let grok_res = join_optional_fetch(grok_task).await;
    let cursor_res = join_optional_fetch(cursor_task).await;
    let antigravity_res = join_optional_fetch(antigravity_task).await;
    let opencodego_res = join_optional_fetch(opencodego_task).await;

    let (claude, claude_cached) = if !enabled.claude {
        (ClaudeService::default(), false)
    } else if let Some(until) = claude_cooldown {
        (
            cached_service(cache, "claude", MSG_RATE_LIMITED, Some(fmt_local(until))),
            true,
        )
    } else {
        match claude_res.expect("Claude live fetch was scheduled") {
            Ok(svc) => {
                cache.claude_cooldown_until = None;
                if let Ok(v) = serde_json::to_value(&svc) {
                    cache.put("claude", v);
                }
                (svc, false)
            }
            Err(FetchError::RateLimited { retry_after }) => {
                let until = retry_deadline(retry_after);
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

    let (codex, codex_cached) =
        apply_standard_fetch(cache, "codex", enabled.codex, fetch_codex, codex_res);
    let (deepseek, deepseek_cached) = apply_standard_fetch(
        cache,
        "deepseek",
        enabled.deepseek,
        fetch_deepseek,
        deepseek_res,
    );
    let (grok, grok_cached) =
        apply_standard_fetch(cache, "grok", enabled.x_pool(), fetch_grok, grok_res);
    let (cursor, cursor_cached) = if enabled.cursor && cursor_folded {
        // Skipped, not rate limited: hidden behind the X card, or shown from
        // cache for one cycle if the seat has just left the unified pool.
        let cached = cache.get("cursor").is_some();
        let svc = if cached {
            cached_service(cache, "cursor", MSG_CACHED, None)
        } else {
            CursorService::default()
        };
        (svc, cached)
    } else {
        apply_standard_fetch(cache, "cursor", enabled.cursor, fetch_cursor, cursor_res)
    };
    let (antigravity, antigravity_cached) = apply_standard_fetch(
        cache,
        "antigravity",
        enabled.antigravity,
        fetch_antigravity,
        antigravity_res,
    );
    let (opencodego, opencodego_cached) = apply_standard_fetch(
        cache,
        "opencodego",
        enabled.opencodego,
        fetch_opencodego,
        opencodego_res,
    );

    cache.updated_at = Some(Utc::now());
    assemble(
        Services {
            codex,
            claude,
            deepseek,
            grok,
            cursor,
            antigravity,
            opencodego,
        },
        EnabledProviders {
            codex: codex_cached,
            claude: claude_cached,
            deepseek: deepseek_cached,
            grok: grok_cached,
            cursor: cursor_cached,
            antigravity: antigravity_cached,
            opencodego: opencodego_cached,
        },
        enabled,
    )
}

fn cached_x_pool(cache: &CacheState) -> bool {
    cache
        .get("grok")
        .and_then(|card| card.data.get("unified"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn login_missing(status: &str) -> bool {
    status.ends_with(MSG_LOGIN_REQUIRED)
}

/// Cards actually on screen. Settings expose Grok and Cursor as one X toggle:
/// a unified pool shows a single X card in the Grok slot, and an unswitched
/// seat keeps both cards but drops a side that has no local login while the
/// other side works. Must match `enabledProviderList` in `planLayout.ts`.
pub(crate) fn shown_providers(enabled: EnabledProviders, services: &Services) -> EnabledProviders {
    if enabled.x_pool() && services.grok.unified {
        return EnabledProviders {
            grok: true,
            cursor: false,
            ..enabled
        };
    }
    let grok_missing = login_missing(&services.grok.status);
    let cursor_missing = login_missing(&services.cursor.status);
    let both = enabled.grok && enabled.cursor;
    EnabledProviders {
        grok: enabled.grok && !(both && grok_missing && !cursor_missing),
        cursor: enabled.cursor && !(both && cursor_missing && !grok_missing),
        ..enabled
    }
}

pub(crate) fn assemble(
    services: Services,
    fallbacks: EnabledProviders,
    enabled: EnabledProviders,
) -> UsageSummary {
    let shown = shown_providers(enabled, &services);
    let active = [
        (shown.claude, &services.claude.status, fallbacks.claude),
        (shown.codex, &services.codex.status, fallbacks.codex),
        (
            shown.deepseek,
            &services.deepseek.status,
            fallbacks.deepseek,
        ),
        (shown.grok, &services.grok.status, fallbacks.grok),
        (shown.cursor, &services.cursor.status, fallbacks.cursor),
        (
            shown.antigravity,
            &services.antigravity.status,
            fallbacks.antigravity,
        ),
        (
            shown.opencodego,
            &services.opencodego.status,
            fallbacks.opencodego,
        ),
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
        services,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assemble, parse_retry_after_at, require_success, FetchError, Resp, MSG_AUTH_EXPIRED,
        MSG_AUTH_FORBIDDEN,
    };
    use crate::cache::CacheState;
    use crate::config::Config;
    use crate::models::{
        AntigravityService, ClaudeService, CodexService, CursorService, DeepSeekService,
        EnabledProviders, GrokService, Services,
    };
    use chrono::{DateTime, Utc};

    fn nominal_services() -> Services {
        let mut claude = ClaudeService::default();
        let mut codex = CodexService::default();
        let mut deepseek = DeepSeekService::default();
        let mut grok = GrokService::default();
        let mut cursor = CursorService::default();
        let mut antigravity = AntigravityService::default();
        claude.status = "NOMINAL".into();
        codex.status = "NOMINAL".into();
        deepseek.status = "NOMINAL".into();
        grok.status = "NOMINAL".into();
        cursor.status = "NOMINAL".into();
        antigravity.status = "NOMINAL".into();
        let opencodego = crate::models::OpenCodeGoService {
            status: "NOMINAL".into(),
            ..Default::default()
        };
        Services {
            codex,
            claude,
            deepseek,
            grok,
            cursor,
            antigravity,
            opencodego,
        }
    }

    #[test]
    fn live_service_warning_degrades_aggregate_status() {
        let mut services = nominal_services();
        services.deepseek.status = "INSUFFICIENT BALANCE".into();

        let summary = assemble(
            services,
            EnabledProviders {
                codex: false,
                claude: false,
                deepseek: false,
                grok: false,
                cursor: false,
                antigravity: false,
                opencodego: false,
            },
            EnabledProviders::default(),
        );

        assert_eq!(summary.status, "partial");
    }

    #[test]
    fn all_nominal_live_services_are_ok() {
        let services = nominal_services();

        let summary = assemble(
            services,
            EnabledProviders {
                codex: false,
                claude: false,
                deepseek: false,
                grok: false,
                cursor: false,
                antigravity: false,
                opencodego: false,
            },
            EnabledProviders::default(),
        );

        assert_eq!(summary.status, "ok");
    }

    #[test]
    fn disabled_provider_failures_do_not_degrade_enabled_services() {
        let mut services = nominal_services();
        services.deepseek.status = "API ERROR".into();
        let enabled = EnabledProviders {
            codex: true,
            claude: true,
            deepseek: false,
            grok: false,
            cursor: false,
            antigravity: false,
            opencodego: false,
        };

        let fallbacks = EnabledProviders {
            codex: false,
            claude: false,
            deepseek: true,
            grok: false,
            cursor: false,
            antigravity: false,
            opencodego: false,
        };
        let summary = assemble(services, fallbacks, enabled);

        assert_eq!(summary.status, "ok");
    }

    #[test]
    fn grok_only_failure_is_an_aggregate_error() {
        let mut services = nominal_services();
        services.grok.status = "API ERROR".into();
        let enabled = EnabledProviders {
            codex: false,
            claude: false,
            deepseek: false,
            grok: true,
            cursor: false,
            antigravity: false,
            opencodego: false,
        };

        let fallbacks = EnabledProviders {
            codex: false,
            claude: false,
            deepseek: false,
            grok: true,
            cursor: false,
            antigravity: false,
            opencodego: false,
        };
        let summary = assemble(services, fallbacks, enabled);

        assert_eq!(summary.status, "error");
    }

    #[test]
    fn x_toggle_hides_the_side_without_a_local_login() {
        let x = EnabledProviders {
            grok: true,
            cursor: true,
            ..EnabledProviders::default()
        };

        // Cursor-only machine on an unswitched seat.
        let mut services = nominal_services();
        services.grok.status = "GROK LOGIN REQUIRED".into();
        let shown = super::shown_providers(x, &services);
        assert!(!shown.grok);
        assert!(shown.cursor);

        // Grok-only machine.
        let mut services = nominal_services();
        services.cursor.status = "LOGIN REQUIRED".into();
        let shown = super::shown_providers(x, &services);
        assert!(shown.grok);
        assert!(!shown.cursor);

        // Neither side signed in: keep the X slot so the login hint is visible.
        let mut services = nominal_services();
        services.grok.status = "GROK LOGIN REQUIRED".into();
        services.cursor.status = "LOGIN REQUIRED".into();
        let shown = super::shown_providers(x, &services);
        assert!(shown.grok);
        assert!(shown.cursor);

        // An expired session is not a missing install and stays visible.
        let mut services = nominal_services();
        services.cursor.status = "SESSION EXPIRED".into();
        let shown = super::shown_providers(x, &services);
        assert!(shown.grok && shown.cursor);

        // A cursor-only legacy config keeps its card even without Grok.
        let only_cursor = EnabledProviders { grok: false, ..x };
        let mut services = nominal_services();
        services.grok.status = "GROK LOGIN REQUIRED".into();
        assert!(super::shown_providers(only_cursor, &services).cursor);
    }

    #[test]
    fn cached_unified_pool_skips_cursor() {
        let mut cache = CacheState::default();
        assert!(!super::cached_x_pool(&cache));
        cache.put(
            "grok",
            serde_json::json!({ "status": "NOMINAL", "unified": true }),
        );
        assert!(super::cached_x_pool(&cache));
        cache.put(
            "grok",
            serde_json::json!({ "status": "NOMINAL", "unified": false }),
        );
        assert!(!super::cached_x_pool(&cache));
    }

    #[test]
    fn folded_cursor_failure_does_not_degrade_a_healthy_x_card() {
        let mut services = nominal_services();
        services.grok.unified = true;
        services.cursor.status = "API ERROR".into();
        let only_cursor = EnabledProviders {
            codex: false,
            claude: false,
            deepseek: false,
            grok: false,
            cursor: true,
            antigravity: false,
            opencodego: false,
        };
        let no_fallbacks = EnabledProviders {
            cursor: false,
            ..only_cursor
        };

        let summary = assemble(services.clone(), no_fallbacks, only_cursor);
        assert_eq!(summary.status, "ok");
        // Settings keep the user's real toggles.
        assert!(summary.enabled_providers.cursor);
        assert!(!summary.enabled_providers.grok);

        services.grok.status = "API ERROR".into();
        let summary = assemble(services, no_fallbacks, only_cursor);
        assert_eq!(summary.status, "error");
    }

    #[test]
    fn cursor_only_failure_is_an_aggregate_error() {
        let mut services = nominal_services();
        services.cursor.status = "API ERROR".into();
        let enabled = EnabledProviders {
            codex: false,
            claude: false,
            deepseek: false,
            grok: false,
            cursor: true,
            antigravity: false,
            opencodego: false,
        };

        let fallbacks = EnabledProviders {
            codex: false,
            claude: false,
            deepseek: false,
            grok: false,
            cursor: true,
            antigravity: false,
            opencodego: false,
        };
        let summary = assemble(services, fallbacks, enabled);

        assert_eq!(summary.status, "error");
    }

    #[test]
    fn disabled_grok_cache_is_retained_for_reenable() {
        let mut cache = CacheState::default();
        let grok = GrokService {
            status: "NOMINAL".into(),
            usage_percent: Some(12.0),
            ..GrokService::default()
        };
        cache.put(
            "grok",
            serde_json::to_value(&grok).expect("serialize Grok cache"),
        );

        let disabled = super::summary_from_cache(&cache, EnabledProviders::default());
        assert_eq!(disabled.services.grok.status, "AWAITING DATA");
        assert!(cache.get("grok").is_some());

        let enabled = EnabledProviders {
            codex: false,
            claude: false,
            deepseek: false,
            grok: true,
            cursor: false,
            antigravity: false,
            opencodego: false,
        };
        let restored = super::summary_from_cache(&cache, enabled);
        assert_eq!(restored.services.grok.status, "LAST KNOWN");
        assert_eq!(restored.services.grok.usage_percent, Some(12.0));
        assert!(restored.services.grok.from_cache);
    }

    #[test]
    fn no_enabled_providers_skips_the_refresh_cycle() {
        let mut cache = CacheState::default();
        let config = Config {
            enabled_providers: EnabledProviders {
                codex: false,
                claude: false,
                deepseek: false,
                grok: false,
                cursor: false,
                antigravity: false,
                opencodego: false,
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

    #[test]
    fn shared_http_classifier_preserves_auth_and_retry_after() {
        let response = |status, retry_after| Resp {
            status,
            retry_after,
            body: String::new(),
        };

        assert!(require_success(&response(204, None), "test").is_ok());
        assert!(matches!(
            require_success(&response(401, None), "test"),
            Err(FetchError::Auth {
                message: MSG_AUTH_EXPIRED
            })
        ));
        assert!(matches!(
            require_success(&response(403, None), "test"),
            Err(FetchError::Auth {
                message: MSG_AUTH_FORBIDDEN
            })
        ));
        assert!(matches!(
            require_success(&response(429, Some(91)), "test"),
            Err(FetchError::RateLimited {
                retry_after: Some(91)
            })
        ));
        assert!(matches!(
            require_success(&response(500, None), "test"),
            Err(FetchError::Other(_))
        ));
    }
}
