//! Codex usage fetcher. Reads the bearer token from `~/.codex/auth.json` and
//! queries the ChatGPT/Codex web backend. That endpoint is not a stable public
//! API, so malformed responses fall back to last-known-good cached data.
//!
//! Credential discovery matches Claude/Grok on Windows: native
//! `%USERPROFILE%\.codex\auth.json` first, then a default WSL `Ubuntu` home
//! fallback. Explicit `codexAuthPath` supports a bounded
//! `wsl:<distro>:<absolute-path>` reader and is fail closed.

use super::{
    require_success, send, send_with_one_retry, FetchError, Resp, MSG_CREDENTIAL_ERROR,
    MSG_LOGIN_REQUIRED,
};
use crate::config::Config;
use crate::models::CodexService;
use crate::util::{clamp_percent, fmt_local, local_label, parse_datetime};
use anyhow::{anyhow, bail, Context};
use chrono::Utc;
use reqwest::Client;
use serde_json::Map;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Stdio};
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const RESET_CREDITS_TIMEOUT: Duration = Duration::from_secs(4);
const LONG_WINDOW_THRESHOLD_SECONDS: u64 = 24 * 60 * 60;
const MAX_AUTH_BYTES: usize = 64 * 1024;
#[cfg(any(windows, test))]
const WSL_MISSING_EXIT_CODE: i32 = 44;
#[cfg(windows)]
const WSL_PROCESS_TIMEOUT: Duration = Duration::from_secs(15);
type Window<'a> = &'a Map<String, Value>;
type WindowPair<'a> = (Option<Window<'a>>, Option<Window<'a>>);

struct CodexAuth {
    token: String,
    account_id: Option<String>,
}

#[derive(Debug)]
enum CodexAuthError {
    Missing,
    Invalid,
}

#[derive(Debug)]
struct MissingCodexToken;

#[derive(Debug)]
struct MissingCodexCredential;

impl std::fmt::Display for MissingCodexToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Codex token missing in auth.json")
    }
}

impl std::error::Error for MissingCodexToken {}

impl std::fmt::Display for MissingCodexCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Codex credential file is missing")
    }
}

impl std::error::Error for MissingCodexCredential {}

struct ResetCreditSummary {
    available: u64,
    earliest_expiry_local: Option<String>,
}

pub async fn fetch(config: &Config, client: &Client) -> Result<CodexService, FetchError> {
    let auth = match read_auth(config) {
        Ok(auth) => auth,
        Err(CodexAuthError::Missing) => {
            return Err(FetchError::Auth {
                message: MSG_LOGIN_REQUIRED,
            })
        }
        Err(CodexAuthError::Invalid) => {
            return Err(FetchError::Auth {
                message: MSG_CREDENTIAL_ERROR,
            })
        }
    };

    let resp: Resp = send_with_one_retry(|| authenticated_get(client, USAGE_URL, &auth))
        .await
        .map_err(FetchError::Other)?;

    require_success(&resp, "Codex usage")?;
    let mut service = parse_usage(&resp.body).map_err(FetchError::Other)?;

    let request = authenticated_get(client, RESET_CREDITS_URL, &auth)
        .timeout(RESET_CREDITS_TIMEOUT)
        .header("Accept", "application/json")
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "ai-usage-dashboard");
    let reset_summary = match send(request).await {
        Ok(resp) if resp.is_success() => parse_reset_credits(&resp.body).ok(),
        _ => None,
    };
    if let Some(summary) = reset_summary {
        service.reset_credits_available = Some(summary.available);
        service.reset_credits_expire_local = summary.earliest_expiry_local;
    }

    Ok(service)
}

pub(crate) fn parse_usage(body: &str) -> anyhow::Result<CodexService> {
    let root: Value = serde_json::from_str(body).context("parse Codex usage body")?;
    let rate = root
        .get("rate_limit")
        .and_then(Value::as_object)
        .context("Codex usage missing rate_limit")?;
    let primary = rate
        .get("primary_window")
        .and_then(Value::as_object)
        .context("Codex usage missing primary_window")?;
    let secondary = rate.get("secondary_window").and_then(Value::as_object);
    let (five_hour, seven_day) = classify_windows(primary, secondary);

    Ok(CodexService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        plan: root
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        five_hour_percent: window_percent(five_hour, "five-hour")?,
        seven_day_percent: window_percent(seven_day, "seven-day")?,
        five_hour_reset_local: window_reset(five_hour),
        seven_day_reset_local: window_reset(seven_day),
        reset_credits_available: reset_credit_count(&root),
        reset_credits_expire_local: None,
    })
}

