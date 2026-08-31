//! Cursor usage fetcher. Credentials are read from the official CLI or the
//! Cursor desktop app and are never cached or sent to the renderer. The
//! dashboard does not refresh Cursor tokens or write credential files. The
//! usage-summary cookie uses the same `userId%3A%3Atoken` encoding as CodexBar
//! and cursor.com; a 401/403 retries the decoded `::` form and the next local
//! login. The usage-summary and GetSandUsageStatus endpoints are not public
//! APIs, so incompatible responses degrade to last-known-good sanitized data.
//! Grok Bot quota is attached from GetSandUsageStatus on the same login; a
//! missing or failed Bot response does not fail the Cursor panel.

use super::{require_success, send_with_one_retry, FetchError, MSG_LOGIN_REQUIRED};
use crate::config::Config;
use crate::models::CursorService;
use crate::util::{clamp_percent, local_label};
use anyhow::{anyhow, bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use reqwest::{Client, RequestBuilder};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Stdio};
use std::time::Duration as StdDuration;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Instant;

const USAGE_URL: &str = "https://cursor.com/api/usage-summary";
const SAND_USAGE_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus";
const LOGIN_REQUIRED: &str = MSG_LOGIN_REQUIRED;
const SESSION_EXPIRED: &str = "SESSION EXPIRED";
const AUTH_READ_TIMEOUT: StdDuration = StdDuration::from_secs(16);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYCHAIN_TIMEOUT: StdDuration = StdDuration::from_secs(8);
const MAX_AUTH_BYTES: usize = 64 * 1024;
const MAX_JWT_PAYLOAD_BYTES: usize = 16 * 1024;
const TOKEN_EXPIRY_MARGIN_SECS: i64 = 60;
const ITEM_ACCESS_TOKEN: &str = "cursorAuth/accessToken";
#[cfg(any(test, target_os = "macos", target_os = "linux"))]
const HELPER_NOT_FOUND_EXIT_CODE: i32 = 44;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYCHAIN_ACCOUNT: &str = "cursor-user";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYCHAIN_ACCESS_TOKEN_SERVICE: &str = "cursor-access-token";

struct CursorAuth {
    token: String,
    user_id: String,
}

enum AuthState {
    Active(CursorAuth),
    Expired,
}

#[derive(Debug)]
enum AuthError {
    Missing,
    Expired,
    Invalid,
}

impl From<AuthError> for FetchError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Missing => FetchError::Auth {
                message: LOGIN_REQUIRED,
            },
            AuthError::Expired => FetchError::Auth {
                message: SESSION_EXPIRED,
            },
            AuthError::Invalid => FetchError::Auth {
                message: super::MSG_CREDENTIAL_ERROR,
            },
        }
    }
}

fn cursor_http_client(timeout_seconds: u64) -> anyhow::Result<Client> {
    // cursor.com sits behind a WAF that has 403'd the shared rustls client
    // while the same cookie succeeds with OpenSSL/SecureTransport/SChannel.
    Ok(Client::builder()
        .timeout(StdDuration::from_secs(timeout_seconds))
        .connect_timeout(StdDuration::from_secs(timeout_seconds))
        .user_agent(super::BROWSER_UA)
        .http1_only()
        .use_native_tls()
        .build()?)
}

fn is_retryable_cursor_auth(error: &FetchError) -> bool {
    match error {
        FetchError::Auth { message }
            if message == &super::MSG_AUTH_EXPIRED || message == &super::MSG_AUTH_FORBIDDEN =>
        {
            true
        }
        FetchError::Other(_) => true,
        FetchError::Auth { .. } | FetchError::RateLimited { .. } => false,
    }
}

