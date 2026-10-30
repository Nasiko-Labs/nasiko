//! Tool-name → backend routing — inverse of the aggregator's id namespacing.
//!   * `{connector_prefix}__{tool}` → that connector's generic backend, `tool`
//!   * bare name owned by a system backend's synced tool catalog → that
//!     system backend, name unchanged (exact match only — never a heuristic)
//!   * bare name otherwise (a Composio meta-tool or toolkit slug, e.g.
//!     `COMPOSIO_SEARCH_TOOLS`/`GMAIL_SEND_EMAIL`) → the composio backend

use std::collections::HashMap;

use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::permissions::toolkit_from_composio_slug;
use crate::types::{MCPServerConfig, ServerType, UnusableConnector, connector_prefix};

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
                s.kind == ServerType::Mcp
                    && !s.url.is_empty()
                    && connector_prefix(s.connector_id) == prefix
            })
            .map(|s| (s, original.to_string()))
            .ok_or_else(|| {
                McpError::BadRequest(format!(
                    "Connector '{prefix}' is not available for this agent. \
                     It may be disabled in the agent's permission settings."
                ))
            });
    }

    // A bare name is owned by a system backend only when its synced tool
    // catalog says so, exactly — never by "it's the only bare-name backend
    // around" or any other shape-based guess. Composio's own bare names
    // (`COMPOSIO_SEARCH_TOOLS`, toolkit slugs like `GMAIL_SEND_EMAIL`, see
    // aggregator.rs's meta-tool filter) are otherwise indistinguishable from
    // a system tool by name alone, so an exact catalog match is the only
    // deterministic way to tell them apart.
    if let Some(system) = servers
        .iter()
        .find(|s| s.system && !s.url.is_empty() && s.tool_names.iter().any(|t| t == tool_name))
    {
        return Ok((system, tool_name.to_string()));
    }

    // Anything a system backend doesn't claim is otherwise only valid as a
    // Composio meta-tool. Never guess a generic backend — the aggregator
    // always namespaces generic tools (except system ones, matched above), so
    // an un-prefixed name that isn't Composio's and isn't system-owned is
    // malformed/hallucinated.
    if let Some(composio) = servers
        .iter()
        .find(|s| s.kind == ServerType::Composio && !s.url.is_empty())
    {
        return Ok((composio, tool_name.to_string()));
    }
    Err(McpError::BadRequest(format!(
        "Unknown tool '{tool_name}' — no matching connector."
    )))
}

/// The name a catalog tool is callable under in this session — the forward of
/// [`route_tool`], matching the aggregator's namespacing: a non-system generic
/// backend's tool is `{connector_prefix}__{tool}`, everything else (system
/// backends, Composio toolkit tools) stays bare.
///
/// `None` when this session can't call it at all: the connector isn't enabled
/// for the agent, has no usable backend right now, or the name would route to
/// a different backend than the one that owns it (a system catalog that hasn't
/// synced the tool, a toolkit with no active connection). Search results must
/// go through this, so they never name a tool `tools/call` can't route back.
/// `toolkit_connectors` is [`crate::session::ResolvedSession::toolkit_to_connector`]:
/// a Composio toolkit tool's catalog id is its toolkit's connector, which no
/// resolved server carries (the Composio backend is `Uuid::nil()`).
pub fn callable_tool_name(
    connector_id: Uuid,
    tool_name: &str,
    servers: &[MCPServerConfig],
    toolkit_connectors: &HashMap<String, Uuid>,
) -> Option<String> {
    let generic = servers
        .iter()
        .any(|s| s.connector_id == connector_id && s.kind == ServerType::Mcp && !s.system);
    let name = if generic {
        format!("{}__{tool_name}", connector_prefix(connector_id))
    } else {
        tool_name.to_string()
    };
    let (server, original) = route_tool(&name, servers).ok()?;
    let owned = match server.kind {
        ServerType::Mcp => server.connector_id == connector_id,
        ServerType::Composio => {
            toolkit_connectors.get(&toolkit_from_composio_slug(tool_name)) == Some(&connector_id)
        }
    };
    (owned && original == tool_name).then_some(name)
}

