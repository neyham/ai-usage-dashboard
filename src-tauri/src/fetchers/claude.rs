//! Claude usage fetcher. Reads OAuth credentials from the local
//! `.claude/.credentials.json` (or, on Windows, falls back to reading the WSL
//! home credentials via `wsl.exe`). Refreshes on expiry / 401 and signals 429
//! back to the orchestrator so it can enter cooldown.

use super::{send, send_with_one_retry, FetchError, Resp};
use crate::cache::cache_dir;
use crate::config::{
    Config, DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD, MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
    MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS, MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
    MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
};
use crate::fs_util::atomic_write;
use crate::models::ClaudeService;
use crate::util::{clamp_percent, local_label, parse_datetime};
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration as StdDuration, Instant};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
// Keep this aligned with the token endpoint shipped by current Claude Code.
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
const MSG_LOGIN_REQUIRED: &str = "LOGIN REQUIRED";
const MSG_AUTH_EXPIRED: &str = "AUTH EXPIRED";
const MSG_REFRESH_BLOCKED: &str = "REFRESH BLOCKED";
const MSG_AUTH_CHECK_FAILED: &str = "AUTH CHECK FAILED";
const CLAUDE_CODE_RECOVERY_THROTTLE: Duration = Duration::minutes(30);
const CLAUDE_CODE_RECOVERY_LOCK_FILE: &str = "claude-code-recovery.lock";
const CLAUDE_CODE_RECOVERY_STATE_FILE: &str = "claude-code-recovery-attempt.txt";
const RECOVERY_LOCK_WAIT: StdDuration = StdDuration::from_secs(5);

#[derive(Debug)]
enum RefreshError {
    RateLimited { retry_after: Option<u64> },
    MissingRefreshToken,
    InvalidGrant,
    DirectRefreshUnavailable,
    Forbidden,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for RefreshError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

pub async fn fetch(config: &Config, client: &Client) -> Result<ClaudeService, FetchError> {
    let load_config = config.clone();
    let mut creds = tokio::task::spawn_blocking(move || load(&load_config))
        .await
        .map_err(|err| FetchError::Other(anyhow!("Claude credential task failed: {err}")))?
        .map_err(load_error)?;

    if creds.is_expired_soon() {
        refresh_or_recover(config, client, &mut creds).await?;
    }

    let mut resp = send_with_one_retry(|| usage_request(client, &creds.access_token))
        .await
        .map_err(FetchError::Other)?;

    if resp.status == 401 {
        refresh_or_recover(config, client, &mut creds).await?;
        resp = send_with_one_retry(|| usage_request(client, &creds.access_token))
            .await
            .map_err(FetchError::Other)?;
    }

    if resp.status == 429 {
        return Err(FetchError::RateLimited {
            retry_after: resp.retry_after,
        });
    }
    if !resp.is_success() {
        if resp.status == 401 {
            return Err(FetchError::Auth {
                message: MSG_AUTH_EXPIRED,
            });
        }
        return Err(FetchError::Other(anyhow!(
            "Claude usage HTTP {}",
            resp.status
        )));
    }

    parse_usage(&resp.body).map_err(FetchError::Other)
}

async fn refresh_or_recover(
    config: &Config,
    client: &Client,
    creds: &mut Creds,
) -> Result<(), FetchError> {
    match creds.refresh(client).await {
        Ok(()) => Ok(()),
        Err(RefreshError::RateLimited { retry_after }) => {
            Err(FetchError::RateLimited { retry_after })
        }
        Err(err) => recover_allowed_error(config, creds, err).await,
    }
}

async fn recover_allowed_error(
    config: &Config,
    creds: &mut Creds,
    err: RefreshError,
) -> Result<(), FetchError> {
    if let RefreshError::RateLimited { retry_after } = &err {
        return Err(FetchError::RateLimited {
            retry_after: *retry_after,
        });
    }
    let (allow_claude_code, message) = recovery_policy(&err);

    if allow_claude_code
        && config.claude_code_refresh_enabled
        && try_claude_code_refresh(config, creds).await.is_ok()
    {
        return Ok(());
    }
    Err(FetchError::Auth { message })
}

fn recovery_policy(err: &RefreshError) -> (bool, &'static str) {
    match err {
        RefreshError::MissingRefreshToken | RefreshError::InvalidGrant => (true, MSG_AUTH_EXPIRED),
        RefreshError::DirectRefreshUnavailable | RefreshError::Forbidden => {
            (true, MSG_REFRESH_BLOCKED)
        }
        RefreshError::Other(err) => {
            let _ = err;
            (false, MSG_AUTH_CHECK_FAILED)
        }
        RefreshError::RateLimited { .. } => (false, MSG_AUTH_CHECK_FAILED),
    }
}

fn load_error(err: anyhow::Error) -> FetchError {
    let text = err.to_string();
    if text.contains("credentials not found") || text.contains("OAuth token missing") {
        FetchError::Auth {
            message: MSG_LOGIN_REQUIRED,
        }
    } else {
        FetchError::Other(err)
    }
}

fn usage_request(client: &Client, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", ANTHROPIC_BETA)
}

fn refresh_request(client: &Client, refresh_token: &str) -> reqwest::RequestBuilder {
    client.post(TOKEN_URL).json(&json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
        "scope": OAUTH_SCOPES,
    }))
}

fn validate_refresh_response(resp: &Resp) -> Result<(), RefreshError> {
    if resp.status == 429 {
        return Err(RefreshError::RateLimited {
            retry_after: resp.retry_after,
        });
    }
    if resp.status == 403 {
        return Err(RefreshError::Forbidden);
    }
    if matches!(resp.status, 400 | 401) && is_invalid_grant(&resp.body) {
        return Err(RefreshError::InvalidGrant);
    }
    if !resp.is_success() {
        return Err(anyhow!("Claude token refresh HTTP {}", resp.status).into());
    }
    Ok(())
}

fn is_invalid_grant(body: &str) -> bool {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    match root.get("error") {
        Some(Value::String(code)) => code == "invalid_grant",
        Some(Value::Object(error)) => {
            error.get("type").and_then(Value::as_str) == Some("invalid_grant")
        }
        _ => false,
    }
}