pub async fn fetch(config: &Config, _shared: &Client) -> Result<CursorService, FetchError> {
    let client = cursor_http_client(config.network_timeout_seconds).map_err(FetchError::Other)?;
    let configured = config.cursor_credentials_path.trim().to_string();
    let auths = match read_auth_candidates(configured).await {
        Ok(auths) => auths,
        Err(error) => return Err(error.into()),
    };

    let mut last_auth_error: Option<FetchError> = None;
    let mut selected: Option<(CursorAuth, super::Resp)> = None;
    for auth in auths {
        let mut accepted: Option<super::Resp> = None;
        for cookie in cookie_header_candidates(&auth) {
            let resp = send_with_one_retry(|| usage_request(&client, &cookie))
                .await
                .map_err(FetchError::Other)?;
            match require_success(&resp, "Cursor usage") {
                Ok(()) => {
                    accepted = Some(resp);
                    break;
                }
                Err(error) if is_retryable_cursor_auth(&error) => {
                    last_auth_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(resp) = accepted {
            selected = Some((auth, resp));
            break;
        }
    }
    let Some((auth, resp)) = selected else {
        return Err(last_auth_error.unwrap_or(FetchError::Auth {
            message: LOGIN_REQUIRED,
        }));
    };

    let sand_client = client.clone();
    let sand_token = auth.token.clone();
    let sand_task = tokio::spawn(async move {
        send_with_one_retry(|| sand_usage_request(&sand_client, &sand_token)).await
    });
    let mut service = parse_usage(&resp.body).map_err(FetchError::Other)?;
    let sand_res = match sand_task.await {
        Ok(Ok(sand)) => Some(sand),
        _ => None,
    };
    apply_sand_usage(sand_res.as_ref(), &mut service);
    Ok(service)
}

fn usage_request(client: &Client, cookie: &str) -> RequestBuilder {
    // 0.8.2's working Surface request sent Origin on this GET. CodexBar omits it;
    // keep Origin so a rustls/WAF path that requires it still matches 0.8.2.
    client
        .get(USAGE_URL)
        .header("Accept", "application/json")
        .header("Cookie", cookie)
        .header("Origin", "https://cursor.com")
}

pub(crate) fn parse_usage(body: &str) -> anyhow::Result<CursorService> {
    let root: Value = serde_json::from_str(body).context("parse Cursor usage-summary")?;
    let plan = root
        .get("membershipType")
        .and_then(Value::as_str)
        .and_then(canonical_plan_label)
        .map(str::to_string);
    let reset_local = root.get("billingCycleEnd").and_then(local_label);
    let individual = root.get("individualUsage");
    let team = root.get("teamUsage");

    let plan_usage = individual.and_then(|value| value.get("plan"));
    let overall = individual.and_then(|value| value.get("overall"));
    let pooled = team.and_then(|value| value.get("pooled"));
    let on_demand = individual.and_then(|value| value.get("onDemand"));

    let api_percent = api_percent(plan_usage);
    let auto = auto_percent(plan_usage);
    let included_percent = if auto.is_some() || api_percent.is_some() {
        plan_included_percent(plan_usage)
    } else {
        None
    };
    let usage_percent = auto
        .or_else(|| percent_from_usage(plan_usage))
        .or_else(|| percent_from_usage(overall))
        .or_else(|| percent_from_usage(pooled));
    let on_demand_percent = on_demand_percent(on_demand);

    if usage_percent.is_none()
        && included_percent.is_none()
        && api_percent.is_none()
        && on_demand_percent.is_none()
    {
        bail!("Cursor usage-summary returned no usable usage windows");
    }

    Ok(CursorService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        plan,
        usage_percent,
        usage_reset_local: reset_local,
        included_percent,
        api_percent,
        on_demand_percent,
        grok_bot_percent: None,
        grok_bot_reset_local: None,
    })
}

fn sand_usage_request(client: &Client, token: &str) -> RequestBuilder {
    client
        .post(SAND_USAGE_URL)
        .header("Content-Type", "application/json")
        .header("connect-protocol-version", "1")
        .header("Authorization", format!("Bearer {token}"))
        .header("x-cursor-client-type", "sand")
        .header("x-cursor-client-version", "0.24.0")
        .body("{}")
}

fn apply_sand_usage(resp: Option<&super::Resp>, service: &mut CursorService) {
    let Some(resp) = resp else {
        return;
    };
    if !resp.is_success() {
        return;
    }
    if let Some((percent, reset)) = parse_sand_usage(&resp.body) {
        service.grok_bot_percent = Some(percent);
        service.grok_bot_reset_local = reset;
    }
}

pub(crate) fn parse_sand_usage(body: &str) -> Option<(f64, Option<String>)> {
    let root: Value = serde_json::from_str(body).ok()?;
    let percent = root
        .get("usagePercent")
        .or_else(|| root.get("usage_percent"))
        .and_then(number_value)
        .map(clamp_percent)?;
    let reset = root
        .get("nextResetTimestampUtc")
        .or_else(|| root.get("next_reset_timestamp_utc"))
        .and_then(local_label);
    Some((percent, reset))
}

fn auto_percent(usage: Option<&Value>) -> Option<f64> {
    usage
        .and_then(|value| value.get("autoPercentUsed"))
        .and_then(number_value)
        .map(clamp_percent)
}

fn plan_included_percent(usage: Option<&Value>) -> Option<f64> {
    usage
        .and_then(|value| value.get("totalPercentUsed"))
        .and_then(number_value)
        .map(clamp_percent)
}

fn api_percent(usage: Option<&Value>) -> Option<f64> {
    usage
        .and_then(|value| value.get("apiPercentUsed"))
        .and_then(number_value)
        .map(clamp_percent)
}

fn percent_from_usage(usage: Option<&Value>) -> Option<f64> {
    let usage = usage?;
    if let Some(percent) = usage.get("totalPercentUsed").and_then(number_value) {
        return Some(clamp_percent(percent));
    }
    if let Some(percent) = usage.get("autoPercentUsed").and_then(number_value) {
        return Some(clamp_percent(percent));
    }

    let used = usage.get("used").and_then(number_value)?;
    let limit = usage.get("limit").and_then(number_value)?;
    if !limit.is_finite() || limit <= 0.0 || !used.is_finite() {
        return None;
    }
    Some(clamp_percent((used / limit) * 100.0))
}

fn on_demand_percent(on_demand: Option<&Value>) -> Option<f64> {
    let on_demand = on_demand?;
    let enabled = on_demand
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    let used = on_demand.get("used").and_then(number_value)?;
    let limit = on_demand.get("limit").and_then(number_value)?;
    if !limit.is_finite() || limit <= 0.0 || !used.is_finite() {
        return None;
    }
    Some(clamp_percent((used / limit) * 100.0))
}

fn canonical_plan_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" | "hobby" => Some("Hobby"),
        "pro" => Some("Pro"),
        "pro_plus" | "proplus" | "pro+" => Some("Pro+"),
        "ultra" => Some("Ultra"),
        "business" | "team" => Some("Team"),
        "enterprise" => Some("Enterprise"),
        _ => None,
    }
}

