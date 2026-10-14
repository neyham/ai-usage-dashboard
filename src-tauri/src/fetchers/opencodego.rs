//! OpenCode Go subscription usage. The API key is read-only from OpenCode's
//! own storage at call time and is never cached, logged, or sent to the
//! renderer. Lookup order:
//!   1. `openCodeGoCredentialsPath` override (an `opencode.db` or `auth.json`)
//!   2. OpenCode 2.x `opencode.db`, table `credential`, integration `opencode-go`
//!   3. OpenCode 1.x `auth.json`, entry `opencode-go`
//!   4. `OPENCODE_API_KEY` environment variable
//!
//! Only the server-reported percentages are used. OpenCode Go and Go Plus set
//! limits per model, so device-local cost totals cannot stand in for quota.

use super::{require_success, send_with_one_retry, FetchError, Resp, MSG_KEY_MISSING};
use crate::config::Config;
use crate::models::OpenCodeGoService;
use crate::util::{clamp_percent, fmt_local, parse_datetime_str};
use anyhow::Context;
use reqwest::Client;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const INTEGRATION_ID: &str = "opencode-go";
const MAX_AUTH_BYTES: u64 = 256 * 1024;

pub async fn fetch(config: &Config, client: &Client) -> Result<OpenCodeGoService, FetchError> {
    let override_path = config.open_code_go_credentials_path.trim().to_string();
    let key = tokio::task::spawn_blocking(move || resolve_key(&override_path))
        .await
        .ok()
        .flatten()
        .ok_or(FetchError::Auth {
            message: MSG_KEY_MISSING,
        })?;

    let resp: Resp = send_with_one_retry(|| {
        client
            .get(USAGE_URL)
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "application/json")
    })
    .await
    .map_err(FetchError::Other)?;

    require_success(&resp, "OpenCode Go usage")?;
    parse_usage(&resp.body).map_err(FetchError::Other)
}

fn resolve_key(override_path: &str) -> Option<String> {
    if !override_path.is_empty() {
        return key_from_path(Path::new(override_path));
    }
    let dir = data_dir()?;
    key_from_path(&dir.join("opencode.db"))
        .or_else(|| key_from_path(&dir.join("auth.json")))
        .or_else(|| {
            std::env::var("OPENCODE_API_KEY")
                .ok()
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
        })
}

/// OpenCode stores data under the XDG data dir on every platform, including
/// `%USERPROFILE%\.local\share\opencode` on Windows.
fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))?;
    Some(base.join("opencode"))
}

fn key_from_path(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let is_json = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
    if is_json {
        key_from_auth_json(path).ok().flatten()
    } else {
        key_from_db(path).ok().flatten()
    }
}

fn key_from_db(path: &Path) -> anyhow::Result<Option<String>> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(super::cursor::sqlite_file_uri(path, "mode=ro"), flags)
        .with_context(|| format!("open OpenCode db at {}", path.display()))?;
    conn.busy_timeout(StdDuration::from_millis(250))
        .context("set OpenCode db busy timeout")?;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM credential WHERE integration_id = ?1 \
             ORDER BY COALESCE(active, 0) DESC, time_updated DESC LIMIT 1",
            [INTEGRATION_ID],
            |row| row.get(0),
        )
        .optional()
        .context("query OpenCode Go credential")?;
    Ok(value.and_then(|text| api_key_from_value(&serde_json::from_str(&text).ok()?)))
}

fn key_from_auth_json(path: &Path) -> anyhow::Result<Option<String>> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .context("open OpenCode auth.json")?
        .take(MAX_AUTH_BYTES)
        .read_to_string(&mut text)
        .context("read OpenCode auth.json")?;
    let root: Value = serde_json::from_str(&text).context("parse OpenCode auth.json")?;
    Ok(root.get(INTEGRATION_ID).and_then(api_key_from_value))
}