/// When [`route_tool`] fails for a `{prefix}__tool` name, check whether the
/// prefix actually matches a connector the caller can reach but that's
/// unusable right now (M1's `ConnectorUnusable`) — as opposed to a prefix
/// that matches no known connector at all (hallucinated/stale tool name).
/// Lets `tools/call` return a distinct "needs re-authentication" signal
/// instead of the generic "not available" error for exactly that case.
pub fn unusable_reason_for_prefix<'a>(
    tool_name: &str,
    unusable: &'a HashMap<Uuid, UnusableConnector>,
) -> Option<(Uuid, &'a UnusableConnector)> {
    let (prefix, _) = tool_name.split_once("__")?;
    unusable
        .iter()
        .find(|(id, _)| connector_prefix(**id) == prefix)
        .map(|(id, info)| (*id, info))
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
            system: false,
            tool_names: vec![],
            instructions: None,
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
    fn callable_name_of_a_generic_tool_routes_back_to_it() {
        let id = Uuid::new_v4();
        let servers = vec![
            srv(ServerType::Composio, Uuid::nil(), "http://c"),
            srv(ServerType::Mcp, id, "http://s"),
        ];
        let name = callable_tool_name(id, "get_order_details", &servers, &HashMap::new())
            .expect("callable");
        assert_eq!(name, format!("{}__get_order_details", connector_prefix(id)));
        let (s, orig) = route_tool(&name, &servers).unwrap();
        assert_eq!(s.connector_id, id);
        assert_eq!(orig, "get_order_details");
    }

    #[test]
    fn callable_name_stays_bare_for_system_and_composio_tools() {
        let sys = system_srv(&["save_file"]);
        let gmail = Uuid::new_v4();
        let toolkits = HashMap::from([("gmail".to_string(), gmail)]);
        let servers = vec![
            srv(ServerType::Composio, Uuid::nil(), "http://c"),
            sys.clone(),
        ];
        assert_eq!(
            callable_tool_name(sys.connector_id, "save_file", &servers, &toolkits).as_deref(),
            Some("save_file")
        );
        assert_eq!(
            callable_tool_name(gmail, "GMAIL_SEND_EMAIL", &servers, &toolkits).as_deref(),
            Some("GMAIL_SEND_EMAIL")
        );
    }

    #[test]
    fn a_generic_connector_outside_the_session_is_not_callable() {
        // Search filters by the user's connectors, not the agent's: one not
        // enabled for the agent (or with no usable backend) must not be named,
        // bare or prefixed — bare would fall through to Composio.
        let servers = vec![
            srv(ServerType::Composio, Uuid::nil(), "http://c"),
            srv(ServerType::Mcp, Uuid::new_v4(), "http://s"),
        ];
        assert_eq!(
            callable_tool_name(
                Uuid::new_v4(),
                "get_order_details",
                &servers,
                &HashMap::new()
            ),
            None
        );
        let id = Uuid::new_v4();
        let emptied = vec![srv(ServerType::Mcp, id, "")];
        assert_eq!(
            callable_tool_name(id, "get_order_details", &emptied, &HashMap::new()),
            None
        );
    }

    #[test]
    fn a_tool_that_would_route_elsewhere_is_not_callable() {
        let sys = system_srv(&["save_file"]);
        let gmail = Uuid::new_v4();
        let toolkits = HashMap::from([("gmail".to_string(), gmail)]);
        let servers = vec![
            srv(ServerType::Composio, Uuid::nil(), "http://c"),
            sys.clone(),
        ];
        // Not in the system backend's synced catalog: bare, it goes to Composio.
        assert_eq!(
            callable_tool_name(sys.connector_id, "delete_file", &servers, &toolkits),
            None
        );
        // A toolkit with no active connection, or a slug of another toolkit.
        assert_eq!(
            callable_tool_name(Uuid::new_v4(), "SLACK_SEND_MESSAGE", &servers, &toolkits),
            None
        );
        assert_eq!(
            callable_tool_name(Uuid::new_v4(), "GMAIL_SEND_EMAIL", &servers, &toolkits),
            None
        );
        // A toolkit tool with no Composio backend in the session.
        assert_eq!(
            callable_tool_name(gmail, "GMAIL_SEND_EMAIL", &[sys], &toolkits),
            None
        );
    }

    #[test]
    fn missing_prefix_is_rejected_not_fallback() {
        let servers = vec![srv(ServerType::Composio, Uuid::nil(), "http://c")];
        assert!(route_tool("abcd1234__search", &servers).is_err());
    }

    /// A system server that owns `tool_names: ["save_file"]` — the same
    /// shape a live one would carry once `credentials::build_server_config`
    /// loads its synced tool catalog.
    fn system_srv(tool_names: &[&str]) -> MCPServerConfig {
        MCPServerConfig {
            connector_id: Uuid::new_v4(),
            kind: ServerType::Mcp,
            name: "workspace".into(),
            url: "http://127.0.0.1:1/x".into(),
            headers: Default::default(),
            transport: "streamable_http".into(),
            trusted: true,
            system: true,
            tool_names: tool_names.iter().map(|s| s.to_string()).collect(),
            instructions: None,
        }
    }

    #[test]
    fn bare_name_owned_by_a_system_server_routes_to_it_even_with_composio_present() {
        let sys = system_srv(&["save_file"]);
        let composio = srv(ServerType::Composio, Uuid::nil(), "http://c");
        let servers = vec![composio, sys.clone()];
        let (s, name) = route_tool("save_file", &servers).unwrap();
        assert_eq!(s.connector_id, sys.connector_id);
        assert_eq!(name, "save_file");
    }

    #[test]
    fn bare_composio_name_still_routes_to_composio_when_a_system_server_is_present() {
        // The system server is present and owns `save_file`, but NOT
        // `COMPOSIO_SEARCH_TOOLS` — a system server existing at all must not
        // swallow every bare name, only the ones its catalog actually lists.
        let sys = system_srv(&["save_file"]);
        let composio = srv(ServerType::Composio, Uuid::nil(), "http://c");
        let servers = vec![sys, composio];
        let (s, orig) = route_tool("COMPOSIO_SEARCH_TOOLS", &servers).unwrap();
        assert_eq!(s.kind, ServerType::Composio);
        assert_eq!(orig, "COMPOSIO_SEARCH_TOOLS");
    }

    #[test]
    fn bare_name_owned_by_nobody_falls_back_to_composio_when_present() {
        // A system server is present but its catalog doesn't list this name —
        // with a Composio backend also present, current (pre-existing)
        // behavior is to hand it to Composio, same as if no system server
        // existed at all.
        let sys = system_srv(&["save_file"]);
        let composio = srv(ServerType::Composio, Uuid::nil(), "http://c");
        let servers = vec![sys, composio];
        let (s, orig) = route_tool("totally_unowned_tool", &servers).unwrap();
        assert_eq!(s.kind, ServerType::Composio);
        assert_eq!(orig, "totally_unowned_tool");
    }

    #[test]
    fn system_server_with_empty_url_is_skipped_for_bare_names() {
        // Same shape as `system_srv`, but `url` empty — the connector's
        // container/backend isn't currently resolvable (mirrors
        // `prefix_matches_connector_but_url_is_empty_is_treated_as_unavailable`
        // for the namespaced-prefix path). A bare name it would otherwise own
        // must fall through exactly as if the system server weren't present
        // at all, never be routed to a backend with nowhere to send the call.
        let mut sys = system_srv(&["save_file"]);
        sys.url = String::new();
        let composio = srv(ServerType::Composio, Uuid::nil(), "http://c");
        let servers = vec![sys, composio];
        let (s, orig) = route_tool("save_file", &servers).unwrap();
        assert_eq!(s.kind, ServerType::Composio);
        assert_eq!(orig, "save_file");
    }

    #[test]
    fn prefixed_name_addressed_to_a_system_connector_still_routes_to_it() {
        // Pins CURRENT behavior, not a requirement: nothing in `route_tool`'s
        // namespaced-prefix branch excludes `system` servers — it matches on
        // `kind == ServerType::Mcp` and the connector prefix alone, same as
        // any other generic backend. So `{prefix}__save_file` addressed to a
        // system connector's own prefix reaches it, exactly like a bare
        // `save_file` would via the system-catalog branch above. This is
        // intentional: an agent that happens to namespace a system tool's
        // name still reaches the right backend, it's just redundant.
        let sys = system_srv(&["save_file"]);
        let name = format!("{}__save_file", connector_prefix(sys.connector_id));
        let servers = vec![sys.clone()];
        let (s, orig) = route_tool(&name, &servers).unwrap();
        assert_eq!(s.connector_id, sys.connector_id);
        assert_eq!(orig, "save_file");
    }

    #[test]
    fn bare_name_owned_by_nobody_errors_without_composio() {
        // Same as above, but with no Composio backend at all — the existing
        // "Unknown tool" error path, unchanged by the system server's presence.
        let sys = system_srv(&["save_file"]);
        let servers = vec![sys];
        assert!(route_tool("totally_unowned_tool", &servers).is_err());
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

    // ─── unusable_reason_for_prefix ─────────────────────────────────────────

    fn unusable_of(
        id: Uuid,
        reason: crate::types::ConnectorUnusable,
    ) -> HashMap<Uuid, crate::types::UnusableConnector> {
        HashMap::from([(
            id,
            crate::types::UnusableConnector {
                reason,
                name: "test-connector".into(),
            },
        )])
    }

    #[test]
    fn unusable_reason_found_for_matching_prefix() {
        let id = Uuid::new_v4();
        let unusable = unusable_of(id, crate::types::ConnectorUnusable::AuthRequired);
        let name = format!("{}__search", connector_prefix(id));
        let (found_id, info) = unusable_reason_for_prefix(&name, &unusable).unwrap();
        assert_eq!(found_id, id);
        assert_eq!(info.reason, crate::types::ConnectorUnusable::AuthRequired);
        assert_eq!(info.name, "test-connector");
    }

    #[test]
    fn unusable_reason_is_none_for_unrelated_prefix() {
        let id = Uuid::new_v4();
        let unusable = unusable_of(id, crate::types::ConnectorUnusable::AuthRequired);
        assert!(unusable_reason_for_prefix("deadbeef00000000__search", &unusable).is_none());
    }

    #[test]
    fn unusable_reason_is_none_for_bare_name() {
        let id = Uuid::new_v4();
        let unusable = unusable_of(id, crate::types::ConnectorUnusable::AuthRequired);
        assert!(unusable_reason_for_prefix("bare_tool_name", &unusable).is_none());
    }

    #[test]
    fn unusable_reason_is_none_when_map_is_empty() {
        let id = Uuid::new_v4();
        let name = format!("{}__search", connector_prefix(id));
        assert!(unusable_reason_for_prefix(&name, &HashMap::new()).is_none());
    }
}
