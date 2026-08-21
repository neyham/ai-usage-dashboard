//! Antigravity Gemini quota fetcher. Credentials are read from the official
//! `agy` CLI OAuth file or, when that file is absent, from the OS keyring
//! entry `agy` writes (`service=gemini`, `account=antigravity`). They are
//! never sent to the renderer. An expired access token may be refreshed in
//! memory through Google's token endpoint. The OAuth client is never baked
//! into this repository: it comes from `ANTIGRAVITY_OAUTH_CLIENT_ID` /
//! `ANTIGRAVITY_OAUTH_CLIENT_SECRET`, `client_id`/`client_secret` on the local
//! login JSON, or a scan of an installed `agy` / Antigravity.app (same approach
//! as CodexBar). The dashboard does not write the credential file or keyring item.
//! Quota endpoints are not public APIs, so incompatible responses degrade to
//! last-known-good sanitized data.

use super::{require_success, send, send_with_one_retry, FetchError, MSG_LOGIN_REQUIRED};
use crate::config::Config;
use crate::models::AntigravityService;
use crate::util::{clamp_percent, local_label, parse_datetime_str};
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::time::Duration as StdDuration;
#[cfg(not(windows))]
use std::time::Instant;

const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const ASSIST_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const LOGIN_REQUIRED: &str = MSG_LOGIN_REQUIRED;
const SESSION_EXPIRED: &str = "SESSION EXPIRED";
const AUTH_READ_TIMEOUT: StdDuration = StdDuration::from_secs(16);
#[cfg(not(windows))]
const KEYRING_TIMEOUT: StdDuration = StdDuration::from_secs(8);
const TOKEN_EXPIRY_MARGIN_SECS: i64 = 60;
const MAX_AUTH_BYTES: usize = 64 * 1024;
const MAX_OAUTH_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const KEYRING_SERVICE: &str = "gemini";
const KEYRING_ACCOUNT: &str = "antigravity";
#[cfg(not(windows))]
const HELPER_NOT_FOUND_EXIT_CODE: i32 = 44;
#[cfg(target_os = "linux")]
const LINUX_SECRET_TOOL_MISSING_EXIT_CODE: i32 = 1;
#[cfg(target_os = "linux")]
const LINUX_LIBSECRET_MISSING_EXIT_CODE: i32 = 2;
#[cfg(target_os = "linux")]
const LINUX_KEYRING_PY: &str = r#"
import sys
try:
    import gi
    gi.require_version("Secret", "1")
    from gi.repository import Secret
except Exception:
    sys.exit(2)
schema = Secret.Schema.new(
    "org.freedesktop.Secret.Generic",
    Secret.SchemaFlags.NONE,
    {
        "service": Secret.SchemaAttributeType.STRING,
        "username": Secret.SchemaAttributeType.STRING,
    },
)
value = Secret.password_lookup_sync(
    schema,
    {"service": "gemini", "username": "antigravity"},
    None,
)
if not value:
    sys.exit(2)
sys.stdout.write(value)
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
struct OAuthClient {
    client_id: String,
    client_secret: String,
}

struct AntigravityAuth {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    oauth_client_id: Option<String>,
    oauth_client_secret: Option<String>,
}

enum AuthState {
    Active(AntigravityAuth),
    Expired,
}

#[derive(Debug)]
enum AuthError {
    Missing,
    Invalid,
}

impl From<AuthError> for FetchError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Missing => FetchError::Auth {
                message: LOGIN_REQUIRED,
            },
            AuthError::Invalid => FetchError::Auth {
                message: super::MSG_CREDENTIAL_ERROR,
            },
        }
    }
}