fn reset_credit_count(root: &Value) -> Option<u64> {
    root.get("rate_limit_reset_credits")?
        .get("available_count")?
        .as_u64()
}

fn parse_reset_credits(body: &str) -> anyhow::Result<ResetCreditSummary> {
    let root: Value = serde_json::from_str(body).context("parse Codex reset credits body")?;
    let available = root
        .get("available_count")
        .and_then(Value::as_u64)
        .context("Codex reset credits missing available_count")?;
    let now = Utc::now();
    let earliest_expiry_local = root
        .get("credits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|credit| credit.get("status").and_then(Value::as_str) == Some("available"))
        .filter_map(|credit| credit.get("expires_at").and_then(parse_datetime))
        .filter(|expiry| *expiry > now)
        .min()
        .map(fmt_local);

    Ok(ResetCreditSummary {
        available,
        earliest_expiry_local,
    })
}

fn classify_windows<'a>(primary: Window<'a>, secondary: Option<Window<'a>>) -> WindowPair<'a> {
    let primary_is_long = is_long_window(primary);
    match secondary {
        Some(secondary) if primary_is_long && !is_long_window(secondary) => {
            (Some(secondary), Some(primary))
        }
        Some(secondary) => (Some(primary), Some(secondary)),
        None if primary_is_long => (None, Some(primary)),
        None => (Some(primary), None),
    }
}

fn is_long_window(window: &Map<String, Value>) -> bool {
    window
        .get("limit_window_seconds")
        .and_then(Value::as_u64)
        .is_some_and(|seconds| seconds >= LONG_WINDOW_THRESHOLD_SECONDS)
}

fn window_percent(window: Option<&Map<String, Value>>, label: &str) -> anyhow::Result<Option<f64>> {
    window
        .map(|window| {
            window
                .get("used_percent")
                .and_then(Value::as_f64)
                .map(clamp_percent)
                .with_context(|| format!("Codex usage missing {label} used_percent"))
        })
        .transpose()
}

fn window_reset(window: Option<&Map<String, Value>>) -> Option<String> {
    window?.get("reset_at").and_then(local_label)
}

fn authenticated_get(client: &Client, url: &str, auth: &CodexAuth) -> reqwest::RequestBuilder {
    let request = client
        .get(url)
        .header("Authorization", format!("Bearer {}", auth.token));
    match &auth.account_id {
        Some(account_id) => request.header("ChatGPT-Account-Id", account_id),
        None => request,
    }
}

fn read_auth(config: &Config) -> Result<CodexAuth, CodexAuthError> {
    let text = read_auth_text(config.codex_auth_path.trim()).map_err(|error| {
        let missing = error.chain().any(|cause| {
            cause.downcast_ref::<MissingCodexCredential>().is_some()
                || cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
        });
        if missing {
            CodexAuthError::Missing
        } else {
            CodexAuthError::Invalid
        }
    })?;
    parse_auth(&text).map_err(|error| {
        if error.downcast_ref::<MissingCodexToken>().is_some() {
            CodexAuthError::Missing
        } else {
            CodexAuthError::Invalid
        }
    })
}

fn read_auth_text(configured: &str) -> anyhow::Result<String> {
    if !configured.is_empty() {
        if let Some(spec) = parse_wsl_spec(configured)? {
            #[cfg(windows)]
            {
                return read_wsl_path(&spec.distro, &spec.path);
            }
            #[cfg(not(windows))]
            {
                let _ = spec;
                bail!("Codex wsl: credential paths require Windows");
            }
        }
        return read_file_limited(Path::new(configured));
    }

    let path = default_auth_path()?;
    #[cfg(windows)]
    {
        match std::fs::metadata(&path) {
            Ok(_) => read_file_limited(&path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => read_default_wsl_auth(),
            Err(err) => {
                Err(err).with_context(|| format!("inspect Codex auth at {}", path.display()))
            }
        }
    }
    #[cfg(not(windows))]
    {
        read_file_limited(&path)
    }
}

fn default_auth_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("no home dir"))?
        .join(".codex")
        .join("auth.json"))
}

fn read_file_limited(path: &Path) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("read Codex auth.json at {}", path.display()))?;
    read_bounded_utf8(file)
}

