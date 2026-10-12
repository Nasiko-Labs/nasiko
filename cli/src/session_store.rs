//! Remembers the *current conversation* per agent so `nasiko chat <agent>`
//! continues the same session across invocations instead of starting a new one
//! every turn.
//!
//! Keyed by the resolved chat **endpoint** (which already encodes the cluster host
//! and the agent UUID), so switching agents or clusters naturally selects a
//! different current conversation. The map lives in `~/.nasiko/sessions.json`,
//! owner-only. Everything here is best-effort: a read/write failure just means the
//! CLI falls back to the server minting a fresh session, never an error — so a
//! missing or unreadable file never blocks a chat.

use std::collections::HashMap;
use std::path::PathBuf;

fn store_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nasiko")
        .join("sessions.json")
}

fn load() -> HashMap<String, String> {
    let path = store_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// The current session id for `endpoint`, if one was remembered.
pub fn current(endpoint: &str) -> Option<String> {
    load().get(endpoint).cloned()
}

/// Remember `session_id` as the current conversation for `endpoint`. Best-effort:
/// any filesystem error is swallowed (the chat already succeeded; losing the
/// pointer only costs continuity on the next turn).
pub fn set_current(endpoint: &str, session_id: &str) {
    let mut map = load();
    if map.get(endpoint).map(String::as_str) == Some(session_id) {
        return; // already current — avoid a needless rewrite
    }
    map.insert(endpoint.to_string(), session_id.to_string());
    let path = store_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    if let Ok(content) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, &content);
        // Session ids aren't secrets, but keep this file owner-only like the rest
        // of ~/.nasiko.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}