pub async fn fetch(config: &Config, client: &Client) -> Result<AntigravityService, FetchError> {
    let configured = config.antigravity_credentials_path.trim().to_string();
    let mut auth = match read_auth(configured).await {
        Ok(AuthState::Active(auth)) => auth,
        Ok(AuthState::Expired) => {
            return Err(FetchError::Auth {
                message: SESSION_EXPIRED,
            });
        }
        Err(error) => return Err(error.into()),
    };

    if needs_refresh(&auth) {
        auth = refresh_access_token(client, auth).await?;
    }

    let quota = send_with_one_retry(|| quota_request(client, &auth.access_token))
        .await
        .map_err(FetchError::Other)?;
    if quota.status == 401 && auth.refresh_token.is_some() {
        auth = refresh_access_token(client, auth).await?;
        let retry = send_with_one_retry(|| quota_request(client, &auth.access_token))
            .await
            .map_err(FetchError::Other)?;
        require_success(&retry, "Antigravity quota")?;
        return service_from_quota_and_plan(client, &auth.access_token, &retry.body).await;
    }
    require_success(&quota, "Antigravity quota")?;
    service_from_quota_and_plan(client, &auth.access_token, &quota.body).await
}

async fn service_from_quota_and_plan(
    client: &Client,
    access_token: &str,
    quota_body: &str,
) -> Result<AntigravityService, FetchError> {
    let mut service = parse_usage(quota_body).map_err(FetchError::Other)?;
    if let Ok(plan) = fetch_plan(client, access_token).await {
        service.plan = plan;
    }
    Ok(service)
}

fn quota_request(client: &Client, token: &str) -> RequestBuilder {
    client
        .post(QUOTA_URL)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("User-Agent", "antigravity")
        .json(&serde_json::json!({}))
}

fn assist_request(client: &Client, token: &str) -> RequestBuilder {
    client
        .post(ASSIST_URL)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("User-Agent", "antigravity")
        .json(&serde_json::json!({
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        }))
}

async fn fetch_plan(client: &Client, token: &str) -> anyhow::Result<Option<String>> {
    let resp = send(assist_request(client, token)).await?;
    if !resp.is_success() {
        return Ok(None);
    }
    Ok(parse_plan(&resp.body))
}

pub(crate) fn parse_usage(body: &str) -> anyhow::Result<AntigravityService> {
    let root: Value = serde_json::from_str(body).context("parse Antigravity quota summary")?;
    let groups = root
        .get("groups")
        .or_else(|| root.get("response").and_then(|value| value.get("groups")))
        .and_then(Value::as_array)
        .context("Antigravity quota summary missing groups")?;

    let gemini = groups.iter().find(|group| {
        group_is_gemini(group.get("displayName").and_then(Value::as_str))
            || group
                .get("buckets")
                .and_then(Value::as_array)
                .is_some_and(|buckets| buckets.iter().any(bucket_is_gemini))
    });
    let Some(gemini) = gemini else {
        bail!("Antigravity quota summary has no Gemini group");
    };
    let buckets = gemini
        .get("buckets")
        .and_then(Value::as_array)
        .context("Gemini quota group has no buckets")?;

    let five_hour = buckets.iter().find(|bucket| bucket_window(bucket, "5h"));
    let weekly = buckets
        .iter()
        .find(|bucket| bucket_window(bucket, "weekly"));
    let five_hour_percent = five_hour.and_then(used_percent);
    let seven_day_percent = weekly.and_then(used_percent);
    if five_hour_percent.is_none() && seven_day_percent.is_none() {
        bail!("Gemini quota group has no usable usage windows");
    }

    Ok(AntigravityService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        plan: None,
        five_hour_percent,
        seven_day_percent,
        five_hour_reset_local: five_hour
            .and_then(|bucket| bucket.get("resetTime"))
            .and_then(local_label),
        seven_day_reset_local: weekly
            .and_then(|bucket| bucket.get("resetTime"))
            .and_then(local_label),
    })
}