pub(crate) fn parse_usage(body: &str) -> anyhow::Result<ClaudeService> {
    let root: Value = serde_json::from_str(body).context("parse Claude usage body")?;
    let five = root
        .get("five_hour")
        .and_then(Value::as_object)
        .context("Claude usage missing five_hour window")?;
    let seven = root
        .get("seven_day")
        .and_then(Value::as_object)
        .context("Claude usage missing seven_day window")?;
    let five_hour_percent = five
        .get("utilization")
        .and_then(Value::as_f64)
        .map(clamp_percent)
        .context("Claude usage missing five_hour utilization")?;
    let seven_day_percent = seven
        .get("utilization")
        .and_then(Value::as_f64)
        .map(clamp_percent)
        .context("Claude usage missing seven_day utilization")?;

    Ok(ClaudeService {
        status: "NOMINAL".into(),
        from_cache: false,
        data_may_be_stale: false,
        cooldown_until_local: None,
        five_hour_percent: Some(five_hour_percent),
        seven_day_percent: Some(seven_day_percent),
        five_hour_reset_local: five.get("resets_at").and_then(local_label),
        seven_day_reset_local: seven.get("resets_at").and_then(local_label),
    })
}

// ---------- Credentials ----------

#[derive(Clone)]
enum CredSource {
    File(PathBuf),
    #[cfg(windows)]
    Wsl {
        distro: String,
        path: String,
    },
}

impl CredSource {
    #[cfg(windows)]
    fn wsl_distro(&self) -> Option<String> {
        match self {
            CredSource::Wsl { distro, .. } => Some(distro.clone()),
            CredSource::File(path) => {
                let text = path.to_string_lossy();
                for prefix in ["\\\\wsl.localhost\\", "\\\\wsl$\\"] {
                    if let Some(rest) = text.strip_prefix(prefix) {
                        return rest
                            .split(['\\', '/'])
                            .next()
                            .filter(|s| !s.is_empty())
                            .map(str::to_string);
                    }
                }
                None
            }
        }
    }
}

#[derive(Clone)]
struct Creds {
    root: Value,
    access_token: String,
    refresh_token: String,
    expires_at: Option<DateTime<Utc>>,
    source: CredSource,
}

impl Creds {
    fn is_expired_soon(&self) -> bool {
        match self.expires_at {
            Some(exp) => Utc::now() + Duration::minutes(2) >= exp,
            None => false,
        }
    }

    async fn refresh(&mut self, client: &Client) -> Result<(), RefreshError> {
        if self.refresh_token.is_empty() {
            return Err(RefreshError::MissingRefreshToken);
        }
        if matches!(&self.source, CredSource::File(_)) {
            return Err(RefreshError::DirectRefreshUnavailable);
        }

        let access_before_lock = self.access_token.clone();
        let expired_before_lock = self.is_expired_soon();
        let mut refresh_lock = acquire_refresh_lock(&self.source).await?;

        // Another dashboard or Claude Code may have refreshed while this
        // process waited. Re-read the exact source while holding the lock and
        // avoid replaying the now-consumed refresh token.
        let locked = reload_credentials(self.source.clone()).await?;
        let access_rotated = locked.access_token != access_before_lock;
        *self = locked;
        if access_rotated || (expired_before_lock && !self.is_expired_soon()) {
            return Ok(());
        }
        if self.refresh_token.is_empty() {
            return Err(RefreshError::MissingRefreshToken);
        }

        let access_used = self.access_token.clone();
        let refresh_used = self.refresh_token.clone();

        // Refresh tokens can rotate. Retrying after an ambiguous transport failure
        // could replay an already-consumed token and destroy the login state.
        let resp = send(refresh_request(client, &refresh_used)).await?;
        validate_refresh_response(&resp)?;

        let data: Value = serde_json::from_str(&resp.body).context("parse refresh response")?;
        refresh_lock.ensure_held()?;

        // Merge into the latest full credential document so unrelated fields
        // written by Claude Code during the request are never rolled back.
        let latest = reload_credentials(self.source.clone()).await?;
        if latest.access_token != access_used || latest.refresh_token != refresh_used {
            let recovered = latest.access_token != access_used;
            *self = latest;
            if recovered {
                return Ok(());
            }
            return Err(anyhow!("Claude credentials changed during token refresh").into());
        }

        let mut updated = latest;
        updated.apply_refresh_response(&data)?;
        refresh_lock.ensure_held()?;
        updated.write_back().await?;
        *self = updated;
        Ok(())
    }

    fn apply_refresh_response(&mut self, data: &Value) -> anyhow::Result<()> {
        let access = str_field(data, &["access_token", "accessToken"]);
        if access.is_empty() {
            bail!("Claude refresh response missing access token");
        }
        let refresh = str_field(data, &["refresh_token", "refreshToken"]);

        // Claude Code stores expiresAt as epoch milliseconds and compares it
        // numerically with Date.now(). Preserve that shared-file contract.
        let now = Utc::now();
        let expires_in = data
            .get("expires_in")
            .or_else(|| data.get("expiresIn"))
            .and_then(Value::as_i64)
            .filter(|secs| *secs > 0);
        let expires = if let Some(secs) = expires_in {
            now.checked_add_signed(Duration::seconds(secs))
                .context("Claude refresh expiration is out of range")?
        } else {
            parse_datetime(&data["expires_at"])
                .context("Claude refresh response missing a valid expiration")?
        };
        if expires <= now {
            bail!("Claude refresh response expiration is not in the future");
        }

        let oauth = self
            .root
            .get_mut("claudeAiOauth")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow!("credentials missing claudeAiOauth object"))?;

        self.access_token = access.clone();
        oauth.insert("accessToken".into(), json!(access));
        if !refresh.is_empty() {
            self.refresh_token = refresh.clone();
            oauth.insert("refreshToken".into(), json!(refresh));
        }
        self.expires_at = Some(expires);
        oauth.insert("expiresAt".into(), json!(expires.timestamp_millis()));
        Ok(())
    }

    async fn write_back(&self) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(&self.root)?;
        let source = self.source.clone();
        tokio::task::spawn_blocking(move || write_credentials(&source, &text))
            .await
            .context("join Claude credential writer")?
    }
}

