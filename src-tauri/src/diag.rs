//! Local diagnostic log: `diagnostics.log` beside the usage cache. It records
//! why a provider card is not live, one line each time the reason changes, so
//! a status such as SESSION EXPIRED can be traced without a debugger.
//!
//! Lines hold a timestamp, the provider, and a sanitized reason: status text,
//! HTTP status codes, and error descriptions with the home directory shortened
//! to `~`. Callers must never pass tokens, keys, or response bodies.

use crate::cache::cache_dir;
use chrono::{SecondsFormat, Utc};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

const LOG_FILE: &str = "diagnostics.log";
const OLD_LOG_FILE: &str = "diagnostics.log.1";
const MAX_LOG_BYTES: u64 = 256 * 1024;
const MAX_LINE_CHARS: usize = 400;

static LAST_REASON: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Record the outcome of a provider fetch. Repeats of the same outcome are
/// skipped, so the log shows transitions rather than every refresh cycle.
pub fn outcome(provider: &str, reason: &str) {
    if record(provider, reason) {
        // A new outcome needs its reason next to it, even one seen before.
        if let Ok(mut guard) = LAST_REASON.lock() {
            if let Some(last) = guard.as_mut() {
                last.remove(&detail_subject(provider));
            }
        }
    }
}

/// Record a detail behind an outcome, such as why a token renewal failed.
pub fn detail(provider: &str, reason: &str) {
    record(&detail_subject(provider), reason);
}

fn detail_subject(provider: &str) -> String {
    format!("{provider} detail")
}

/// Whether the reason differs from the last one written for the subject.
fn record(subject: &str, reason: &str) -> bool {
    // Unit tests exercise the fetch paths; keep them out of the user's log.
    if cfg!(test) {
        return false;
    }
    let reason = sanitize(reason, dirs::home_dir().as_deref());
    let Ok(mut guard) = LAST_REASON.lock() else {
        return false;
    };
    let last = guard.get_or_insert_with(HashMap::new);
    if last.get(subject) == Some(&reason) {
        return false;
    }
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    // A line that could not be written is tried again next time.
    if append_line(&dir, subject, &reason).is_ok() {
        last.insert(subject.to_string(), reason);
    }
    true
}

fn sanitize(reason: &str, home: Option<&Path>) -> String {
    let mut text = reason.to_string();
    if let Some(home) = home.and_then(Path::to_str).filter(|home| home.len() > 1) {
        text = text.replace(home, "~");
    }
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(MAX_LINE_CHARS)
        .collect()
}

fn append_line(dir: &Path, subject: &str, reason: &str) -> std::io::Result<()> {
    let path = dir.join(LOG_FILE);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() >= MAX_LOG_BYTES) {
        std::fs::rename(&path, dir.join(OLD_LOG_FILE))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    writeln!(file, "{now} [{subject}] {reason}")
}

#[cfg(test)]
mod tests {
    use super::{append_line, sanitize, LOG_FILE, MAX_LOG_BYTES, OLD_LOG_FILE};
    use std::path::Path;

    #[test]
    fn reasons_lose_the_home_directory_control_characters_and_excess_length() {
        let home = Path::new("/home/someone");
        assert_eq!(
            sanitize(
                "read Codex file /home/someone/.codex/auth.json\nNo such file",
                Some(home)
            ),
            "read Codex file ~/.codex/auth.json No such file"
        );
        assert_eq!(sanitize(&"x".repeat(900), Some(home)).len(), 400);
    }

    #[test]
    fn log_appends_lines_and_keeps_one_old_file_when_full() {
        let dir = std::env::temp_dir().join(format!("ai-dashboard-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        append_line(&dir, "codex", "auth: LOGIN REQUIRED").unwrap();
        append_line(&dir, "codex", "ok").unwrap();
        let text = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("[codex] auth: LOGIN REQUIRED"));
        assert!(lines[1].ends_with("[codex] ok"));

        std::fs::write(dir.join(LOG_FILE), vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        append_line(&dir, "claude", "ok").unwrap();
        assert_eq!(
            std::fs::metadata(dir.join(OLD_LOG_FILE)).unwrap().len(),
            MAX_LOG_BYTES
        );
        let text = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert_eq!(text.lines().count(), 1);

        std::fs::remove_dir_all(dir).unwrap();
    }
}