fn parse_plan(body: &str) -> Option<String> {
    let root: Value = serde_json::from_str(body).ok()?;
    let current = root
        .get("currentTier")
        .and_then(|tier| tier.get("name"))
        .and_then(Value::as_str)
        .and_then(canonical_plan_label);
    let paid = root
        .get("paidTier")
        .and_then(|tier| tier.get("name"))
        .and_then(Value::as_str)
        .and_then(canonical_plan_label);
    current.or(paid).map(str::to_string)
}

fn canonical_plan_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "antigravity" => Some("Antigravity"),
        "free" | "free-tier" | "free tier" => Some("Free"),
        "google ai pro" | "ai pro" | "pro" => Some("Google AI Pro"),
        "google ai ultra" | "ai ultra" | "ultra" => Some("Google AI Ultra"),
        _ => None,
    }
}

fn group_is_gemini(name: Option<&str>) -> bool {
    name.is_some_and(|value| value.to_ascii_lowercase().contains("gemini"))
}

fn bucket_is_gemini(bucket: &Value) -> bool {
    bucket
        .get("bucketId")
        .and_then(Value::as_str)
        .is_some_and(|id| id.to_ascii_lowercase().starts_with("gemini"))
}

fn bucket_window(bucket: &Value, window: &str) -> bool {
    let id = bucket
        .get("bucketId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let declared = bucket
        .get("window")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match window {
        "5h" => declared == "5h" || id.ends_with("-5h") || id.contains("5h"),
        "weekly" => declared == "weekly" || id.ends_with("-weekly") || id.contains("weekly"),
        _ => false,
    }
}

fn used_percent(bucket: &Value) -> Option<f64> {
    let remaining = bucket
        .get("remainingFraction")
        .or_else(|| {
            bucket
                .get("remaining")
                .and_then(|value| value.get("remainingFraction"))
        })
        .and_then(number_value)?;
    if !remaining.is_finite() {
        return None;
    }
    Some(clamp_percent((1.0 - remaining) * 100.0))
}

fn number_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.as_u64().map(|number| number as f64))
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|number| number.is_finite())
}

fn needs_refresh(auth: &AntigravityAuth) -> bool {
    match auth.expires_at {
        Some(expires) => expires.timestamp() <= Utc::now().timestamp() + TOKEN_EXPIRY_MARGIN_SECS,
        None => false,
    }
}

async fn refresh_access_token(
    client: &Client,
    auth: AntigravityAuth,
) -> Result<AntigravityAuth, FetchError> {
    let refresh = auth
        .refresh_token
        .as_deref()
        .filter(|token| !token.is_empty())
        .ok_or(FetchError::Auth {
            message: SESSION_EXPIRED,
        })?;

    let clients = resolve_oauth_clients(&auth);
    if clients.is_empty() {
        return Err(FetchError::Auth {
            message: SESSION_EXPIRED,
        });
    }

    for client_pair in clients {
        let resp = send(
            client
                .post(TOKEN_URL)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&[
                    ("client_id", client_pair.client_id.as_str()),
                    ("client_secret", client_pair.client_secret.as_str()),
                    ("refresh_token", refresh),
                    ("grant_type", "refresh_token"),
                ]),
        )
        .await
        .map_err(FetchError::Other)?;
        if resp.status == 401 || resp.status == 400 {
            continue;
        }
        if !resp.is_success() {
            continue;
        }
        let root: Value = serde_json::from_str(&resp.body).map_err(|_| FetchError::Auth {
            message: SESSION_EXPIRED,
        })?;
        let access = root
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or(FetchError::Auth {
                message: SESSION_EXPIRED,
            })?;
        let expires_in = root
            .get("expires_in")
            .and_then(Value::as_i64)
            .unwrap_or(3500);
        return Ok(AntigravityAuth {
            access_token: access.to_string(),
            refresh_token: auth.refresh_token,
            expires_at: Some(Utc::now() + chrono::Duration::seconds(expires_in)),
            oauth_client_id: Some(client_pair.client_id),
            oauth_client_secret: Some(client_pair.client_secret),
        });
    }

    Err(FetchError::Auth {
        message: SESSION_EXPIRED,
    })
}