async fn reload_credentials(source: CredSource) -> anyhow::Result<Creds> {
    tokio::task::spawn_blocking(move || reload_source(&source))
        .await
        .context("join Claude credential reload")?
}

struct ClaudeRefreshLock {
    #[cfg(windows)]
    wsl: WslRefreshLock,
}

impl ClaudeRefreshLock {
    fn acquire(source: &CredSource) -> anyhow::Result<Self> {
        #[cfg(windows)]
        if let CredSource::Wsl { distro, path } = source {
            return Ok(Self {
                wsl: WslRefreshLock::acquire(distro, path)?,
            });
        }

        let _ = source;
        bail!("direct OAuth refresh unavailable for local Claude credentials")
    }

    #[cfg(windows)]
    fn ensure_held(&mut self) -> anyhow::Result<()> {
        self.wsl.ensure_held()
    }

    #[cfg(not(windows))]
    fn ensure_held(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn acquire_refresh_lock(source: &CredSource) -> anyhow::Result<ClaudeRefreshLock> {
    let source = source.clone();
    tokio::task::spawn_blocking(move || ClaudeRefreshLock::acquire(&source))
        .await
        .context("join Claude refresh lock acquisition")?
}

struct OsFileLock {
    file: File,
}

impl OsFileLock {
    fn acquire(path: &Path, timeout: StdDuration) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create Claude recovery lock directory")?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open Claude recovery lock at {}", path.display()))?;
        let start = Instant::now();

        loop {
            if try_lock_file(&file)? {
                return Ok(Self { file });
            }
            if start.elapsed() >= timeout {
                bail!("Claude recovery lock timed out");
            }
            std::thread::sleep(StdDuration::from_millis(100));
        }
    }
}

impl Drop for OsFileLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

#[cfg(unix)]
fn try_lock_file(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(err)
    }
}

