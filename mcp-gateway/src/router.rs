//! Tool-name → backend routing.
//!
//! Inverse of the aggregator's namespacing:
//!   * `serpapi__search` → (serpapi backend, `search`)
//!   * `COMPOSIO_*`      → (composio backend, unchanged)
//!   * bare name         → composio if present, else the first live backend

use crate::error::{McpError, Result};
use crate::types::MCPServerConfig;

/// Resolve a tool name to its backend and the original (un-namespaced) tool name
/// to forward.
///
/// A `{prefix}__{tool}` name whose prefix is not among the (already
/// permission-filtered) backends is rejected — it means the server was disabled
/// for this agent, so we must **not** silently fall back to Composio.
pub fn route_tool<'a>(
    tool_name: &str,
    servers: &'a [MCPServerConfig],
) -> Result<(&'a MCPServerConfig, String)> {
    if let Some((prefix, original)) = tool_name.split_once("__") {
        return servers
            .iter()
            .find(|s| s.name == prefix && !s.url.is_empty())
            .map(|s| (s, original.to_string()))
            .ok_or_else(|| {
                McpError::BadRequest(format!(
                    "Server '{prefix}' is not available for this agent. \
                     It may be disabled in the agent's permission settings."
                ))
            });
    }

    if let Some(composio) = servers.iter().find(|s| s.name == "composio" && !s.url.is_empty()) {
        return Ok((composio, tool_name.to_string()));
    }
    if let Some(fallback) = servers.iter().find(|s| !s.url.is_empty()) {
        return Ok((fallback, tool_name.to_string()));
    }
    Err(McpError::BadRequest(format!(
        "No backend server found for tool '{tool_name}'"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn srv(name: &str, url: &str) -> MCPServerConfig {
        MCPServerConfig { name: name.into(), url: url.into(), headers: HashMap::new(), transport: "streamable_http".into() }
    }

    #[test]
    fn namespaced_prefix_routes_to_backend() {
        let servers = vec![srv("composio", "http://c"), srv("serpapi", "http://s")];
        let (s, orig) = route_tool("serpapi__search", &servers).unwrap();
        assert_eq!(s.name, "serpapi");
        assert_eq!(orig, "search");
    }

    #[test]
    fn namespaced_prefix_with_extra_underscores() {
        let servers = vec![srv("serpapi", "http://s")];
        // split_once on "__" keeps the rest intact.
        let (_s, orig) = route_tool("serpapi__deep__search", &servers).unwrap();
        assert_eq!(orig, "deep__search");
    }

    #[test]
    fn missing_prefix_is_rejected_not_fallback() {
        // Server disabled/absent → error, never silent composio fallback.
        let servers = vec![srv("composio", "http://c")];
        assert!(route_tool("serpapi__search", &servers).is_err());
    }

    #[test]
    fn bare_name_routes_to_composio() {
        let servers = vec![srv("serpapi", "http://s"), srv("composio", "http://c")];
        let (s, orig) = route_tool("COMPOSIO_SEARCH_TOOLS", &servers).unwrap();
        assert_eq!(s.name, "composio");
        assert_eq!(orig, "COMPOSIO_SEARCH_TOOLS");
    }

    #[test]
    fn bare_name_falls_back_to_first_live_when_no_composio() {
        let servers = vec![srv("serpapi", "http://s")];
        let (s, _) = route_tool("something", &servers).unwrap();
        assert_eq!(s.name, "serpapi");
    }

    #[test]
    fn empty_or_urlless_servers_error() {
        assert!(route_tool("x", &[]).is_err());
        let servers = vec![srv("composio", "")]; // no url → not live
        assert!(route_tool("x", &servers).is_err());
    }
}