fn json_nonempty(objects: [&Value; 2], keys: &[&str]) -> Option<String> {
    for object in objects {
        for key in keys {
            if let Some(value) = object
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn env_oauth_client() -> Option<OAuthClient> {
    let client_id = std::env::var("ANTIGRAVITY_OAUTH_CLIENT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let client_secret = std::env::var("ANTIGRAVITY_OAUTH_CLIENT_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    Some(OAuthClient {
        client_id,
        client_secret,
    })
}

fn resolve_oauth_clients(auth: &AntigravityAuth) -> Vec<OAuthClient> {
    let mut clients = Vec::new();
    if let Some(client) = env_oauth_client() {
        push_unique_client(&mut clients, client);
    }
    if let (Some(client_id), Some(client_secret)) = (
        auth.oauth_client_id.clone(),
        auth.oauth_client_secret.clone(),
    ) {
        push_unique_client(
            &mut clients,
            OAuthClient {
                client_id,
                client_secret,
            },
        );
    }
    for client in discover_oauth_clients() {
        push_unique_client(&mut clients, client);
    }
    clients
}

fn push_unique_client(clients: &mut Vec<OAuthClient>, client: OAuthClient) {
    if !clients.iter().any(|existing| existing == &client) {
        clients.push(client);
    }
}

fn discover_oauth_clients() -> Vec<OAuthClient> {
    let mut clients = Vec::new();
    for path in oauth_artifact_candidates() {
        if let Some(client) = oauth_client_from_artifact(&path) {
            push_unique_client(&mut clients, client);
        }
    }
    clients
}

fn oauth_artifact_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = which_command("agy") {
        paths.push(path);
    }
    if let Some(path) = which_command("antigravity-cli") {
        paths.push(path);
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local").join("bin").join("agy"));
        paths.push(home.join("Applications").join("Antigravity.app"));
    }
    paths.extend(installed_antigravity_artifacts());
    paths
}

fn installed_antigravity_artifacts() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let roots = [
            PathBuf::from("/Applications"),
            dirs::home_dir().unwrap_or_default().join("Applications"),
        ];
        let relatives = [
            "Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm",
            "Contents/Resources/app/extensions/antigravity/bin/language_server_macos_x64",
            "Contents/Resources/app/extensions/antigravity/bin/language_server_macos",
            "Contents/Resources/app/out/main.js",
            "Contents/Resources/bin/language_server",
            "Contents/MacOS/agy",
        ];
        for root in roots {
            let bundle = root.join("Antigravity.app");
            for relative in relatives {
                paths.push(bundle.join(relative));
            }
        }
        paths.push(PathBuf::from("/opt/homebrew/bin/agy"));
        paths.push(PathBuf::from("/usr/local/bin/agy"));
    }
    #[cfg(target_os = "linux")]
    {
        paths.push(PathBuf::from("/usr/bin/agy"));
        paths.push(PathBuf::from("/usr/local/bin/agy"));
    }
    paths
}

fn which_command(name: &str) -> Option<PathBuf> {
    let path_os = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_os) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn oauth_client_from_artifact(path: &Path) -> Option<OAuthClient> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_OAUTH_ARTIFACT_BYTES {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_OAUTH_ARTIFACT_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    parse_oauth_client_from_bytes(&bytes)
}

fn parse_oauth_client_from_bytes(data: &[u8]) -> Option<OAuthClient> {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Some(client) = parse_oauth_client_from_text(text) {
            return Some(client);
        }
    }
    let client_id = find_ascii_client_id(data)?;
    let client_secret = find_ascii_client_secret(data)?;
    Some(OAuthClient {
        client_id,
        client_secret,
    })
}