fn read_bounded_utf8(reader: impl Read) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_AUTH_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read Codex credential data")?;
    if bytes.len() > MAX_AUTH_BYTES {
        bail!("Codex auth.json exceeds {MAX_AUTH_BYTES} bytes");
    }
    String::from_utf8(bytes).context("Codex auth.json is not UTF-8")
}

#[derive(Debug, PartialEq, Eq)]
struct WslSpec {
    distro: String,
    path: String,
}

fn parse_wsl_spec(value: &str) -> anyhow::Result<Option<WslSpec>> {
    let Some(spec) = value.strip_prefix("wsl:") else {
        return Ok(None);
    };
    let (raw_distro, raw_path) = spec
        .split_once(':')
        .context("Codex WSL path must be wsl:<distro>:<absolute-path>")?;
    if raw_distro.chars().any(char::is_control) || raw_path.chars().any(char::is_control) {
        bail!("Codex WSL credential spec contains control characters");
    }
    let distro = raw_distro.trim();
    let path = raw_path.trim();
    if distro.is_empty() {
        bail!("Codex WSL distribution is invalid");
    }
    if !path.starts_with('/') {
        bail!("Codex WSL credential path must be absolute");
    }
    Ok(Some(WslSpec {
        distro: distro.to_string(),
        path: path.to_string(),
    }))
}

#[cfg(windows)]
fn read_wsl_path(distro: &str, path: &str) -> anyhow::Result<String> {
    if let Some(text) = read_wsl_unc(distro, path) {
        return Ok(text);
    }
    let mut command = Command::new("wsl.exe");
    command.args([
        "-d",
        distro,
        "--",
        "sh",
        "-lc",
        "if [ ! -f \"$1\" ]; then exit 44; fi; cat -- \"$1\"",
        "ai-usage-dashboard",
        path,
    ]);
    read_wsl_command(command)
        .with_context(|| format!("read Codex WSL credentials at wsl:{distro}:{path}"))
}

#[cfg(windows)]
fn read_default_wsl_auth() -> anyhow::Result<String> {
    let mut command = Command::new("wsl.exe");
    command.args([
        "-d",
        "Ubuntu",
        "--",
        "sh",
        "-lc",
        "if [ ! -f \"$HOME/.codex/auth.json\" ]; then exit 44; fi; cat -- \"$HOME/.codex/auth.json\"",
    ]);
    read_wsl_command(command).context("read Codex auth.json from default WSL Ubuntu home")
}

#[cfg(windows)]
fn read_wsl_unc(distro: &str, linux_path: &str) -> Option<String> {
    for candidate in wsl_unc_candidates(distro, linux_path)? {
        if let Ok(text) = read_file_limited(&candidate) {
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    None
}

#[cfg(windows)]
fn wsl_unc_candidates(distro: &str, linux_path: &str) -> Option<[PathBuf; 2]> {
    fn numbered_device(stem: &str, prefix: &str) -> bool {
        let Some(suffix) = stem.strip_prefix(prefix) else {
            return false;
        };
        suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
    }

    fn valid_component(component: &str) -> bool {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with('.')
            || component.ends_with(' ')
            || component.chars().any(|ch| {
                ch <= '\u{1f}' || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            })
        {
            return false;
        }

        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !numbered_device(&stem, "COM")
            && !numbered_device(&stem, "LPT")
    }

    if !valid_component(distro) {
        return None;
    }
    let relative = linux_path.strip_prefix('/')?;
    let components = relative.split('/').collect::<Vec<_>>();
    if components.is_empty() || components.iter().any(|part| !valid_component(part)) {
        return None;
    }
    let joined = components.join("\\");
    Some([
        PathBuf::from(format!(r"\\wsl.localhost\{distro}\{joined}")),
        PathBuf::from(format!(r"\\wsl$\{distro}\{joined}")),
    ])
}

#[cfg(windows)]
fn read_wsl_command(mut command: Command) -> anyhow::Result<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("start WSL Codex credential reader")?;
    let stdout = child
        .stdout
        .take()
        .context("capture WSL Codex credential output")?;
    let reader = std::thread::spawn(move || read_bounded_utf8(stdout));
    let deadline = Instant::now() + WSL_PROCESS_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader
                    .join()
                    .map_err(|_| anyhow!("join WSL Codex credential reader"))??;
                if !status.success() {
                    if wsl_exit_is_missing(status.code()) {
                        return Err(MissingCodexCredential.into());
                    }
                    bail!("WSL Codex credential reader exited with {status}");
                }
                if text.trim().is_empty() {
                    bail!("WSL Codex credential file is empty");
                }
                return Ok(text);
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                bail!("WSL Codex credential reader timed out");
            }
            Err(err) => {
                let _ = child.kill();
                let _ = reader.join();
                return Err(err).context("wait for WSL Codex credential reader");
            }
        }
    }
}