#[cfg(unix)]
fn unlock_file(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    const LOCK_UN: i32 = 8;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    if unsafe { flock(file.as_raw_fd(), LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock_file(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows::Win32::System::IO::OVERLAPPED;

    let handle = HANDLE(file.as_raw_handle());
    let mut overlapped = OVERLAPPED::default();
    match unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    } {
        Ok(()) => Ok(true),
        Err(err) => {
            let err = io::Error::from(err);
            if err.raw_os_error() == Some(33) {
                Ok(false)
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(windows)]
fn unlock_file(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::UnlockFileEx;
    use windows::Win32::System::IO::OVERLAPPED;

    let handle = HANDLE(file.as_raw_handle());
    let mut overlapped = OVERLAPPED::default();
    unsafe { UnlockFileEx(handle, 0, 1, 0, &mut overlapped) }.map_err(io::Error::from)
}

#[cfg(windows)]
struct WslRefreshLock {
    child: std::process::Child,
}

#[cfg(windows)]
impl WslRefreshLock {
    fn acquire(distro: &str, credentials_path: &str) -> anyhow::Result<Self> {
        use std::io::BufRead;

        const SCRIPT: &str = r#"set -u
credentials_path=$1
root=$(dirname -- "$credentials_path")
refresh_lock="$root/.oauth_refresh.lock"
legacy_lock="${root}.lock"
storage_lock="$root/.storage-write.lock"
have_refresh=0
have_legacy=0
have_storage=0
refresh_identity=
legacy_identity=
storage_identity=

identity() {
  stat -c '%d:%i:%w' -- "$1" 2>/dev/null
}

release_if_owned() {
  local lock_path=$1
  local expected_identity=$2
  if [ -n "$expected_identity" ] && [ "$(identity "$lock_path")" = "$expected_identity" ]; then
    rmdir -- "$lock_path" 2>/dev/null || true
  fi
}

release_locks() {
  if [ "$have_storage" -eq 1 ]; then release_if_owned "$storage_lock" "$storage_identity"; fi
  if [ "$have_legacy" -eq 1 ]; then release_if_owned "$legacy_lock" "$legacy_identity"; fi
  if [ "$have_refresh" -eq 1 ]; then release_if_owned "$refresh_lock" "$refresh_identity"; fi
}
trap release_locks EXIT
trap 'exit 0' HUP INT TERM

acquire_dir() {
  local lock_path=$1
  local stale_seconds=$2
  local modified
  local now
  if mkdir -- "$lock_path" 2>/dev/null; then return 0; fi
  modified=$(stat -c %Y -- "$lock_path" 2>/dev/null) || return 1
  now=$(date +%s)
  if [ $((now - modified)) -ge "$stale_seconds" ]; then
    rmdir -- "$lock_path" 2>/dev/null || return 1
    mkdir -- "$lock_path" 2>/dev/null && return 0
  fi
  return 1
}

attempt=0
while [ "$attempt" -lt 12 ]; do
  attempt=$((attempt + 1))
  if acquire_dir "$refresh_lock" 10; then
    have_refresh=1
    refresh_identity=$(identity "$refresh_lock")
    if [ -n "$refresh_identity" ] && acquire_dir "$legacy_lock" 10; then
      have_legacy=1
      legacy_identity=$(identity "$legacy_lock")
      if [ -n "$legacy_identity" ]; then break; fi
      release_if_owned "$legacy_lock" "$legacy_identity"
      have_legacy=0
    fi
    release_if_owned "$refresh_lock" "$refresh_identity"
    have_refresh=0
  fi
  sleep 1
done

if [ "$have_legacy" -ne 1 ]; then
  printf 'LOCKED\n'
  exit 75
fi

attempt=0
while [ "$attempt" -lt 20 ]; do
  attempt=$((attempt + 1))
  locks_intact=1
  if [ "$(identity "$refresh_lock")" != "$refresh_identity" ]; then have_refresh=0; locks_intact=0; fi
  if [ "$(identity "$legacy_lock")" != "$legacy_identity" ]; then have_legacy=0; locks_intact=0; fi
  if [ "$locks_intact" -ne 1 ]; then exit 76; fi
  touch -c -- "$refresh_lock" "$legacy_lock" || exit 76
  if acquire_dir "$storage_lock" 15; then
    have_storage=1
    storage_identity=$(identity "$storage_lock")
    if [ -n "$storage_identity" ]; then break; fi
    release_if_owned "$storage_lock" "$storage_identity"
    have_storage=0
  fi
  sleep 0.5
done

if [ "$have_storage" -ne 1 ]; then
  printf 'LOCKED\n'
  exit 75
fi

locks_intact=1
if [ "$(identity "$refresh_lock")" != "$refresh_identity" ]; then have_refresh=0; locks_intact=0; fi
if [ "$(identity "$legacy_lock")" != "$legacy_identity" ]; then have_legacy=0; locks_intact=0; fi
if [ "$(identity "$storage_lock")" != "$storage_identity" ]; then have_storage=0; locks_intact=0; fi
if [ "$locks_intact" -ne 1 ]; then exit 76; fi
touch -c -- "$refresh_lock" "$legacy_lock" "$storage_lock" || exit 76
printf 'ACQUIRED\n'
while :; do
  locks_intact=1
  if [ "$(identity "$refresh_lock")" != "$refresh_identity" ]; then have_refresh=0; locks_intact=0; fi
  if [ "$(identity "$legacy_lock")" != "$legacy_identity" ]; then have_legacy=0; locks_intact=0; fi
  if [ "$(identity "$storage_lock")" != "$storage_identity" ]; then have_storage=0; locks_intact=0; fi
  if [ "$locks_intact" -ne 1 ]; then exit 76; fi
  touch -c -- "$refresh_lock" "$legacy_lock" "$storage_lock" || exit 76
  IFS= read -r -t 2 _
  read_status=$?
  if [ "$read_status" -eq 0 ] || [ "$read_status" -eq 1 ]; then break; fi
done
"#;

        let mut child = Command::new("wsl.exe")
            .args([
                "-d",
                distro,
                "--exec",
                "bash",
                "-c",
                SCRIPT,
                "ai-dashboard-refresh-lock",
                credentials_path,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn WSL Claude refresh lock keeper")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no stdout for WSL Claude refresh lock keeper"))?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = std::io::BufReader::new(stdout)
                .read_line(&mut line)
                .map(|_| line);
            let _ = sender.send(result);
        });

        let line = match receiver.recv_timeout(StdDuration::from_secs(30)) {
            Ok(Ok(line)) => line,
            Ok(Err(err)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err).context("read WSL Claude refresh lock status");
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("WSL Claude refresh lock timed out");
            }
        };

        if line.trim() != "ACQUIRED" {
            let _ = child.wait();
            bail!("Claude refresh lock is held by another process");
        }
        Ok(Self { child })
    }

    fn ensure_held(&mut self) -> anyhow::Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .context("poll WSL Claude refresh lock")?
        {
            bail!("WSL Claude refresh lock keeper exited with {status}");
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for WslRefreshLock {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = wait_for_child(
            &mut self.child,
            StdDuration::from_secs(5),
            "WSL Claude refresh lock keeper",
        );
    }
}

fn write_credentials(source: &CredSource, text: &str) -> anyhow::Result<()> {
    match source {
        CredSource::File(path) => {
            atomic_write(path, text.as_bytes()).context("write Claude credentials")?
        }
        #[cfg(windows)]
        CredSource::Wsl { distro, path } => wsl_write(distro, path, text)?,
    }
    Ok(())
}

async fn try_claude_code_refresh(config: &Config, creds: &mut Creds) -> anyhow::Result<()> {
    let refresh_config = config.clone();
    let mut candidate = creds.clone();
    let refreshed = tokio::task::spawn_blocking(move || {
        try_claude_code_refresh_blocking(&refresh_config, &mut candidate)?;
        Ok::<Creds, anyhow::Error>(candidate)
    })
    .await
    .context("join Claude Code refresh")??;
    *creds = refreshed;
    Ok(())
}

fn try_claude_code_refresh_blocking(config: &Config, creds: &mut Creds) -> anyhow::Result<()> {
    if !config.claude_code_refresh_enabled {
        bail!("Claude Code refresh disabled");
    }
    if !reserve_claude_code_recovery()? {
        bail!("Claude Code recovery is temporarily throttled");
    }

    let before_access = creds.access_token.clone();
    let before_expires = creds.expires_at;

    let mut cmd = build_claude_code_refresh_command(config, &creds.source);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    let timeout = StdDuration::from_secs(config.claude_code_refresh_timeout_seconds.clamp(
        MIN_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
        MAX_CLAUDE_CODE_REFRESH_TIMEOUT_SECONDS,
    ));
    let run_result = run_with_timeout(cmd, timeout);

    // Claude Code may refresh credentials before a non-zero exit, e.g. when the
    // configured budget is too low for the actual tiny prompt. Trust the file,
    // not the process status, and never expose command output to the renderer.
    let refreshed = load(config)?;
    let has_new_access =
        refreshed.access_token != before_access || refreshed.expires_at != before_expires;
    let is_fresh = match refreshed.expires_at {
        Some(exp) => exp > Utc::now() + Duration::minutes(5),
        None => !refreshed.access_token.is_empty(),
    };

    if has_new_access && is_fresh {
        *creds = refreshed;
        return Ok(());
    }

    run_result?;
    bail!("Claude Code did not refresh credentials")
}

fn reserve_claude_code_recovery() -> anyhow::Result<bool> {
    reserve_claude_code_recovery_at(&cache_dir(), Utc::now())
}

fn reserve_claude_code_recovery_at(dir: &Path, now: DateTime<Utc>) -> anyhow::Result<bool> {
    std::fs::create_dir_all(dir).context("create Claude recovery state directory")?;
    let lock_path = dir.join(CLAUDE_CODE_RECOVERY_LOCK_FILE);
    let _lock = OsFileLock::acquire(&lock_path, RECOVERY_LOCK_WAIT)?;
    let state_path = dir.join(CLAUDE_CODE_RECOVERY_STATE_FILE);

    let last_attempt = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|text| DateTime::parse_from_rfc3339(text.trim()).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc));
    if last_attempt.is_some_and(|timestamp| now - timestamp < CLAUDE_CODE_RECOVERY_THROTTLE) {
        return Ok(false);
    }

    atomic_write(&state_path, now.to_rfc3339().as_bytes())
        .context("persist Claude recovery attempt")?;
    Ok(true)
}

fn build_claude_code_refresh_command(config: &Config, _source: &CredSource) -> Command {
    #[cfg(windows)]
    {
        if let Some(distro) = _source.wsl_distro() {
            let mut cmd = Command::new("wsl.exe");
            let shell = claude_code_refresh_shell(config);
            cmd.args(["-d", &distro, "--exec", "bash", "-lc", &shell]);
            return cmd;
        }
    }

    let mut cmd = Command::new(config.claude_code_command.trim());
    add_claude_code_refresh_args(&mut cmd, config);
    cmd
}

fn add_claude_code_refresh_args(cmd: &mut Command, config: &Config) {
    let budget = claude_code_refresh_budget(config);
    cmd.args([
        "-p",
        "OK",
        "--output-format",
        "json",
        "--tools",
        "",
        "--model",
        "claude-haiku-4-5-20251001",
        "--max-budget-usd",
        &budget,
    ]);
}

fn claude_code_refresh_budget(config: &Config) -> String {
    let configured = config.claude_code_refresh_max_budget_usd;
    if configured.is_finite() && configured > 0.0 {
        configured
            .clamp(
                MIN_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
                MAX_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD,
            )
            .to_string()
    } else {
        DEFAULT_CLAUDE_CODE_REFRESH_MAX_BUDGET_USD.to_string()
    }
}

#[cfg(windows)]
fn claude_code_refresh_shell(config: &Config) -> String {
    format!(
        "PATH=\"$HOME/.local/bin:$PATH\"; {} -p OK --output-format json --tools '' --model claude-haiku-4-5-20251001 --max-budget-usd {}",
        shell_quote(config.claude_code_command.trim()),
        shell_quote(&claude_code_refresh_budget(config))
    )
}

#[cfg(windows)]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn run_with_timeout(mut cmd: Command, timeout: StdDuration) -> anyhow::Result<()> {
    let mut child = cmd.spawn().context("spawn Claude Code refresh")?;
    let start = Instant::now();

    loop {
        if let Some(status) = child.try_wait().context("poll Claude Code refresh")? {
            if status.success() {
                return Ok(());
            }
            bail!("Claude Code refresh exited with {status}");
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Claude Code refresh timed out");
        }

        std::thread::sleep(StdDuration::from_millis(250));
    }
}