fn parse_oauth_client_from_text(text: &str) -> Option<OAuthClient> {
    let client_id = find_text_client_id(text)?;
    let client_secret = find_text_client_secret(text)?;
    Some(OAuthClient {
        client_id,
        client_secret,
    })
}

fn find_text_client_id(text: &str) -> Option<String> {
    let needle = ".apps.googleusercontent.com";
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(needle) {
        let end = search_from + rel + needle.len();
        let prefix = &text[..search_from + rel];
        let start = prefix
            .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let candidate = &text[start..end];
        if candidate
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit())
            && candidate.contains('-')
        {
            return Some(candidate.to_string());
        }
        search_from = end;
    }
    None
}

fn find_text_client_secret(text: &str) -> Option<String> {
    let needle = "GOCSPX-";
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(needle) {
        let start = search_from + rel;
        let rest = &text[start + needle.len()..];
        let taken = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
            .count();
        if taken >= 20 {
            return Some(text[start..start + needle.len() + taken].to_string());
        }
        search_from = start + needle.len();
    }
    None
}

fn find_ascii_client_id(data: &[u8]) -> Option<String> {
    let needle = b".apps.googleusercontent.com";
    let mut search_from = 0;
    while let Some(rel) = find_bytes(&data[search_from..], needle) {
        let end = search_from + rel + needle.len();
        let mut start = search_from + rel;
        while start > 0 {
            let previous = data[start - 1];
            if is_oauth_id_byte(previous) {
                start -= 1;
            } else {
                break;
            }
        }
        if let Ok(candidate) = std::str::from_utf8(&data[start..end]) {
            if candidate
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_digit())
                && candidate.contains('-')
            {
                return Some(candidate.to_string());
            }
        }
        search_from = end;
    }
    None
}

fn find_ascii_client_secret(data: &[u8]) -> Option<String> {
    let needle = b"GOCSPX-";
    let mut search_from = 0;
    while let Some(rel) = find_bytes(&data[search_from..], needle) {
        let start = search_from + rel;
        let mut end = start + needle.len();
        while end < data.len() && is_oauth_secret_byte(data[end]) {
            end += 1;
        }
        if end >= start + needle.len() + 20 {
            if let Ok(secret) = std::str::from_utf8(&data[start..end]) {
                return Some(secret.to_string());
            }
        }
        search_from = start + needle.len();
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn is_oauth_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn is_oauth_secret_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

async fn read_auth(configured: String) -> Result<AuthState, AuthError> {
    tokio::time::timeout(
        AUTH_READ_TIMEOUT,
        tokio::task::spawn_blocking(move || load_auth(&configured)),
    )
    .await
    .map_err(|_| AuthError::Invalid)?
    .map_err(|_| AuthError::Invalid)?
}

fn load_auth(configured: &str) -> Result<AuthState, AuthError> {
    let text = if !configured.is_empty() {
        read_configured_text(configured)?
    } else {
        match read_default_text() {
            Ok(text) => text,
            Err(AuthError::Missing) => read_os_keyring_text()?,
            Err(error) => return Err(error),
        }
    };
    parse_auth_text(&text)
}

fn read_configured_text(configured: &str) -> Result<String, AuthError> {
    read_file_limited(Path::new(configured)).map_err(|error| {
        if is_missing_io(&error) {
            AuthError::Missing
        } else {
            AuthError::Invalid
        }
    })
}

fn read_default_text() -> Result<String, AuthError> {
    let path = default_auth_path().ok_or(AuthError::Missing)?;
    read_file_limited(&path).map_err(|error| {
        if is_missing_io(&error) {
            AuthError::Missing
        } else {
            AuthError::Invalid
        }
    })
}

fn default_auth_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".gemini")
            .join("antigravity-cli")
            .join("antigravity-oauth-token"),
    )
}