#[cfg(any(windows, test))]
fn wsl_exit_is_missing(exit_code: Option<i32>) -> bool {
    exit_code == Some(WSL_MISSING_EXIT_CODE)
}

fn parse_auth(text: &str) -> anyhow::Result<CodexAuth> {
    let root: Value = serde_json::from_str(text).context("parse Codex auth.json")?;
    let tokens = &root["tokens"];
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    for key in ["access_token", "id_token"] {
        if let Some(t) = tokens.get(key).and_then(Value::as_str) {
            if !t.is_empty() {
                return Ok(CodexAuth {
                    token: t.to_string(),
                    account_id,
                });
            }
        }
    }
    Err(MissingCodexToken.into())
}

#[cfg(test)]
mod tests {
    use super::{
        default_auth_path, parse_auth, parse_reset_credits, parse_usage, parse_wsl_spec, read_auth,
        read_auth_text, wsl_exit_is_missing, CodexAuthError, WslSpec, WSL_MISSING_EXIT_CODE,
    };
    use crate::config::Config;
    use crate::util::local_label;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn wsl_missing_credential_exit_code_is_distinct() {
        assert!(wsl_exit_is_missing(Some(WSL_MISSING_EXIT_CODE)));
        assert!(!wsl_exit_is_missing(Some(1)));
        assert!(!wsl_exit_is_missing(None));
    }