fn number_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.as_u64().map(|number| number as f64))
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|number| number.is_finite())
}

fn cookie_header(auth: &CursorAuth) -> String {
    cookie_header_encoded(auth, true)
}

fn cookie_header_encoded(auth: &CursorAuth, percent_encode_separator: bool) -> String {
    let separator = if percent_encode_separator {
        "%3A%3A"
    } else {
        "::"
    };
    format!(
        "WorkosCursorSessionToken={user_id}{separator}{token}",
        user_id = auth.user_id,
        token = auth.token
    )
}

fn cookie_header_candidates(auth: &CursorAuth) -> [String; 2] {
    [cookie_header_encoded(auth, false), cookie_header(auth)]
}

async fn read_auth_candidates(configured: String) -> Result<Vec<CursorAuth>, AuthError> {
    tokio::time::timeout(
        AUTH_READ_TIMEOUT,
        tokio::task::spawn_blocking(move || load_auth_candidates(&configured)),
    )
    .await
    .map_err(|_| AuthError::Invalid)?
    .map_err(|_| AuthError::Invalid)?
}

fn load_auth_candidates(configured: &str) -> Result<Vec<CursorAuth>, AuthError> {
    if !configured.is_empty() {
        return match load_configured_auth(configured) {
            Ok(AuthState::Active(auth)) => Ok(vec![auth]),
            Ok(AuthState::Expired) => Err(AuthError::Expired),
            Err(error) => Err(error),
        };
    }

    let mut saw_expired = false;
    let mut saw_invalid = false;
    let mut active = Vec::new();
    for path in cli_auth_paths() {
        collect_active_auth(
            load_auth_file(&path),
            &mut saw_expired,
            &mut saw_invalid,
            &mut active,
        );
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    collect_active_auth(
        load_keychain_auth(),
        &mut saw_expired,
        &mut saw_invalid,
        &mut active,
    );
    if let Some(path) = desktop_vscdb_path() {
        collect_active_auth(
            load_vscdb_auth(&path),
            &mut saw_expired,
            &mut saw_invalid,
            &mut active,
        );
    }
    if !active.is_empty() {
        Ok(active)
    } else {
        Err(match finish_auth_search(saw_expired, saw_invalid) {
            Err(error) => error,
            Ok(_) => AuthError::Missing,
        })
    }
}

fn collect_active_auth(
    result: Result<AuthState, AuthError>,
    saw_expired: &mut bool,
    saw_invalid: &mut bool,
    active: &mut Vec<CursorAuth>,
) {
    if let Some(auth) = consider_auth_source(result, saw_expired, saw_invalid) {
        push_unique_auth(active, auth);
    }
}

fn push_unique_auth(active: &mut Vec<CursorAuth>, auth: CursorAuth) {
    if active
        .iter()
        .any(|existing| existing.token == auth.token && existing.user_id == auth.user_id)
    {
        return;
    }
    active.push(auth);
}

fn consider_auth_source(
    result: Result<AuthState, AuthError>,
    saw_expired: &mut bool,
    saw_invalid: &mut bool,
) -> Option<CursorAuth> {
    match result {
        Ok(AuthState::Active(auth)) => Some(auth),
        Ok(AuthState::Expired) => {
            *saw_expired = true;
            None
        }
        Err(AuthError::Missing) => None,
        Err(AuthError::Expired) => {
            *saw_expired = true;
            None
        }
        Err(AuthError::Invalid) => {
            *saw_invalid = true;
            None
        }
    }
}

fn finish_auth_search(saw_expired: bool, saw_invalid: bool) -> Result<AuthState, AuthError> {
    if saw_expired {
        Err(AuthError::Expired)
    } else if saw_invalid {
        Err(AuthError::Invalid)
    } else {
        Err(AuthError::Missing)
    }
}

#[cfg(test)]
fn select_auth_state(
    sources: impl IntoIterator<Item = Result<AuthState, AuthError>>,
) -> Result<AuthState, AuthError> {
    let mut saw_expired = false;
    let mut saw_invalid = false;
    for source in sources {
        if let Some(auth) = consider_auth_source(source, &mut saw_expired, &mut saw_invalid) {
            return Ok(AuthState::Active(auth));
        }
    }
    finish_auth_search(saw_expired, saw_invalid)
}

#[cfg(any(test, target_os = "macos", target_os = "linux"))]
fn helper_status_is_missing(code: Option<i32>) -> bool {
    code == Some(HELPER_NOT_FOUND_EXIT_CODE) || cfg!(target_os = "linux") && code == Some(1)
}

fn load_configured_auth(configured: &str) -> Result<AuthState, AuthError> {
    let path = Path::new(configured);
    if is_vscdb_path(path) {
        load_vscdb_auth(path)
    } else {
        load_auth_file(path)
    }
}

fn load_auth_file(path: &Path) -> Result<AuthState, AuthError> {
    let text = read_file_limited(path).map_err(|error| {
        if is_missing_io(&error) {
            AuthError::Missing
        } else {
            AuthError::Invalid
        }
    })?;
    parse_auth_text(&text)
}

fn parse_auth_text(text: &str) -> Result<AuthState, AuthError> {
    let root: Value = serde_json::from_str(text).map_err(|_| AuthError::Invalid)?;
    let token = root
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(AuthError::Missing)?;
    parse_access_token(token)
}

fn split_combined_session(raw: &str) -> (Option<&str>, &str) {
    let raw = raw.trim();
    if let Some((user_id, token)) = raw.split_once("%3A%3A") {
        (Some(user_id.trim()), token.trim())
    } else if let Some((user_id, token)) = raw.split_once("::") {
        (Some(user_id.trim()), token.trim())
    } else {
        (None, raw)
    }
}

fn parse_access_token(raw: &str) -> Result<AuthState, AuthError> {
    let (user_id, token) = split_combined_session(raw);
    if token.is_empty() {
        return Err(AuthError::Invalid);
    }

    let claims = jwt_claims(token).ok_or(AuthError::Invalid)?;
    let exp = claims
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or(AuthError::Invalid)?;
    let now = Utc::now().timestamp();
    if exp <= now + TOKEN_EXPIRY_MARGIN_SECS {
        return Ok(AuthState::Expired);
    }

    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::Invalid)?;
    let derived_user_id = subject
        .rsplit('|')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::Invalid)?;
    let user_id = user_id.unwrap_or(derived_user_id);
    if !is_safe_user_id(user_id) {
        return Err(AuthError::Invalid);
    }

    Ok(AuthState::Active(CursorAuth {
        token: token.to_string(),
        user_id: user_id.to_string(),
    }))
}

