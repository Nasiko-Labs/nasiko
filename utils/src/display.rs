//! Shared `#[tabled(display = "...")]` helpers for CLI table rendering.
//!
//! `Option<T>` has no `Display` impl in Rust, so every optional table column
//! needs one of these to render `None` as something other than a compile
//! error. Used by both `nasiko` (oss/cli) and `nasiko-ee` (ee/cli).

use std::fmt::Display;

/// Truncates `s` to at most `n` bytes, snapping to the nearest valid UTF-8
/// boundary at or before `n` instead of panicking mid-character.
pub fn trunc(s: &str, n: usize) -> String {
    s.get(..n).unwrap_or(s).to_string()
}

/// `Some(v)` -> `v.to_string()`, `None` -> `"-"`.
pub fn opt_dash<T: Display>(o: &Option<T>) -> String {
    o.as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".into())
}

/// `Some(v)` -> `v.to_string()`, `None` -> `default`.
pub fn opt_or(o: &Option<String>, default: &str) -> String {
    o.as_deref().unwrap_or(default).to_string()
}

/// Rounds a millisecond duration to the nearest whole number; `None` -> `"-"`.
pub fn opt_round(o: &Option<f64>) -> String {
    o.map(|v| format!("{v:.0}")).unwrap_or_else(|| "-".into())
}

/// Same as [`opt_round`] with a trailing `ms` unit.
pub fn opt_lat_ms(o: &Option<f64>) -> String {
    o.map(|v| format!("{v:.0}ms")).unwrap_or_else(|| "-".into())
}

/// Formats a cost as `$X.XXXX`; `None` -> `"-"`.
pub fn opt_cost(o: &Option<f64>) -> String {
    o.map(|v| format!("${v:.4}")).unwrap_or_else(|| "-".into())
}

/// Truncates an ISO-8601 timestamp to its `YYYY-MM-DDTHH:MM:SS` prefix;
/// `None` -> `"-"`.
pub fn opt_started(o: &Option<String>) -> String {
    trunc(o.as_deref().unwrap_or("-"), 19)
}

/// `true` -> `"yes"`, `false` -> `"no"`.
pub fn yes_no(b: &bool) -> String {
    if *b { "yes" } else { "no" }.to_string()
}

/// `Some(role)` -> the role, `None` -> `"superuser"`/`"member"` based on `is_superuser`.
pub fn role_or(role: &Option<String>, is_superuser: &bool) -> String {
    role.as_deref()
        .unwrap_or(if *is_superuser { "superuser" } else { "member" })
        .to_string()
}
