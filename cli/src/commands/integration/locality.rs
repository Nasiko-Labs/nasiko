//! Whether a control plane keeps a coding agent's chat history (prompts, responses, tool output),
//! and the words every command uses to say so. The server decides
//! (`GET /api/me/coding-agents/privacy`): a hosted Nasiko never keeps it, an open-source one on a
//! developer's machine does, and an enterprise install does when its admin turns it on. Where the
//! server keeps it, the user is still asked first; everything else — usage — is always synced.

use serde::Deserialize;

/// What is synced either way, named in full wherever reporting is described.
pub const USAGE: &str = "tokens, cost, latency, models, tools and subagents";

/// Shown before asking whether to sync chat history to `cluster`.
pub fn chat_history_warning(cluster: &str) -> String {
    format!(
        "Warning: syncing chat history sends your coding sessions' prompts, responses and tool \
         output to {cluster}."
    )
}

/// The question itself; the default answer is no.
pub fn chat_history_question(cluster: &str) -> String {
    format!(
        "Also send chat history (the text of prompts, responses and tool output) to {cluster}? \
         (No sends usage only: {USAGE})"
    )
}

/// Said instead, for a control plane that doesn't keep chat history.
pub fn usage_only_notice(cluster: &str) -> String {
    format!(
        "{cluster} syncs usage only — {USAGE}. Chat history (prompts, responses, tool output) \
         never leaves this machine."
    )
}

/// Said when `--content` asks for chat history from a control plane that doesn't keep it.
pub fn content_refused(cluster: &str) -> String {
    format!(
        "--content ignored: {cluster} doesn't keep chat history, so it gets usage only ({USAGE})."
    )
}

/// Said instead of asking, for new or moved reporting, when the control plane couldn't be asked
/// whether it keeps chat history.
pub fn privacy_unknown(cluster: &str) -> String {
    format!(
        "Couldn't ask {cluster} whether it keeps chat history, so it gets usage only for now \
         ({USAGE})."
    )
}

/// Said when `--content` asks for chat history from a control plane that couldn't be asked.
pub fn content_unconfirmed(cluster: &str) -> String {
    format!(
        "--content ignored: couldn't ask {cluster} whether it keeps chat history, so the saved \
         setting is kept."
    )
}

/// What an agent's reporting syncs, for plans and status lines.
pub fn synced(chat_history: bool) -> String {
    if chat_history {
        "usage and chat history".to_string()
    } else {
        "usage only".to_string()
    }
}

#[derive(Deserialize)]
struct Privacy {
    chat_history: bool,
}

/// Whether the active cluster (at `cluster_url`) keeps chat history, by its own answer. A server
/// too old to say (a 404) is taken to keep it only when it runs on this machine. `None` when it
/// can't be asked at all (unreachable, an expired login, an unreadable answer): unknown, not no,
/// so a saved setting is kept rather than rewritten by an outage.
pub fn server_accepts_chat_history(cluster_url: &str) -> Option<bool> {
    let answer = crate::api::Client::from_active_cluster()
        .and_then(|client| client.get_json_optional::<Privacy>("/me/coding-agents/privacy"));
    match answer {
        Ok(Some(privacy)) => Some(privacy.chat_history),
        Ok(None) => Some(is_local_cluster(cluster_url)),
        Err(_) => None,
    }
}

/// Whether `cluster_url` names this machine: `localhost` (or a `*.localhost` name), an IPv4
/// loopback address (`127.0.0.0/8`) or `::1`, on any port and with any scheme. Only the fallback
/// for a server too old to state its own policy.
pub fn is_local_cluster(cluster_url: &str) -> bool {
    let host = host_of(cluster_url).to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The host of `url`, without scheme, credentials, port, path or IPv6 brackets.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    if let Some(bracketed) = host_port.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or_default();
    }
    host_port.split(':').next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_on_any_port_is_local() {
        for url in [
            "http://localhost",
            "http://localhost:8080",
            "https://LOCALHOST:443/api",
            "http://127.0.0.1:3000",
            "http://127.1.2.3",
            "http://[::1]:8080/",
            "http://user:pass@localhost:9000",
            "http://nasiko.localhost:8080",
            "localhost:8080",
        ] {
            assert!(is_local_cluster(url), "{url} is this machine");
        }
    }

    #[test]
    fn anything_else_is_not_local() {
        for url in [
            "https://nasiko.example.com",
            "https://app.nasiko.com:443",
            "http://10.0.0.5:8080",
            "http://192.168.1.10",
            "http://[2001:db8::1]:8080",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "",
        ] {
            assert!(!is_local_cluster(url), "{url} is not this machine");
        }
    }

    #[test]
    fn every_line_names_everything_usage_covers() {
        for line in [
            chat_history_question("prod"),
            usage_only_notice("prod"),
            content_refused("prod"),
        ] {
            assert!(line.contains(USAGE), "{line}");
        }
        assert!(usage_only_notice("prod").starts_with("prod "));
    }
}