fn str_field(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn load(config: &Config) -> anyhow::Result<Creds> {
    let (text, source) = resolve_and_read(config)?;
    parse_credentials(&text, source)
}

fn parse_credentials(text: &str, source: CredSource) -> anyhow::Result<Creds> {
    let root: Value = serde_json::from_str(text).context("parse Claude credentials JSON")?;
    let oauth = &root["claudeAiOauth"];
    let access = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let refresh = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let expires_at = parse_datetime(&oauth["expiresAt"]);
    if access.is_empty() && refresh.is_empty() {
        bail!("Claude OAuth token missing");
    }
    Ok(Creds {
        root,
        access_token: access,
        refresh_token: refresh,
        expires_at,
        source,
    })
}

fn read_source(source: &CredSource) -> anyhow::Result<String> {
    match source {
        CredSource::File(path) => std::fs::read_to_string(path)
            .with_context(|| format!("read Claude credentials at {}", path.display())),
        #[cfg(windows)]
        CredSource::Wsl { distro, path } => wsl_read(distro, path)
            .ok_or_else(|| anyhow!("Claude WSL credentials not readable: {distro}:{path}")),
    }
}

fn reload_source(source: &CredSource) -> anyhow::Result<Creds> {
    parse_credentials(&read_source(source)?, source.clone())
}

fn resolve_and_read(config: &Config) -> anyhow::Result<(String, CredSource)> {
    let configured = config.claude_credentials_path.trim();

    if !configured.is_empty() {
        if let Some(spec) = configured.strip_prefix("wsl:") {
            #[cfg(windows)]
            {
                let (distro, path) = spec
                    .split_once(':')
                    .filter(|(distro, path)| !distro.is_empty() && !path.is_empty())
                    .context("Claude WSL credential path must be wsl:<distro>:<path>")?;
                let source = CredSource::Wsl {
                    distro: distro.to_string(),
                    path: path.to_string(),
                };
                return Ok((read_source(&source)?, source));
            }
            #[cfg(not(windows))]
            {
                let _ = spec;
                bail!("Claude WSL credential paths are only supported on Windows");
            }
        } else {
            let p = PathBuf::from(expand(configured));
            let source = CredSource::File(p);
            return Ok((read_source(&source)?, source));
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".claude").join(".credentials.json"));
        candidates.push(home.join(".claude").join("credentials.json"));
    }
    for p in candidates {
        if let Ok(text) = std::fs::read_to_string(&p) {
            if !text.trim().is_empty() {
                return Ok((text, CredSource::File(p)));
            }
        }
    }

    #[cfg(windows)]
    {
        for relative_path in [".claude/.credentials.json", ".claude/credentials.json"] {
            if let Some((text, path)) = wsl_read_home_file("Ubuntu", relative_path) {
                return Ok((
                    text,
                    CredSource::Wsl {
                        distro: "Ubuntu".into(),
                        path,
                    },
                ));
            }
        }
    }

    bail!("Claude credentials not found")
}

/// Minimal expansion of a leading `~` and `%VAR%` segments.
fn expand(p: &str) -> String {
    let mut s = p.to_string();
    if let Some(rest) = s.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            s = format!("{}{}", home.display(), rest);
        }
    }
    while let Some(start) = s.find('%') {
        if let Some(end_rel) = s[start + 1..].find('%') {
            let end = start + 1 + end_rel;
            let var = &s[start + 1..end];
            let val = std::env::var(var).unwrap_or_default();
            s = format!("{}{}{}", &s[..start], val, &s[end + 1..]);
        } else {
            break;
        }
    }
    s
}

