//! Small shared helpers: percent normalization and date parsing/formatting.

use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::Value;

/// Clamp values that are already reported as 0..100 percentages, such as Codex
/// `used_percent` and Claude `utilization`. `1` means 1%, not 100%.
pub fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

/// Parse a JSON value that may be an RFC3339 string, a numeric epoch (seconds
/// or milliseconds), or a numeric string, into a UTC timestamp.
pub fn parse_datetime(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                from_epoch(i)
            } else {
                n.as_f64().and_then(|f| from_epoch(f as i64))
            }
        }
        Value::String(s) => parse_datetime_str(s),
        _ => None,
    }
}

pub fn parse_datetime_str(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Numeric string epoch.
    if let Ok(i) = s.parse::<i64>() {
        return from_epoch(i);
    }
    None
}

fn from_epoch(value: i64) -> Option<DateTime<Utc>> {
    // Heuristic: > ~year 2001 in ms means it's milliseconds.
    let secs = if value.unsigned_abs() > 100_000_000_000 {
        value / 1000
    } else {
        value
    };
    Utc.timestamp_opt(secs, 0).single()
}

/// Format a UTC timestamp as local "MM-DD HH:mm".
pub fn fmt_local(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%m-%d %H:%M").to_string()
}

/// Convenience: parse a JSON value to a local "MM-DD HH:mm" label, if possible.
pub fn local_label(value: &Value) -> Option<String> {
    parse_datetime(value).map(fmt_local)
}