fn jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty()
        || payload.is_empty()
        || signature.is_empty()
        || payload.len() > MAX_JWT_PAYLOAD_BYTES
    {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    if decoded.len() > MAX_JWT_PAYLOAD_BYTES {
        return None;
    }
    serde_json::from_slice(&decoded).ok()
}

fn is_safe_user_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn cli_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(path) = dirs::home_dir().map(|home| home.join(".cursor").join("auth.json")) {
        paths.push(path);
    }
    #[cfg(windows)]
    if let Some(path) = dirs::config_dir().map(|dir| dir.join("Cursor").join("auth.json")) {
        paths.push(path);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        if let Some(path) = dirs::config_dir().map(|dir| dir.join("cursor").join("auth.json")) {
            paths.push(path);
        }
        if let Some(path) = dirs::home_dir().map(|home| home.join(".cursor").join("auth.json")) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

fn desktop_vscdb_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb"),
    )
}

fn is_vscdb_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vscdb"))
}

fn load_vscdb_auth(path: &Path) -> Result<AuthState, AuthError> {
    if !path.is_file() {
        return Err(AuthError::Missing);
    }
    let token = read_vscdb_access_token(path).map_err(|_| AuthError::Invalid)?;
    let Some(token) = token else {
        return Err(AuthError::Missing);
    };
    parse_access_token(&token)
}

fn read_vscdb_access_token(path: &Path) -> anyhow::Result<Option<String>> {
    let wal = path.with_file_name("state.vscdb-wal");
    let shm = path.with_file_name("state.vscdb-shm");
    let has_sidecars = wal.exists() || shm.exists();
    match query_vscdb_token(path, false) {
        Ok(value) => Ok(value),
        Err(error) if !has_sidecars => query_vscdb_token(path, true).or(Err(error)),
        Err(error) => Err(error),
    }
}