#[cfg(windows)]
fn wsl_read(distro: &str, path: &str) -> Option<String> {
    if let Some(candidates) = wsl_unc_candidates(distro, path) {
        for candidate in candidates {
            if let Ok(text) = std::fs::read_to_string(candidate) {
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
    }

    for attempt in 0..2 {
        let mut command = Command::new("wsl.exe");
        command.args(["-d", distro, "--exec", "cat", path]);
        let out = match command_output_with_timeout(command, WSL_IO_TIMEOUT) {
            Ok(out) => out,
            Err(_) if attempt == 0 => {
                std::thread::sleep(StdDuration::from_millis(250));
                continue;
            }
            Err(_) => return None,
        };
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
        if attempt == 0 {
            std::thread::sleep(StdDuration::from_millis(250));
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
    let relative = components.join("\\");

    Some([
        PathBuf::from(format!(r"\\wsl.localhost\{distro}\{relative}")),
        PathBuf::from(format!(r"\\wsl$\{distro}\{relative}")),
    ])
}

#[cfg(windows)]
fn wsl_read_home_file(distro: &str, relative_path: &str) -> Option<(String, String)> {
    let shell = format!(
        "p=\"$HOME\"/{}; [ -s \"$p\" ] || exit 1; printf '%s\\n' \"$p\"; cat \"$p\"",
        shell_quote(relative_path)
    );
    for attempt in 0..2 {
        let mut command = Command::new("wsl.exe");
        command.args(["-d", distro, "--exec", "bash", "-lc", &shell]);
        let out = match command_output_with_timeout(command, WSL_IO_TIMEOUT) {
            Ok(out) => out,
            Err(_) if attempt == 0 => {
                std::thread::sleep(StdDuration::from_millis(250));
                continue;
            }
            Err(_) => return None,
        };
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            if let Some((path, text)) = stdout.split_once('\n') {
                if !text.trim().is_empty() {
                    return Some((text.to_string(), path.to_string()));
                }
            }
        }
        if attempt == 0 {
            std::thread::sleep(StdDuration::from_millis(250));
        }
    }
    None
}

#[cfg(windows)]
fn wsl_write(distro: &str, path: &str, text: &str) -> anyhow::Result<()> {
    use std::io::Write;

    const SCRIPT: &str = r#"set -eu
path=$1
if [ -L "$path" ]; then
  path=$(readlink -f -- "$path")
fi
dir=$(dirname -- "$path")
base=$(basename -- "$path")
tmp=$(mktemp "$dir/.${base}.tmp.XXXXXX")
cleanup() { rm -f -- "$tmp"; }
trap cleanup EXIT HUP INT TERM
cat > "$tmp"
if [ -e "$path" ]; then
  chmod --reference="$path" "$tmp"
fi
sync -f "$tmp"
mv -f -- "$tmp" "$path"
trap - EXIT HUP INT TERM
"#;

    let mut child = Command::new("wsl.exe")
        .args([
            "-d",
            distro,
            "--exec",
            "bash",
            "-c",
            SCRIPT,
            "ai-dashboard-writer",
            path,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn atomic WSL credential writer")?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("no stdin for WSL credential writer"))?;
    stdin.write_all(text.as_bytes())?;
    drop(stdin);

    if !wait_for_child(&mut child, WSL_IO_TIMEOUT, "WSL credential writer")?.success() {
        bail!("atomic WSL credential write failed");
    }
    Ok(())
}

#[cfg(windows)]
const WSL_IO_TIMEOUT: StdDuration = StdDuration::from_secs(15);

#[cfg(windows)]
fn command_output_with_timeout(
    mut command: Command,
    timeout: StdDuration,
) -> anyhow::Result<std::process::Output> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("spawn WSL reader")?;
    let start = Instant::now();

    loop {
        if child.try_wait().context("poll WSL reader")?.is_some() {
            return child
                .wait_with_output()
                .context("collect WSL reader output");
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("WSL reader timed out");
        }
        std::thread::sleep(StdDuration::from_millis(50));
    }
}

#[cfg(windows)]
fn wait_for_child(
    child: &mut std::process::Child,
    timeout: StdDuration,
    description: &str,
) -> anyhow::Result<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("poll child process")? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{description} timed out");
        }
        std::thread::sleep(StdDuration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        claude_code_refresh_budget, parse_usage, recovery_policy, refresh_request,
        reserve_claude_code_recovery_at, resolve_and_read, validate_refresh_response, CredSource,
        Creds, OsFileLock, RefreshError, OAUTH_SCOPES, TOKEN_URL,
    };
    use crate::config::Config;
    use crate::fetchers::Resp;
    use chrono::{Duration, TimeZone, Utc};
    use reqwest::Client;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

    #[cfg(windows)]
    use super::wsl_unc_candidates;

    #[cfg(windows)]
    struct WslTestDirectory {
        distro: &'static str,
        root: String,
    }

    #[cfg(windows)]
    impl Drop for WslTestDirectory {
        fn drop(&mut self) {
            let _ = std::process::Command::new("wsl.exe")
                .args(["-d", self.distro, "--exec", "rm", "-rf", "--", &self.root])
                .status();
        }
    }

    fn test_creds() -> Creds {
        Creds {
            root: json!({
                "claudeAiOauth": {
                    "accessToken": "old-access",
                    "refreshToken": "old-refresh",
                    "expiresAt": 0
                },
                "oauthAccount": {"displayName": "preserve me"}
            }),
            access_token: "old-access".into(),
            refresh_token: "old-refresh".into(),
            expires_at: None,
            source: CredSource::File(PathBuf::from("unused-test-credentials.json")),
        }
    }

    fn test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ai-usage-dashboard-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn refresh_request_matches_current_claude_code_contract() {
        let request = refresh_request(&Client::new(), "refresh-secret")
            .build()
            .expect("refresh request");
        assert_eq!(request.url().as_str(), TOKEN_URL);

        let body: Value = serde_json::from_slice(
            request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("JSON request body"),
        )
        .expect("valid JSON request body");
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "refresh-secret");
        assert_eq!(body["scope"], OAUTH_SCOPES);
    }

    #[test]
    fn token_refresh_rate_limit_preserves_retry_after() {
        for retry_after in [Some(91), None] {
            let resp = Resp {
                status: 429,
                body: String::new(),
                retry_after,
            };

            match validate_refresh_response(&resp) {
                Err(RefreshError::RateLimited {
                    retry_after: actual,
                }) => assert_eq!(actual, retry_after),
                other => panic!("expected typed rate limit, got {other:?}"),
            }
        }
    }

    #[test]
    fn token_refresh_only_classifies_invalid_grant_as_expired_auth() {
        for body in [
            r#"{"error":"invalid_grant"}"#,
            r#"{"error":{"type":"invalid_grant","message":"expired"}}"#,
        ] {
            let resp = Resp {
                status: 400,
                body: body.into(),
                retry_after: None,
            };
            assert!(matches!(
                validate_refresh_response(&resp),
                Err(RefreshError::InvalidGrant)
            ));
        }

        for body in [r#"{"error":"invalid_scope"}"#, "not JSON"] {
            let resp = Resp {
                status: 400,
                body: body.into(),
                retry_after: None,
            };
            match validate_refresh_response(&resp) {
                Err(RefreshError::Other(err)) => {
                    assert_eq!(err.to_string(), "Claude token refresh HTTP 400")
                }
                other => panic!("expected recoverable refresh error, got {other:?}"),
            }
        }

        let forbidden = Resp {
            status: 403,
            body: "not exposed".into(),
            retry_after: None,
        };
        assert!(matches!(
            validate_refresh_response(&forbidden),
            Err(RefreshError::Forbidden)
        ));
    }

    #[test]
    fn cli_recovery_policy_excludes_transient_and_parse_failures() {
        for err in [
            RefreshError::MissingRefreshToken,
            RefreshError::InvalidGrant,
            RefreshError::DirectRefreshUnavailable,
            RefreshError::Forbidden,
        ] {
            assert!(recovery_policy(&err).0, "expected CLI recovery for {err:?}");
        }

        for err in [
            RefreshError::Other(anyhow::anyhow!("transport failure")),
            RefreshError::Other(anyhow::anyhow!("HTTP 500")),
            RefreshError::Other(anyhow::anyhow!("parse refresh response")),
            RefreshError::RateLimited { retry_after: None },
        ] {
            assert!(
                !recovery_policy(&err).0,
                "unexpected CLI recovery for {err:?}"
            );
        }
    }

    #[test]
    fn local_credentials_never_use_direct_oauth_refresh() {
        let mut creds = test_creds();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");

        let result = runtime.block_on(creds.refresh(&Client::new()));

        assert!(matches!(
            result,
            Err(RefreshError::DirectRefreshUnavailable)
        ));
    }

    #[test]
    fn cli_recovery_budget_is_defensively_capped() {
        let too_high = Config {
            claude_code_refresh_max_budget_usd: 50.0,
            ..Config::default()
        };
        let invalid = Config {
            claude_code_refresh_max_budget_usd: f64::NAN,
            ..Config::default()
        };

        assert_eq!(claude_code_refresh_budget(&too_high), "0.1");
        assert_eq!(claude_code_refresh_budget(&invalid), "0.03");
    }

    #[test]
    fn refresh_response_preserves_claude_code_expiration_format() {
        let mut creds = test_creds();
        let before = Utc::now().timestamp_millis();

        creds
            .apply_refresh_response(&json!({
                "access_token": "new-access",
                "expires_in": 3600
            }))
            .expect("valid refresh response");

        let oauth = &creds.root["claudeAiOauth"];
        assert_eq!(oauth["accessToken"], "new-access");
        assert_eq!(oauth["refreshToken"], "old-refresh");
        assert_eq!(creds.root["oauthAccount"]["displayName"], "preserve me");
        let expires_at = oauth["expiresAt"]
            .as_i64()
            .expect("expiresAt remains epoch milliseconds");
        assert!(expires_at >= before + 3_599_000);
        assert!(expires_at <= Utc::now().timestamp_millis() + 3_601_000);
    }

    #[test]
    fn refresh_response_prefers_positive_expires_in() {
        let mut creds = test_creds();
        let before = Utc::now().timestamp_millis();

        creds
            .apply_refresh_response(&json!({
                "access_token": "new-access",
                "expires_in": 3600,
                "expires_at": "2000-01-01T00:00:00Z"
            }))
            .expect("expires_in takes precedence");

        let expires_at = creds.root["claudeAiOauth"]["expiresAt"]
            .as_i64()
            .expect("epoch milliseconds");
        assert!(expires_at >= before + 3_599_000);
    }

    #[test]
    fn configured_credential_path_is_fail_closed() {
        let config = Config {
            claude_credentials_path: std::env::temp_dir()
                .join(format!(
                    "missing-ai-dashboard-claude-credentials-{}",
                    std::process::id()
                ))
                .display()
                .to_string(),
            ..Config::default()
        };

        assert!(resolve_and_read(&config).is_err());

        #[cfg(not(windows))]
        {
            let mut config = config;
            config.claude_credentials_path =
                "wsl:Ubuntu:/home/user/.claude/.credentials.json".into();
            assert!(resolve_and_read(&config).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_candidates_are_exact_and_fail_closed() {
        assert_eq!(
            wsl_unc_candidates("Ubuntu", "/root/.claude/.credentials.json"),
            Some([
                PathBuf::from(r"\\wsl.localhost\Ubuntu\root\.claude\.credentials.json"),
                PathBuf::from(r"\\wsl$\Ubuntu\root\.claude\.credentials.json"),
            ])
        );
        assert_eq!(
            wsl_unc_candidates("Ubuntu-24.04", "/home/user/credentials.json"),
            Some([
                PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\user\credentials.json"),
                PathBuf::from(r"\\wsl$\Ubuntu-24.04\home\user\credentials.json"),
            ])
        );

        for distro in [
            "",
            ".",
            "..",
            "Ubuntu/other",
            r"Ubuntu\other",
            "Ubuntu:other",
        ] {
            assert!(wsl_unc_candidates(distro, "/root/credentials.json").is_none());
        }
        for path in [
            "root/credentials.json",
            "/",
            "/root//credentials.json",
            "/root/../credentials.json",
            r"/root/dir\credentials.json",
            "/root/credentials:alternate",
            "/root/credentials\0.json",
            "/root/trailing./credentials.json",
            "/root/trailing /credentials.json",
            "/root/question?/credentials.json",
            "/root/quote\"/credentials.json",
            "/root/NUL/credentials.json",
            "/root/com1.txt/credentials.json",
            "/root/LPT9/credentials.json",
        ] {
            assert!(wsl_unc_candidates("Ubuntu", path).is_none());
        }
    }

    #[test]
    fn recovery_file_lock_serializes_dashboard_processes() {
        let dir = test_dir("recovery-lock");
        let path = dir.join("recovery.lock");
        let first =
            OsFileLock::acquire(&path, StdDuration::from_secs(1)).expect("first recovery lock");
        assert!(OsFileLock::acquire(&path, StdDuration::ZERO).is_err());
        drop(first);
        OsFileLock::acquire(&path, StdDuration::from_secs(1)).expect("recovery lock after release");
        std::fs::remove_dir_all(dir).expect("remove recovery lock test directory");
    }

    #[test]
    fn failed_cli_attempt_consumes_persistent_throttle_slot() {
        let dir = test_dir("recovery-throttle");
        let now = Utc
            .with_ymd_and_hms(2026, 7, 11, 10, 0, 0)
            .single()
            .expect("fixed timestamp");

        assert!(reserve_claude_code_recovery_at(&dir, now).expect("reserve first attempt"));
        assert!(
            !reserve_claude_code_recovery_at(&dir, now + Duration::minutes(29))
                .expect("throttle second attempt")
        );
        assert!(
            reserve_claude_code_recovery_at(&dir, now + Duration::minutes(30))
                .expect("reserve after throttle")
        );

        std::fs::remove_dir_all(dir).expect("remove recovery throttle test directory");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the Ubuntu WSL distribution"]
    fn wsl_refresh_lock_creates_heartbeats_and_releases_claude_code_locks() {
        use super::WslRefreshLock;
        use std::process::Command;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after Unix epoch")
            .as_nanos();
        let root = format!("/tmp/ai-dashboard-refresh-lock-{unique}");
        let _cleanup = WslTestDirectory {
            distro: "Ubuntu",
            root: root.clone(),
        };
        let credentials = format!("{root}/.credentials.json");
        let status = Command::new("wsl.exe")
            .args([
                "-d",
                "Ubuntu",
                "--exec",
                "bash",
                "-c",
                "mkdir -p -- \"$1\"",
                "lock-test",
                &root,
            ])
            .status()
            .expect("create WSL test directory");
        assert!(status.success());

        let mut lock = WslRefreshLock::acquire("Ubuntu", &credentials).expect("WSL refresh lock");
        lock.ensure_held().expect("lock keeper is running");
        std::thread::sleep(StdDuration::from_millis(2_200));
        lock.ensure_held().expect("lock heartbeat is running");

        let status = Command::new("wsl.exe")
            .args([
                "-d",
                "Ubuntu",
                "--exec",
                "bash",
                "-c",
                "[ -d \"$1/.oauth_refresh.lock\" ] && [ -d \"${1}.lock\" ] && [ -d \"$1/.storage-write.lock\" ]",
                "lock-test",
                &root,
            ])
            .status()
            .expect("inspect WSL lock directories");
        assert!(status.success());
        drop(lock);

        let status = Command::new("wsl.exe")
            .args([
                "-d",
                "Ubuntu",
                "--exec",
                "bash",
                "-c",
                "[ ! -e \"$1/.oauth_refresh.lock\" ] && [ ! -e \"${1}.lock\" ] && [ ! -e \"$1/.storage-write.lock\" ]",
                "lock-test",
                &root,
            ])
            .status()
            .expect("verify WSL lock release");
        assert!(status.success());

        let mut compromised =
            WslRefreshLock::acquire("Ubuntu", &credentials).expect("second WSL refresh lock");
        let status = Command::new("wsl.exe")
            .args([
                "-d",
                "Ubuntu",
                "--exec",
                "bash",
                "-c",
                "rmdir -- \"$1/.storage-write.lock\" && mkdir -- \"$1/.storage-write.lock\"",
                "lock-test",
                &root,
            ])
            .status()
            .expect("replace WSL storage lock directory");
        assert!(status.success());
        let deadline = std::time::Instant::now() + StdDuration::from_secs(6);
        let lock_loss_detected = loop {
            if compromised.ensure_held().is_err() {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(StdDuration::from_millis(100));
        };
        assert!(lock_loss_detected, "lock keeper did not detect replacement");
        drop(compromised);

        let status = Command::new("wsl.exe")
            .args([
                "-d",
                "Ubuntu",
                "--exec",
                "bash",
                "-c",
                "[ ! -e \"$1/.oauth_refresh.lock\" ] && [ ! -e \"${1}.lock\" ] && [ -d \"$1/.storage-write.lock\" ] && rmdir -- \"$1/.storage-write.lock\" && rmdir -- \"$1\"",
                "lock-test",
                &root,
            ])
            .status()
            .expect("verify compromised lock ownership");
        assert!(status.success());
    }

    #[test]
    fn malformed_refresh_response_does_not_mutate_credentials() {
        for response in [
            json!({"expires_in": 3600}),
            json!({"access_token": "new-access"}),
            json!({"access_token": "new-access", "expires_in": 0}),
            json!({"access_token": "new-access", "expires_at": "2000-01-01T00:00:00Z"}),
        ] {
            let mut creds = test_creds();
            let original = creds.root.clone();
            assert!(creds.apply_refresh_response(&response).is_err());
            assert_eq!(creds.root, original);
            assert_eq!(creds.access_token, "old-access");
            assert_eq!(creds.refresh_token, "old-refresh");
        }
    }

    #[test]
    fn claude_utilization_is_already_percent_scale() {
        let service = parse_usage(
            r#"{
                "five_hour":{"utilization":0.42,"resets_at":"2026-07-10T08:30:00Z"},
                "seven_day":{"utilization":1,"resets_at":"2026-07-17T08:30:00Z"}
            }"#,
        )
        .expect("valid Claude usage");

        assert_eq!(service.five_hour_percent, Some(0.42));
        assert_eq!(service.seven_day_percent, Some(1.0));
    }

    #[test]
    fn claude_usage_requires_both_windows_and_utilization() {
        for body in [
            r#"{}"#,
            r#"{"five_hour":{"utilization":1}}"#,
            r#"{"five_hour":{"utilization":1},"seven_day":{}}"#,
        ] {
            assert!(parse_usage(body).is_err(), "unexpectedly accepted {body}");
        }
    }
}