    #[test]
    fn codex_used_percent_is_already_percent_scale() {
        let body = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 1,
                    "reset_at": 1780813800
                },
                "secondary_window": {
                    "used_percent": 38,
                    "reset_at": 1781121600
                }
            },
            "rate_limit_reset_credits": { "available_count": 4 }
        }"#;

        let usage = parse_usage(body).expect("valid Codex usage");

        assert_eq!(usage.five_hour_percent, Some(1.0));
        assert_eq!(usage.seven_day_percent, Some(38.0));
        assert_eq!(usage.reset_credits_available, Some(4));
    }

    #[test]
    fn codex_usage_accepts_a_single_classified_window() {
        let body = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 12,
                    "limit_window_seconds": 604800,
                    "reset_at": 1784502445
                },
                "secondary_window": null
            }
        }"#;

        let usage = parse_usage(body).expect("valid weekly-only Codex usage");

        assert_eq!(usage.five_hour_percent, None);
        assert_eq!(usage.seven_day_percent, Some(12.0));
        assert_eq!(usage.five_hour_reset_local, None);
        assert!(usage.seven_day_reset_local.is_some());
    }

    #[test]
    fn codex_usage_rejects_missing_windows_and_percentages() {
        for body in [
            r#"{}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":1},"secondary_window":{}}}"#,
            r#"{"rate_limit":{"primary_window":{"limit_window_seconds":604800}}}"#,
        ] {
            assert!(parse_usage(body).is_err(), "unexpectedly accepted {body}");
        }
    }

    #[test]
    fn codex_reset_credits_keep_count_and_earliest_available_expiry() {
        let body = r#"{
            "available_count": 3,
            "credits": [
                {"status":"available","expires_at":"2099-08-03T09:00:00Z"},
                {"status":"redeemed","expires_at":"2099-07-01T09:00:00Z"},
                {"status":"available","expires_at":null},
                {"status":"available","expires_at":"2099-08-01T09:00:00Z"}
            ]
        }"#;

        let summary = parse_reset_credits(body).expect("valid reset-credit inventory");

        assert_eq!(summary.available, 3);
        assert_eq!(
            summary.earliest_expiry_local,
            local_label(&json!("2099-08-01T09:00:00Z"))
        );
    }

    #[test]
    fn codex_auth_keeps_account_scope_without_exposing_it_to_the_service() {
        let auth = parse_auth(r#"{"tokens":{"access_token":"secret","account_id":"account-123"}}"#)
            .expect("valid auth");

        assert_eq!(auth.token, "secret");
        assert_eq!(auth.account_id.as_deref(), Some("account-123"));
    }

    #[test]
    fn codex_wsl_spec_parses_bounded_absolute_paths() {
        assert_eq!(
            parse_wsl_spec("wsl:Ubuntu:/root/.codex/auth.json").expect("valid"),
            Some(WslSpec {
                distro: "Ubuntu".into(),
                path: "/root/.codex/auth.json".into(),
            })
        );
        assert_eq!(
            parse_wsl_spec(r"C:\Users\me\.codex\auth.json").expect("native"),
            None
        );
        assert!(parse_wsl_spec("wsl::/root/.codex/auth.json").is_err());
        assert!(parse_wsl_spec("wsl:Ubuntu:relative/auth.json").is_err());
        assert!(parse_wsl_spec("wsl:Ubuntu\n:/root/.codex/auth.json").is_err());
    }

    #[test]
    fn codex_explicit_native_path_is_fail_closed() {
        let missing = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-missing-codex-auth-{}.json",
            std::process::id()
        ));
        let err = read_auth_text(&missing.to_string_lossy()).expect_err("missing explicit path");
        let message = format!("{err:#}");
        assert!(
            message.contains("read Codex auth.json") || message.contains("No such file"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn codex_auth_classifies_only_missing_credentials_as_login_required() {
        let root = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-codex-auth-classification-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create auth fixture directory");

        let missing_config = Config {
            codex_auth_path: root.join("missing.json").to_string_lossy().into_owned(),
            ..Config::default()
        };
        assert!(matches!(
            read_auth(&missing_config),
            Err(CodexAuthError::Missing)
        ));

        let malformed = root.join("malformed.json");
        std::fs::write(&malformed, b"not-json").expect("write malformed auth fixture");
        let malformed_config = Config {
            codex_auth_path: malformed.to_string_lossy().into_owned(),
            ..Config::default()
        };
        assert!(matches!(
            read_auth(&malformed_config),
            Err(CodexAuthError::Invalid)
        ));

        let tokenless = root.join("tokenless.json");
        std::fs::write(&tokenless, br#"{"tokens":{}}"#).expect("write tokenless fixture");
        let tokenless_config = Config {
            codex_auth_path: tokenless.to_string_lossy().into_owned(),
            ..Config::default()
        };
        assert!(matches!(
            read_auth(&tokenless_config),
            Err(CodexAuthError::Missing)
        ));

        std::fs::remove_dir_all(root).expect("remove auth fixtures");
    }

    #[test]
    fn codex_explicit_native_path_reads_auth_json() {
        let path = std::env::temp_dir().join(format!(
            "ai-usage-dashboard-codex-auth-{}.json",
            std::process::id()
        ));
        let mut file = std::fs::File::create(&path).expect("create auth fixture");
        file.write_all(br#"{"tokens":{"access_token":"from-file","account_id":"acct"}}"#)
            .expect("write auth fixture");
        drop(file);

        let text = read_auth_text(&path.to_string_lossy()).expect("read explicit path");
        let auth = parse_auth(&text).expect("parse");
        assert_eq!(auth.token, "from-file");
        assert_eq!(auth.account_id.as_deref(), Some("acct"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn codex_default_auth_path_is_home_dot_codex() {
        let path = default_auth_path().expect("home");
        assert!(path.ends_with(std::path::Path::new(".codex").join("auth.json")));
    }

    #[cfg(windows)]
    #[test]
    fn codex_wsl_unc_candidates_are_exact() {
        use super::wsl_unc_candidates;
        let paths =
            wsl_unc_candidates("Ubuntu", "/root/.codex/auth.json").expect("valid linux path");
        assert_eq!(
            paths[0].to_string_lossy(),
            r"\\wsl.localhost\Ubuntu\root\.codex\auth.json"
        );
        assert_eq!(
            paths[1].to_string_lossy(),
            r"\\wsl$\Ubuntu\root\.codex\auth.json"
        );
        assert!(wsl_unc_candidates("Ubuntu", "relative/auth.json").is_none());
        assert!(wsl_unc_candidates("bad:distro", "/root/.codex/auth.json").is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn codex_wsl_spec_requires_windows() {
        let err = read_auth_text("wsl:Ubuntu:/root/.codex/auth.json")
            .expect_err("wsl paths only work on Windows");
        assert!(format!("{err:#}").contains("require Windows"));
    }
}