fn query_vscdb_token(path: &Path, immutable: bool) -> anyhow::Result<Option<String>> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = if immutable {
        Connection::open_with_flags(sqlite_file_uri(path, "mode=ro&immutable=1"), flags)
    } else {
        Connection::open_with_flags(sqlite_file_uri(path, "mode=ro"), flags)
    }
    .with_context(|| format!("open Cursor state db at {}", path.display()))?;
    conn.busy_timeout(StdDuration::from_millis(250))
        .context("set Cursor state db busy timeout")?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1")
        .context("prepare Cursor state db query")?;
    let mut rows = stmt
        .query([ITEM_ACCESS_TOKEN])
        .context("query Cursor access token")?;
    let Some(row) = rows.next().context("read Cursor access token row")? else {
        return Ok(None);
    };
    decode_sqlite_value(row).map(Some)
}

fn decode_sqlite_value(row: &rusqlite::Row<'_>) -> anyhow::Result<String> {
    if let Ok(text) = row.get::<_, String>(0) {
        return normalize_stored_token(&text)
            .ok_or_else(|| anyhow!("Cursor desktop token is empty"));
    }
    let bytes: Vec<u8> = row.get(0).context("read Cursor desktop token blob")?;
    let text = String::from_utf8(bytes).context("Cursor desktop token is not UTF-8")?;
    normalize_stored_token(&text).ok_or_else(|| anyhow!("Cursor desktop token is empty"))
}

fn normalize_stored_token(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('"') {
        return serde_json::from_str::<String>(trimmed)
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
    }
    Some(trimmed.to_string())
}

fn sqlite_file_uri(path: &Path, query: &str) -> String {
    let mut rendered = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows)
        && rendered.len() >= 2
        && rendered.as_bytes().get(1) == Some(&b':')
        && !rendered.starts_with('/')
    {
        rendered.insert(0, '/');
    }
    let encoded = rendered
        .replace('%', "%25")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace(' ', "%20");
    format!("file:{encoded}?{query}")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn load_keychain_auth() -> Result<AuthState, AuthError> {
    let token = read_os_keychain_password(KEYCHAIN_ACCOUNT, KEYCHAIN_ACCESS_TOKEN_SERVICE)?;
    parse_access_token(&token)
}

#[cfg(target_os = "macos")]
fn read_os_keychain_password(account: &str, service: &str) -> Result<String, AuthError> {
    let mut command = Command::new("/usr/bin/security");
    command.args(["find-generic-password", "-a", account, "-s", service, "-w"]);
    read_keychain_command_password(command)
}

#[cfg(target_os = "linux")]
fn read_os_keychain_password(account: &str, service: &str) -> Result<String, AuthError> {
    let mut command = Command::new("secret-tool");
    command.args(["lookup", "service", service, "account", account]);
    read_keychain_command_password(command)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_keychain_command_password(command: Command) -> Result<String, AuthError> {
    let output = run_command_capture(command, KEYCHAIN_TIMEOUT).map_err(|error| {
        if is_missing_io(&error) {
            AuthError::Missing
        } else {
            AuthError::Invalid
        }
    })?;
    let token = output.trim();
    if token.is_empty() {
        Err(AuthError::Missing)
    } else {
        Ok(token.to_string())
    }
}

fn read_file_limited(path: &Path) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("read Cursor auth at {}", path.display()))?;
    read_bounded_utf8(file)
}

fn read_bounded_utf8(reader: impl Read) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_AUTH_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read Cursor credential data")?;
    if bytes.len() > MAX_AUTH_BYTES {
        bail!("Cursor credential data exceeds {MAX_AUTH_BYTES} bytes");
    }
    String::from_utf8(bytes).context("Cursor credential data is not UTF-8")
}

