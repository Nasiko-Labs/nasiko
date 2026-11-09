//! Whether a control plane runs on this machine. A coding agent's chat history (prompts,
//! responses, tool output) is only ever offered to a control plane on localhost: anything else is
//! a hosted server, which gets what prices a turn — tokens, model, cost, timings — and nothing
//! more.

/// Shown before asking whether to sync chat history to a control plane on this machine.
pub const CHAT_HISTORY_WARNING: &str = "Warning: syncing chat history sends your coding sessions' \
     prompts, responses and tool output to the Nasiko running on this machine.";
/// The question itself; the default answer is no.
pub const CHAT_HISTORY_QUESTION: &str =
    "Sync chat history too? (No sends only tokens, latency and cost)";
/// Said instead, for a control plane anywhere else.
pub const HOSTED_NOTICE: &str = "Only tokens, latency and cost are synced to this cluster — \
     prompt and response text never leaves this machine.";
/// Said when `--content` asks for chat history on a hosted control plane.
pub const CONTENT_REFUSED: &str = "--content ignored: chat history is only synced to a Nasiko \
     running on this machine (localhost); this cluster gets tokens, latency and cost only.";

/// Whether `cluster_url` names this machine: `localhost` (or a `*.localhost` name), an IPv4
/// loopback address (`127.0.0.0/8`) or `::1`, on any port and with any scheme.
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
    fn anything_else_is_hosted() {
        for url in [
            "https://internal.nasiko.dev",
            "https://nasiko.dev:443",
            "http://10.0.0.5:8080",
            "http://192.168.1.10",
            "http://[2001:db8::1]:8080",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "",
        ] {
            assert!(!is_local_cluster(url), "{url} is hosted");
        }
    }
}