fn parse_auth_text(text: &str) -> Result<AuthState, AuthError> {
    let root: Value = serde_json::from_str(text).map_err(|_| AuthError::Invalid)?;
    let token = root.get("token").unwrap_or(&root);
    let access = token
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let refresh = token
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let expires_at = token
        .get("expiry")
        .and_then(Value::as_str)
        .and_then(parse_datetime_str);
    let oauth_client_id = json_nonempty([token, &root], &["client_id", "clientId"]);
    let oauth_client_secret = json_nonempty([token, &root], &["client_secret", "clientSecret"]);
    if access.is_none() && refresh.is_none() {
        return Err(AuthError::Missing);
    }
    if let Some(expires) = expires_at {
        if expires.timestamp() <= Utc::now().timestamp() + TOKEN_EXPIRY_MARGIN_SECS {
            if refresh.is_some() {
                // Treat as active so fetch() can refresh in memory.
                return Ok(AuthState::Active(AntigravityAuth {
                    access_token: access.unwrap_or_default(),
                    refresh_token: refresh,
                    expires_at: Some(expires),
                    oauth_client_id: oauth_client_id.clone(),
                    oauth_client_secret: oauth_client_secret.clone(),
                }));
            }
            return Ok(AuthState::Expired);
        }
    }
    let access = access.ok_or(AuthError::Missing)?;
    Ok(AuthState::Active(AntigravityAuth {
        access_token: access,
        refresh_token: refresh,
        expires_at,
        oauth_client_id,
        oauth_client_secret,
    }))
}

fn read_os_keyring_text() -> Result<String, AuthError> {
    #[cfg(target_os = "macos")]
    {
        read_macos_keyring_text()
    }
    #[cfg(target_os = "linux")]
    {
        read_linux_keyring_text()
    }
    #[cfg(windows)]
    {
        read_windows_keyring_text()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Err(AuthError::Missing)
    }
}

#[cfg(target_os = "macos")]
fn read_macos_keyring_text() -> Result<String, AuthError> {
    let mut command = Command::new("/usr/bin/security");
    command.args([
        "find-generic-password",
        "-a",
        KEYRING_ACCOUNT,
        "-s",
        KEYRING_SERVICE,
        "-w",
    ]);
    read_helper_password(command)
}

#[cfg(target_os = "linux")]
fn read_linux_keyring_text() -> Result<String, AuthError> {
    match read_secret_tool_password() {
        Ok(text) => Ok(text),
        Err(AuthError::Missing) => read_libsecret_password(),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
fn read_secret_tool_password() -> Result<String, AuthError> {
    let mut command = Command::new("secret-tool");
    command.args([
        "lookup",
        "service",
        KEYRING_SERVICE,
        "username",
        KEYRING_ACCOUNT,
    ]);
    read_helper_password(command)
}

#[cfg(target_os = "linux")]
fn read_libsecret_password() -> Result<String, AuthError> {
    let mut command = Command::new("python3");
    command.args(["-c", LINUX_KEYRING_PY]);
    read_helper_password(command)
}

#[cfg(windows)]
fn read_windows_keyring_text() -> Result<String, AuthError> {
    match read_windows_generic_credential(&format!("{KEYRING_SERVICE}:{KEYRING_ACCOUNT}")) {
        Ok(text) => Ok(text),
        Err(AuthError::Missing) => {
            read_windows_generic_credential(&format!("{KEYRING_ACCOUNT}.{KEYRING_SERVICE}"))
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn read_windows_generic_credential(target: &str) -> Result<String, AuthError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    let encoded: Vec<u16> = std::ffi::OsStr::new(target)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut credential = std::ptr::null_mut();
    unsafe {
        if CredReadW(
            PCWSTR(encoded.as_ptr()),
            CRED_TYPE_GENERIC,
            0,
            &mut credential,
        )
        .is_err()
            || credential.is_null()
        {
            return Err(AuthError::Missing);
        }
    }
    struct CredGuard(*mut CREDENTIALW);
    impl Drop for CredGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CredFree(self.0 as *const _);
                }
            }
        }
    }
    let guard = CredGuard(credential);
    let cred = unsafe { &*guard.0 };
    let len = cred.CredentialBlobSize as usize;
    if len == 0 || cred.CredentialBlob.is_null() {
        return Err(AuthError::Missing);
    }
    if len > MAX_AUTH_BYTES {
        return Err(AuthError::Invalid);
    }
    let bytes = unsafe { std::slice::from_raw_parts(cred.CredentialBlob, len) };
    String::from_utf8(bytes.to_vec()).map_err(|_| AuthError::Invalid)
}