fn is_missing_io(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_command_capture(mut command: Command, timeout: StdDuration) -> anyhow::Result<String> {
    hide_command_window(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("start Cursor credential helper")?;
    let stdout = child
        .stdout
        .take()
        .context("capture Cursor credential helper output")?;
    let reader = std::thread::spawn(move || read_bounded_utf8(stdout));
    let deadline = Instant::now() + timeout;

    let status = loop {
        if let Some(status) = child.try_wait().context("poll Cursor credential helper")? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("Cursor credential helper timed out");
        }
        std::thread::sleep(StdDuration::from_millis(25));
    };

    let text = reader
        .join()
        .map_err(|_| anyhow!("join Cursor credential helper output"))??;
    if !status.success() {
        if helper_status_is_missing(status.code()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Cursor credential helper found no login",
            )
            .into());
        }
        bail!("Cursor credential helper failed");
    }
    Ok(text)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn hide_command_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::{
        apply_sand_usage, canonical_plan_label, collect_active_auth, cookie_header,
        cookie_header_candidates, helper_status_is_missing, is_safe_user_id, is_vscdb_path,
        jwt_claims, load_auth_file, load_configured_auth, load_vscdb_auth, normalize_stored_token,
        parse_access_token, parse_auth_text, parse_sand_usage, parse_usage, percent_from_usage,
        select_auth_state, sqlite_file_uri, AuthError, AuthState, CursorAuth, ITEM_ACCESS_TOKEN,
    };
    use crate::fetchers::Resp;
    use base64::Engine as _;
    use chrono::Utc;
    use rusqlite::Connection;
    use serde_json::json;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn encode_b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    fn test_jwt(sub: &str, exp: i64) -> String {
        let header = encode_b64(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = encode_b64(format!(r#"{{"sub":"{sub}","exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    fn future_exp() -> i64 {
        Utc::now().timestamp() + 3_600
    }

    fn active_token() -> String {
        test_jwt("auth0|user_01TESTCURSORTOKEN", future_exp())
    }

    #[test]
    fn parse_sand_usage_reads_percent_and_reset() {
        let parsed = parse_sand_usage(include_str!("../../../mocks/grok_bot_usage_normal.json"))
            .expect("sand usage");
        assert_eq!(parsed.0, 37.5);
        assert!(parsed.1.is_some());
        let snake = parse_sand_usage(
            r#"{"usage_percent":110,"next_reset_timestamp_utc":"2026-09-01T00:00:00Z"}"#,
        )
        .expect("snake_case sand usage");
        assert_eq!(snake.0, 100.0);
        assert!(snake.1.is_some());
        assert!(parse_sand_usage("{}").is_none());
        assert!(parse_sand_usage("not-json").is_none());
        assert!(parse_sand_usage(r#"{"nextResetTimestampUtc":"2026-09-01T00:00:00Z"}"#).is_none());
    }

    #[test]
    fn sand_failure_leaves_included_auto_and_api_alone() {
        let mut service = parse_usage(include_str!("../../../mocks/cursor_usage_normal.json"))
            .expect("bundled Cursor fixture");
        apply_sand_usage(None, &mut service);
        apply_sand_usage(
            Some(&Resp {
                status: 503,
                body: include_str!("../../../mocks/grok_bot_usage_normal.json").into(),
                retry_after: None,
            }),
            &mut service,
        );
        assert_eq!(service.usage_percent, Some(8.0));
        assert_eq!(service.included_percent, Some(24.0));
        assert_eq!(service.api_percent, Some(41.0));
        assert!(service.grok_bot_percent.is_none());

        apply_sand_usage(
            Some(&Resp {
                status: 200,
                body: include_str!("../../../mocks/grok_bot_usage_normal.json").into(),
                retry_after: None,
            }),
            &mut service,
        );
        assert_eq!(service.grok_bot_percent, Some(37.5));
        assert_eq!(service.usage_percent, Some(8.0));
        assert_eq!(service.included_percent, Some(24.0));
        assert_eq!(service.api_percent, Some(41.0));
    }

    #[test]
    fn parse_usage_keeps_auto_and_named_api_lanes_separate() {
        let service = parse_usage(include_str!("../../../mocks/cursor_usage_normal.json"))
            .expect("bundled Cursor fixture");

        assert_eq!(service.status, "NOMINAL");
        assert_eq!(service.plan.as_deref(), Some("Ultra"));
        assert_eq!(service.usage_percent, Some(8.0));
        assert_eq!(service.included_percent, Some(24.0));
        assert_eq!(service.api_percent, Some(41.0));
        assert!(service.on_demand_percent.is_none());
        assert!(service.usage_reset_local.is_some());
        assert!(service.grok_bot_percent.is_none());
        assert!(service.grok_bot_reset_local.is_none());
    }

    #[test]
    fn parse_usage_falls_back_to_used_limit_and_on_demand() {
        let service = parse_usage(
            r#"{
                "membershipType":"pro_plus",
                "billingCycleEnd":"2026-09-01T00:00:00Z",
                "individualUsage":{
                    "plan":{"used":2500,"limit":10000},
                    "onDemand":{"enabled":true,"used":250,"limit":1000}
                }
            }"#,
        )
        .expect("ratio-based Cursor usage");

        assert_eq!(service.plan.as_deref(), Some("Pro+"));
        assert_eq!(service.usage_percent, Some(25.0));
        assert_eq!(service.on_demand_percent, Some(25.0));
    }

    #[test]
    fn parse_usage_uses_overall_then_team_pool() {
        let overall = parse_usage(
            r#"{
                "membershipType":"enterprise",
                "individualUsage":{"overall":{"used":7384,"limit":20000}}
            }"#,
        )
        .expect("overall Cursor usage");
        assert_eq!(overall.plan.as_deref(), Some("Enterprise"));
        assert!((overall.usage_percent.unwrap_or_default() - 36.92).abs() < 0.01);

        let pooled = parse_usage(
            r#"{"membershipType":"team","teamUsage":{"pooled":{"used":10,"limit":40}}}"#,
        )
        .expect("pooled Cursor usage");
        assert_eq!(pooled.plan.as_deref(), Some("Team"));
        assert_eq!(pooled.usage_percent, Some(25.0));
    }

    #[test]
    fn parse_usage_uses_auto_lane_when_total_is_missing() {
        let percent = percent_from_usage(Some(&json!({
            "autoPercentUsed": 10.0,
            "apiPercentUsed": 30.0
        })));
        assert_eq!(percent, Some(10.0));
    }

    #[test]
    fn parse_usage_clamps_over_limit_and_rejects_empty_bodies() {
        let over = parse_usage(r#"{"individualUsage":{"plan":{"used":120,"limit":100}}}"#)
            .expect("over-limit Cursor usage");
        assert_eq!(over.usage_percent, Some(100.0));

        for body in ["{}", r#"{"individualUsage":{}}"#, "not-json"] {
            assert!(parse_usage(body).is_err(), "accepted {body}");
        }
    }

    #[test]
    fn plan_labels_are_allowlisted() {
        assert_eq!(canonical_plan_label("ultra"), Some("Ultra"));
        assert_eq!(canonical_plan_label("PRO+"), Some("Pro+"));
        assert_eq!(canonical_plan_label("hobby"), Some("Hobby"));
        assert_eq!(canonical_plan_label("mystery"), None);
    }

    #[test]
    fn access_token_becomes_session_cookie_without_keeping_refresh_material() {
        let token = active_token();
        let state = parse_auth_text(&format!(
            r#"{{"accessToken":"{token}","refreshToken":"do-not-keep"}}"#
        ))
        .expect("usable Cursor auth.json");
        let AuthState::Active(auth) = state else {
            panic!("expected active Cursor session");
        };
        assert_eq!(auth.user_id, "user_01TESTCURSORTOKEN");
        assert_eq!(
            cookie_header(&auth),
            format!("WorkosCursorSessionToken=user_01TESTCURSORTOKEN%3A%3A{token}")
        );
        assert!(jwt_claims(&auth.token).is_some());
        assert!(is_safe_user_id(&auth.user_id));
    }

    #[test]
    fn expired_or_unsafe_tokens_fail_closed() {
        let expired = test_jwt("auth0|user_01TESTCURSORTOKEN", Utc::now().timestamp() - 10);
        assert!(matches!(
            parse_access_token(&expired),
            Ok(AuthState::Expired)
        ));
        assert!(parse_access_token("not-a-jwt").is_err());
        assert!(!is_safe_user_id("user/../escape"));
        assert!(!is_safe_user_id(""));
    }

    #[test]
    fn unexpired_cli_session_does_not_hide_a_later_distinct_login() {
        let first = test_jwt("auth0|user_01FIRSTTOKENAAAAA", future_exp());
        let second = test_jwt("auth0|user_01SECONDTOKENAAAA", future_exp());
        let mut saw_expired = false;
        let mut saw_invalid = false;
        let mut active = Vec::new();
        collect_active_auth(
            parse_access_token(&first),
            &mut saw_expired,
            &mut saw_invalid,
            &mut active,
        );
        collect_active_auth(
            parse_access_token(&second),
            &mut saw_expired,
            &mut saw_invalid,
            &mut active,
        );
        collect_active_auth(
            parse_access_token(&first),
            &mut saw_expired,
            &mut saw_invalid,
            &mut active,
        );
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].token, first);
        assert_eq!(active[1].token, second);
    }

    #[test]
    fn expired_cli_session_does_not_block_a_later_active_source() {
        let expired = test_jwt("auth0|user_01EXPIREDTOKENAAA", Utc::now().timestamp() - 10);
        let active = active_token();
        let selected = select_auth_state([
            parse_access_token(&expired),
            Err(AuthError::Missing),
            parse_access_token(&active),
        ])
        .expect("later desktop session should win");
        let AuthState::Active(auth) = selected else {
            panic!("expected active session after expired CLI token");
        };
        assert_eq!(auth.token, active);
    }

    #[test]
    fn all_expired_sources_surface_session_expired() {
        let expired = test_jwt("auth0|user_01EXPIREDTOKENAAA", Utc::now().timestamp() - 10);
        assert!(matches!(
            select_auth_state([
                parse_access_token(&expired),
                Err(AuthError::Missing),
                parse_access_token(&expired),
            ]),
            Err(AuthError::Expired)
        ));
    }

    #[test]
    fn helper_not_found_is_login_required_not_credential_error() {
        assert!(helper_status_is_missing(Some(44)));
        assert_eq!(helper_status_is_missing(Some(1)), cfg!(target_os = "linux"));
        assert!(!helper_status_is_missing(None));
    }

    #[test]
    fn expired_cli_auth_file_falls_through_to_desktop_vscdb() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-cursor-fallback-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated Cursor fallback dir");
        let auth_path = dir.join("auth.json");
        let vscdb_path = dir.join("state.vscdb");
        let expired = test_jwt("auth0|user_01EXPIREDTOKENAAA", Utc::now().timestamp() - 10);
        let active = active_token();
        std::fs::write(&auth_path, format!(r#"{{"accessToken":"{expired}"}}"#))
            .expect("write expired CLI auth");
        let conn = Connection::open(&vscdb_path).expect("create fallback vscdb");
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB)",
            [],
        )
        .expect("create ItemTable");
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            rusqlite::params![ITEM_ACCESS_TOKEN, active.as_str()],
        )
        .expect("insert desktop token");
        drop(conn);

        let selected =
            select_auth_state([load_auth_file(&auth_path), load_vscdb_auth(&vscdb_path)])
                .expect("desktop session after expired CLI file");
        let AuthState::Active(auth) = selected else {
            panic!("expected active desktop session");
        };
        assert_eq!(auth.token, active);
        std::fs::remove_dir_all(dir).expect("remove isolated Cursor fallback dir");
    }

    #[test]
    fn combined_session_value_reuses_embedded_user_id() {
        let token = active_token();
        let AuthState::Active(auth) =
            parse_access_token(&format!("user_from_cookie::{token}")).expect("combined token")
        else {
            panic!("expected active session");
        };
        assert_eq!(auth.user_id, "user_from_cookie");
        assert_eq!(auth.token, token);
    }

    #[test]
    fn combined_session_accepts_percent_encoded_separator() {
        let token = active_token();
        let AuthState::Active(auth) = parse_access_token(&format!("user_from_cookie%3A%3A{token}"))
            .expect("percent-encoded combined token")
        else {
            panic!("expected active session");
        };
        assert_eq!(auth.user_id, "user_from_cookie");
        assert_eq!(auth.token, token);
    }

    #[test]
    fn stored_tokens_tolerate_json_quotes() {
        assert_eq!(
            normalize_stored_token("  \"abc.def.ghi\"  ").as_deref(),
            Some("abc.def.ghi")
        );
        assert_eq!(
            normalize_stored_token("abc.def.ghi").as_deref(),
            Some("abc.def.ghi")
        );
        assert!(normalize_stored_token("   ").is_none());
    }

    #[test]
    fn vscdb_reader_loads_desktop_access_token() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-cursor-vscdb-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated Cursor db dir");
        let path = dir.join("state.vscdb");
        let token = active_token();
        let conn = Connection::open(&path).expect("create test vscdb");
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB)",
            [],
        )
        .expect("create ItemTable");
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            rusqlite::params![ITEM_ACCESS_TOKEN, token.as_str()],
        )
        .expect("insert access token");
        drop(conn);

        let AuthState::Active(auth) = load_vscdb_auth(&path).expect("read desktop token") else {
            panic!("expected active desktop session");
        };
        assert_eq!(auth.token, token);
        assert!(is_vscdb_path(&path));
        std::fs::remove_dir_all(dir).expect("remove isolated Cursor db dir");
    }

    #[test]
    fn configured_auth_json_is_fail_closed() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-cursor-auth-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create isolated Cursor auth dir");
        let path = dir.join("auth.json");
        std::fs::write(&path, format!(r#"{{"accessToken":"{}"}}"#, active_token()))
            .expect("write Cursor auth.json");

        assert!(matches!(load_auth_file(&path), Ok(AuthState::Active(_))));
        assert!(matches!(
            load_configured_auth(path.to_str().expect("utf-8 path")),
            Ok(AuthState::Active(_))
        ));
        assert!(matches!(
            load_configured_auth(dir.join("missing.json").to_str().expect("utf-8 path")),
            Err(AuthError::Missing)
        ));
        std::fs::remove_dir_all(dir).expect("remove isolated Cursor auth dir");
    }

    #[test]
    fn sqlite_uri_encodes_spaces_and_windows_drives() {
        let unix = sqlite_file_uri(Path::new("/tmp/Application Support/state.vscdb"), "mode=ro");
        assert!(unix.contains("%20"));
        assert!(unix.starts_with("file:"));
        assert!(unix.contains("mode=ro"));
    }

    #[test]
    fn cookie_header_uses_codexbar_percent_encoding() {
        let auth = CursorAuth {
            token: "header.payload.sig".into(),
            user_id: "user_01ABC".into(),
        };
        assert_eq!(
            cookie_header(&auth),
            "WorkosCursorSessionToken=user_01ABC%3A%3Aheader.payload.sig"
        );
        let [raw, percent] = cookie_header_candidates(&auth);
        assert_eq!(
            raw,
            "WorkosCursorSessionToken=user_01ABC::header.payload.sig"
        );
        assert!(!raw.contains("%3A%3A"));
        assert_eq!(
            percent,
            "WorkosCursorSessionToken=user_01ABC%3A%3Aheader.payload.sig"
        );
    }
}