/// OpenCode 2.x stores `{"type":"key","key":"…"}` and 1.x `{"type":"api",…}`;
/// other auth types (OAuth) are ignored.
fn api_key_from_value(value: &Value) -> Option<String> {
    if !matches!(
        value.get("type").and_then(Value::as_str),
        Some("key" | "api")
    ) {
        return None;
    }
    value
        .get("key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty() && !key.chars().any(char::is_whitespace))
        .map(str::to_string)
}

struct Window {
    percent: f64,
    reset_local: Option<String>,
    limited: bool,
}

fn parse_window(usage: &Value, name: &str) -> Option<Window> {
    let window = usage.get(name)?;
    let percent = clamp_percent(window.get("percent")?.as_f64()?);
    let status = window.get("status").and_then(Value::as_str).unwrap_or("ok");
    Some(Window {
        percent,
        reset_local: window
            .get("resetsAt")
            .and_then(Value::as_str)
            .and_then(parse_datetime_str)
            .map(fmt_local),
        limited: !status.eq_ignore_ascii_case("ok") && percent >= 100.0,
    })
}

pub(crate) fn parse_usage(body: &str) -> anyhow::Result<OpenCodeGoService> {
    let root: Value = serde_json::from_str(body).context("parse OpenCode Go usage body")?;
    let usage = root
        .get("usage")
        .context("OpenCode Go usage missing usage")?;
    let rolling =
        parse_window(usage, "rolling").context("OpenCode Go usage missing rolling window")?;
    let weekly = parse_window(usage, "weekly");
    let monthly = parse_window(usage, "monthly");
    let limited = rolling.limited
        || weekly.as_ref().is_some_and(|window| window.limited)
        || monthly.as_ref().is_some_and(|window| window.limited);

    Ok(OpenCodeGoService {
        status: if limited {
            "QUOTA EXHAUSTED".into()
        } else {
            "NOMINAL".into()
        },
        from_cache: false,
        data_may_be_stale: false,
        rolling_percent: Some(rolling.percent),
        rolling_reset_local: rolling.reset_local,
        weekly_percent: weekly.as_ref().map(|window| window.percent),
        weekly_reset_local: weekly.and_then(|window| window.reset_local),
        monthly_percent: monthly.as_ref().map(|window| window.percent),
        monthly_reset_local: monthly.and_then(|window| window.reset_local),
    })
}

#[cfg(test)]
mod tests {
    use super::{api_key_from_value, key_from_auth_json, key_from_db, parse_usage};
    use rusqlite::Connection;
    use serde_json::json;

    #[test]
    fn usage_maps_three_windows() {
        let svc = parse_usage(
            r#"{"usage":{
                "rolling":{"status":"ok","percent":2,"resetsAt":"2026-10-14T18:51:57.000Z"},
                "weekly":{"status":"ok","percent":0.5,"resetsAt":"2026-10-19T00:00:00.000Z"},
                "monthly":{"status":"ok","percent":61,"resetsAt":"2026-11-14T13:32:33.000Z"}
            }}"#,
        )
        .expect("valid usage");
        assert_eq!(svc.status, "NOMINAL");
        assert_eq!(svc.rolling_percent, Some(2.0));
        assert_eq!(svc.weekly_percent, Some(0.5));
        assert_eq!(svc.monthly_percent, Some(61.0));
        assert!(svc.rolling_reset_local.is_some());
        assert!(svc.monthly_reset_local.is_some());
    }

    #[test]
    fn missing_rolling_window_is_a_parse_error_and_others_stay_absent() {
        assert!(parse_usage(r#"{"usage":{"weekly":{"percent":3}}}"#).is_err());
        assert!(parse_usage(r#"{"error":"nope"}"#).is_err());
        let svc = parse_usage(r#"{"usage":{"rolling":{"percent":250}}}"#).expect("rolling only");
        assert_eq!(svc.rolling_percent, Some(100.0));
        assert_eq!(svc.weekly_percent, None);
        assert_eq!(svc.monthly_percent, None);
    }

    #[test]
    fn exhausted_window_is_reported() {
        let svc = parse_usage(
            r#"{"usage":{"rolling":{"status":"ok","percent":10},
                "weekly":{"status":"limited","percent":100}}}"#,
        )
        .expect("valid usage");
        assert_eq!(svc.status, "QUOTA EXHAUSTED");
    }

    #[test]
    fn only_api_keys_are_accepted() {
        assert_eq!(
            api_key_from_value(&json!({"type":"api","key":" sk-test "})).as_deref(),
            Some("sk-test")
        );
        assert_eq!(
            api_key_from_value(&json!({"type":"key","key":"sk-v2"})).as_deref(),
            Some("sk-v2")
        );
        assert_eq!(
            api_key_from_value(&json!({"type":"oauth","key":"sk-test"})),
            None
        );
        assert_eq!(api_key_from_value(&json!({"type":"api","key":"a b"})), None);
        assert_eq!(api_key_from_value(&json!({"type":"api"})), None);
    }

    #[test]
    #[ignore = "requires a signed-in OpenCode Go install on this machine"]
    fn live_fetch_uses_local_opencode_key() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let client = crate::fetchers::build_client(15).expect("client");
        let svc = runtime
            .block_on(super::fetch(&crate::config::Config::default(), &client))
            .unwrap_or_else(|error| match error {
                crate::fetchers::FetchError::Auth { message } => panic!("auth: {message}"),
                crate::fetchers::FetchError::RateLimited { .. } => panic!("rate limited"),
                crate::fetchers::FetchError::Other(error) => panic!("other: {error:#}"),
            });
        assert_eq!(svc.status, "NOMINAL");
        assert!(svc.rolling_percent.is_some());
    }

    #[test]
    fn reads_v2_db_and_v1_auth_json() {
        let dir = std::env::temp_dir().join(format!("ocgo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let db = dir.join("opencode.db");
        let _ = std::fs::remove_file(&db);
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE credential (id text PRIMARY KEY, integration_id text, label text NOT NULL,
               value text NOT NULL, connector_id text, method_id text, active integer,
               time_created integer NOT NULL, time_updated integer NOT NULL);
             INSERT INTO credential VALUES ('a','opencode-go','old','{\"type\":\"api\",\"key\":\"sk-old\"}',NULL,NULL,0,1,1);
             INSERT INTO credential VALUES ('b','opencode-go','new','{\"type\":\"key\",\"key\":\"sk-new\"}',NULL,NULL,1,2,2);
             INSERT INTO credential VALUES ('c','other','x','{\"type\":\"api\",\"key\":\"sk-other\"}',NULL,NULL,1,3,3);",
        )
        .unwrap();
        drop(conn);
        assert_eq!(key_from_db(&db).unwrap().as_deref(), Some("sk-new"));

        let auth = dir.join("auth.json");
        std::fs::write(
            &auth,
            r#"{"opencode-go":{"type":"api","key":"sk-v1"},"anthropic":{"type":"oauth"}}"#,
        )
        .unwrap();
        assert_eq!(key_from_auth_json(&auth).unwrap().as_deref(), Some("sk-v1"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
