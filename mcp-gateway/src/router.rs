//! Tool-name → backend routing — inverse of the aggregator's id namespacing.
//!   * `{connector_prefix}__{tool}` → (that connector's backend, `tool`)
//!   * `COMPOSIO_*` / bare name     → the composio backend, unchanged
//!   * bare name, no composio       → first live backend

use crate::error::{McpError, Result};
use crate::types::{MCPServerConfig, ServerType, connector_prefix};

/// Resolve a tool name to its backend and the original (un-namespaced) tool name.
///
/// A `{prefix}__{tool}` name whose prefix matches no live generic backend is
/// rejected — it means the connector was disabled/hidden for this agent, so we
/// must NOT silently fall back to Composio.
pub fn route_tool<'a>(
    tool_name: &str,
    servers: &'a [MCPServerConfig],
) -> Result<(&'a MCPServerConfig, String)> {
    if let Some((prefix, original)) = tool_name.split_once("__") {
        return servers
            .iter()
            .find(|s| {
                s.kind == ServerType::Mcp && !s.url.is_empty() && connector_prefix(s.connector_id) == prefix
            })
            .map(|s| (s, original.to_string()))
            .ok_or_else(|| {
                McpError::BadRequest(format!(
                    "Connector '{prefix}' is not available for this agent. \
                     It may be disabled in the agent's permission settings."
                ))
            });
    }

    // A bare (un-prefixed) name is only valid as a Composio meta-tool. Never
    // guess a generic backend — the aggregator always namespaces generic tools,
    // so an un-prefixed name that isn't Composio is malformed/hallucinated.
    if let Some(composio) = servers.iter().find(|s| s.kind == ServerType::Composio && !s.url.is_empty()) {
        return Ok((composio, tool_name.to_string()));
    }
    Err(McpError::BadRequest(format!("Unknown tool '{tool_name}' — no matching connector.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn srv(kind: ServerType, id: Uuid, url: &str) -> MCPServerConfig {
        MCPServerConfig {
            connector_id: id,
            kind,
            name: "n".into(),
            url: url.into(),
            headers: HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
        }
    }

    #[test]
    fn namespaced_prefix_routes_to_backend() {
        let id = Uuid::new_v4();
        let servers = vec![
            srv(ServerType::Composio, Uuid::nil(), "http://c"),
            srv(ServerType::Mcp, id, "http://s"),
        ];
        let name = format!("{}__search", connector_prefix(id));
        let (s, orig) = route_tool(&name, &servers).unwrap();
        assert_eq!(s.connector_id, id);
        assert_eq!(orig, "search");
    }

    #[test]
    fn namespaced_prefix_with_extra_underscores() {
        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, "http://s")];
        let name = format!("{}__deep__search", connector_prefix(id));
        let (_s, orig) = route_tool(&name, &servers).unwrap();
        assert_eq!(orig, "deep__search");
    }

    #[test]
    fn missing_prefix_is_rejected_not_fallback() {
        let servers = vec![srv(ServerType::Composio, Uuid::nil(), "http://c")];
        assert!(route_tool("abcd1234__search", &servers).is_err());
    }

    #[test]
    fn bare_name_routes_to_composio() {
        let servers = vec![
            srv(ServerType::Mcp, Uuid::new_v4(), "http://s"),
            srv(ServerType::Composio, Uuid::nil(), "http://c"),
        ];
        let (s, orig) = route_tool("COMPOSIO_SEARCH_TOOLS", &servers).unwrap();
        assert_eq!(s.kind, ServerType::Composio);
        assert_eq!(orig, "COMPOSIO_SEARCH_TOOLS");
    }

    #[test]
    fn bare_name_without_composio_is_error_not_first_live_guess() {
        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, "http://s")];
        // No composio backend + un-prefixed name → error, never a silent guess.
        assert!(route_tool("something", &servers).is_err());
    }

    #[test]
    fn empty_or_urlless_servers_error() {
        assert!(route_tool("x", &[]).is_err());
        let servers = vec![srv(ServerType::Composio, Uuid::nil(), "")];
        assert!(route_tool("x", &servers).is_err());
    }

    #[test]
    fn bare_name_never_falls_back_to_a_generic_backend_regardless_of_order() {
        // Regression guard for fix #7: with no Composio backend, a bare (un-
        // prefixed) name must error and NEVER be silently routed to whichever
        // generic server happens to be first. Both input orders must error —
        // there is no order-dependent first-live guess anymore.
        let s_hi = srv(ServerType::Mcp, Uuid::from_u128(2), "http://a");
        let s_lo = srv(ServerType::Mcp, Uuid::from_u128(1), "http://b");
        assert!(route_tool("bare_tool", &[s_hi.clone(), s_lo.clone()]).is_err());
        assert!(route_tool("bare_tool", &[s_lo, s_hi]).is_err());
    }

    #[test]
    fn unmatched_prefix_errors_even_with_live_generic_servers_present() {
        // A prefix matching no known connector must be rejected outright — never
        // fall through to another live generic backend (that would misroute).
        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, "http://s")];
        assert!(route_tool("deadbeef__search", &servers).is_err());
    }

    #[test]
    fn empty_prefix_string_is_rejected_not_treated_as_bare_name() {
        // `"__tool".split_once("__")` yields `("", "tool")` — an empty prefix
        // matches no real connector prefix, so it must error.
        let servers = vec![srv(ServerType::Composio, Uuid::nil(), "http://c")];
        assert!(route_tool("__tool", &servers).is_err());
        assert!(route_tool("__", &servers).is_err());
    }

    #[test]
    fn prefix_matches_connector_but_url_is_empty_is_treated_as_unavailable() {
        // A generic server whose prefix matches but whose `url` is empty
        // (disabled/unresolvable) must not be selected.
        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, "")];
        let name = format!("{}__search", connector_prefix(id));
        assert!(route_tool(&name, &servers).is_err());
    }
}