#[cfg(not(windows))]
fn read_helper_password(command: Command) -> Result<String, AuthError> {
    let output = run_command_capture(command, KEYRING_TIMEOUT).map_err(|error| {
        if is_missing_io(&error) {
            AuthError::Missing
        } else {
            AuthError::Invalid
        }
    })?;
    let text = output.trim();
    if text.is_empty() {
        return Err(AuthError::Missing);
    }
    if text.len() > MAX_AUTH_BYTES {
        return Err(AuthError::Invalid);
    }
    Ok(text.to_string())
}

#[cfg(not(windows))]
fn run_command_capture(mut command: Command, timeout: StdDuration) -> anyhow::Result<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("start Antigravity credential helper")?;
    let stdout = child
        .stdout
        .take()
        .context("capture Antigravity credential helper output")?;
    let reader = std::thread::spawn(move || read_bounded_utf8(stdout));
    let deadline = Instant::now() + timeout;

    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("poll Antigravity credential helper")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("Antigravity credential helper timed out");
        }
        std::thread::sleep(StdDuration::from_millis(25));
    };

    let text = reader
        .join()
        .map_err(|_| anyhow!("join Antigravity credential helper output"))??;
    if !status.success() {
        if helper_status_is_missing(status.code()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Antigravity credential helper found no login",
            )
            .into());
        }
        bail!("Antigravity credential helper failed");
    }
    Ok(text)
}

#[cfg(target_os = "macos")]
fn helper_status_is_missing(code: Option<i32>) -> bool {
    matches!(code, Some(HELPER_NOT_FOUND_EXIT_CODE))
}

#[cfg(target_os = "linux")]
fn helper_status_is_missing(code: Option<i32>) -> bool {
    matches!(
        code,
        Some(LINUX_SECRET_TOOL_MISSING_EXIT_CODE)
            | Some(LINUX_LIBSECRET_MISSING_EXIT_CODE)
            | Some(HELPER_NOT_FOUND_EXIT_CODE)
    )
}

#[cfg(all(not(windows), not(target_os = "macos"), not(target_os = "linux")))]
fn helper_status_is_missing(_code: Option<i32>) -> bool {
    false
}

fn read_file_limited(path: &Path) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("read Antigravity auth at {}", path.display()))?;
    read_bounded_utf8(file)
}

fn read_bounded_utf8(reader: impl Read) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_AUTH_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read Antigravity credential data")?;
    if bytes.len() > MAX_AUTH_BYTES {
        bail!("Antigravity credential data exceeds {MAX_AUTH_BYTES} bytes");
    }
    String::from_utf8(bytes).context("Antigravity credential data is not UTF-8")
}

fn is_missing_io(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_plan_label, load_auth, parse_auth_text, parse_plan, parse_usage, AuthError,
        AuthState,
    };
    use chrono::{Duration, Utc};

    #[test]
    fn parse_usage_reads_gemini_windows_from_remaining_fraction() {
        let service = parse_usage(include_str!("../../../mocks/antigravity_quota_normal.json"))
            .expect("bundled Antigravity fixture");
        assert_eq!(service.status, "NOMINAL");
        assert!((service.five_hour_percent.unwrap_or_default() - 27.0).abs() < 0.01);
        assert!((service.seven_day_percent.unwrap_or_default() - 21.0).abs() < 0.01);
        assert!(service.five_hour_reset_local.is_some());
        assert!(service.seven_day_reset_local.is_some());
    }

    #[test]
    fn parse_usage_rejects_missing_gemini_group() {
        assert!(parse_usage(r#"{"groups":[]}"#).is_err());
        assert!(parse_usage("{}").is_err());
    }

    #[test]
    fn plan_labels_are_allowlisted() {
        assert_eq!(canonical_plan_label("Antigravity"), Some("Antigravity"));
        assert_eq!(canonical_plan_label("Google AI Pro"), Some("Google AI Pro"));
        assert_eq!(canonical_plan_label("ultra"), Some("Google AI Ultra"));
        assert_eq!(canonical_plan_label("mystery"), None);
        assert_eq!(
            parse_plan(
                r#"{"currentTier":{"name":"Antigravity"},"paidTier":{"name":"Google AI Pro"}}"#
            )
            .as_deref(),
            Some("Antigravity")
        );
    }

    #[test]
    fn expired_access_token_stays_refreshable_without_exposing_refresh_material() {
        let expired = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let state = parse_auth_text(&format!(
            r#"{{"token":{{"access_token":"old","refresh_token":"do-not-keep","expiry":"{expired}"}}}}"#
        ))
        .expect("refreshable expired session");
        let AuthState::Active(auth) = state else {
            panic!("expected refreshable session");
        };
        assert_eq!(auth.access_token, "old");
        assert!(auth.refresh_token.is_some());
    }

    #[test]
    fn default_file_is_used_when_present() {
        if super::default_auth_path().is_some_and(|path| path.exists()) {
            assert!(matches!(load_auth(""), Ok(AuthState::Active(_))));
        }
    }

    #[test]
    fn configured_missing_file_does_not_consult_os_keyring() {
        let path = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-missing-agy-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        assert!(matches!(
            load_auth(path.to_str().expect("utf-8 temp path")),
            Err(AuthError::Missing)
        ));
    }

    #[test]
    fn missing_tokens_are_login_required() {
        assert!(matches!(
            parse_auth_text(r#"{"token":{}}"#),
            Err(super::AuthError::Missing)
        ));
    }

    #[test]
    fn expired_access_without_refresh_is_session_expired() {
        let expired = (Utc::now() - Duration::hours(1)).to_rfc3339();
        assert!(matches!(
            parse_auth_text(&format!(
                r#"{{"token":{{"access_token":"old","expiry":"{expired}"}}}}"#
            )),
            Ok(AuthState::Expired)
        ));
    }

    fn sample_oauth_client() -> (String, String) {
        (
            format!("123-abc.apps.{}", "googleusercontent.com"),
            format!("GOCSPX-{}", "abcdefghijklmnopqrstuvwxyz12"),
        )
    }

    #[test]
    fn oauth_client_is_parsed_from_credential_json() {
        let (client_id, client_secret) = sample_oauth_client();
        let AuthState::Active(auth) = parse_auth_text(&format!(
            r#"{{"token":{{"access_token":"a","refresh_token":"r","client_id":"{client_id}","client_secret":"{client_secret}"}}}}"#
        ))
        .expect("auth with oauth client") else {
            panic!("expected active session");
        };
        assert_eq!(auth.oauth_client_id.as_deref(), Some(client_id.as_str()));
        let clients = super::resolve_oauth_clients(&auth);
        assert!(clients.iter().any(|client| client.client_id == client_id));
    }

    #[test]
    fn oauth_client_is_parsed_from_installed_artifact_text() {
        let (client_id, client_secret) = sample_oauth_client();
        let hay = format!(
            r#"vs/platform/cloudCode/common/oauthClient.js "{client_id}" "{client_secret}""#
        );
        let client = super::parse_oauth_client_from_text(&hay).expect("artifact client");
        assert_eq!(client.client_id, client_id);
        assert!(client.client_secret.starts_with("GOCSPX-"));
    }
}
