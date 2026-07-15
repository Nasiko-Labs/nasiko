//! MCP JSON-RPC handlers for the aggregating gateway.
//!
//! Permission enforcement is two-layer, in order: Layer 1 — can the caller reach
//! this connector at all (owner/grant/composio); Layer 2 — is the tool allowed
//! for this agent. Composio meta-tools are never filtered at list; per-toolkit
//! enforcement happens here at `tools/call` via slug→connector resolution.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::aggregator;
use crate::permissions::{self, PermissionContext, ToolAccess, toolkit_from_composio_slug};
use crate::provider::generic::DEFAULT_CALL_TIMEOUT;
use crate::router;
use crate::session::{self, ResolvedSession};
use crate::state::McpState;
use crate::types::{MCPServerConfig, PROTOCOL_VERSION, ServerType, codes};

fn ok(req_id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
}

fn err(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": code, "message": message.into() } })
}

/// Like [`err`] but carries a `data` object (e.g. the human-readable connector
/// name, so the route layer's approval flow event stays readable — id-based tool
/// prefixes are otherwise opaque).
fn err_data(req_id: &Value, code: i64, message: impl Into<String>, data: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": code, "message": message.into(), "data": data } })
}

/// Build a JSON-RPC error object (for the route layer's identity failures).
pub fn rpc_error(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    err(req_id, code, message)
}

/// Full JSON-RPC dispatch for the agent-facing gateway. Returns `None` for a
/// notification (a request with no `id`).
pub async fn handle_request(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    body: &Value,
    traceparent: Option<&str>,
) -> Option<Value> {
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");

    let Some(req_id) = body.get("id").cloned() else {
        tracing::debug!(method, "mcp notification");
        return None;
    };

    match method {
        "initialize" => return Some(handle_initialize(&req_id)),
        "ping" => return Some(json!({ "jsonrpc": "2.0", "id": req_id, "result": {} })),
        _ => {}
    }

    let perms = match permissions::load_permission_context(state, user_id, agent_id).await {
        Ok(p) => p,
        Err(e) => return Some(err(&req_id, e.json_rpc_code(), e.to_json_rpc().message)),
    };
    let resolved = match session::resolve_session(state, user_id).await {
        Ok(r) => r,
        Err(e) => return Some(err(&req_id, e.json_rpc_code(), e.to_json_rpc().message)),
    };

    let result = match method {
        "tools/list" => {
            handle_tools_list(state, &req_id, &resolved.servers, &resolved.connected_toolkits, &perms, traceparent).await
        }
        "tools/call" => {
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            handle_tools_call(state, &req_id, &params, &resolved, &perms, traceparent).await
        }
        other => err(&req_id, codes::METHOD_NOT_FOUND, format!("Method not found: {other}")),
    };
    Some(result)
}

/// `initialize` — gateway capability handshake.
pub fn handle_initialize(req_id: &Value) -> Value {
    ok(
        req_id,
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "MCP Gateway", "version": "1.0.0" },
        }),
    )
}

/// `tools/list` — aggregate, namespace, permission-filter, merge.
pub async fn handle_tools_list(
    state: &McpState,
    req_id: &Value,
    servers: &[MCPServerConfig],
    connected_toolkits: &[String],
    perms: &PermissionContext,
    traceparent: Option<&str>,
) -> Value {
    match aggregator::aggregate_tools(state, perms.user_id, servers, connected_toolkits, perms, traceparent).await {
        Ok(tools) => ok(req_id, json!({ "tools": tools })),
        Err(e) => err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
    }
}

/// `tools/call` — route, enforce two-layer permissions, forward to the backend.
pub async fn handle_tools_call(
    state: &McpState,
    req_id: &Value,
    params: &Value,
    resolved: &ResolvedSession,
    perms: &PermissionContext,
    traceparent: Option<&str>,
) -> Value {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let mut arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let (server, original) = match router::route_tool(tool_name, &resolved.servers) {
        Ok(pair) => pair,
        Err(e) => return err(req_id, codes::INVALID_PARAMS, e.to_string()),
    };

    // ── Generic MCP tool: Layer 1 (reachability) then Layer 2 (decide) ─────
    if server.kind == ServerType::Mcp {
        match state.authorizer.can_access_connector(&state.db, perms.user_id, server.connector_id).await {
            Ok(true) => {}
            Ok(false) => {
                return err(req_id, codes::TOOL_BLOCKED, format!("Connector for '{tool_name}' is not available."));
            }
            Err(e) => return err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
        }
        // Same decision `tools/list` filters on — a connector disabled for this
        // agent denies the call even though Layer 1 (owner/grant) still passes.
        match perms.decide(server.connector_id, &original) {
            ToolAccess::Denied => {
                return err(req_id, codes::TOOL_BLOCKED, format!("Tool '{tool_name}' is blocked or disabled for this agent."));
            }
            ToolAccess::Ask => {
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!("Tool '{tool_name}' requires user approval. Grant access in the agent settings."),
                    json!({ "server": server.name }),
                );
            }
            ToolAccess::Allowed => {}
        }
    }

    // ── Composio DIRECT tool call: enforce per-toolkit permission ───────────
    // A direct toolkit tool (e.g. GMAIL_SEND_EMAIL) resolves to its connector and
    // is subject to the same decide() as any other tool. Previously only the two
    // batch meta-tools below were checked, so a direct call bypassed enforcement
    // entirely (Round 3). Cross-toolkit meta-tools (COMPOSIO_SEARCH_TOOLS,
    // MANAGE_CONNECTIONS, MULTI_EXECUTE_TOOL) resolve to toolkit "composio", which
    // maps to no connector — they skip this block and are handled below / passed
    // through, exactly as before.
    if server.kind == ServerType::Composio
        && let Some(&cid) = resolved.toolkit_to_connector.get(&toolkit_from_composio_slug(tool_name))
    {
        match perms.decide(cid, tool_name) {
            ToolAccess::Denied => {
                return err(req_id, codes::TOOL_BLOCKED, format!("Tool '{tool_name}' is blocked or disabled for this agent."));
            }
            ToolAccess::Ask => {
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!("Tool '{tool_name}' requires user approval. Grant access in the agent settings."),
                    json!({ "server": "composio" }),
                );
            }
            ToolAccess::Allowed => {}
        }
    }

    // ── Composio meta-tool interception (per-toolkit → connector) ───────────
    if tool_name == "COMPOSIO_MANAGE_CONNECTIONS"
        && let Some(requested) = arguments.get("toolkits").and_then(|v| v.as_array())
    {
        let requested: Vec<String> = requested.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let blocked: Vec<String> = requested
            .iter()
            .filter(|tk| connector_disabled(resolved, perms, tk))
            .cloned()
            .collect();
        if !blocked.is_empty() {
            let allowed: Vec<String> = requested.iter().filter(|tk| !blocked.contains(tk)).cloned().collect();
            if allowed.is_empty() {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Toolkit(s) are disabled for this agent: {blocked:?}."),
                );
            }
            arguments["toolkits"] = json!(allowed);
        }
    }

    if tool_name == "COMPOSIO_MULTI_EXECUTE_TOOL"
        && perms.has_any_restriction()
        && let Some(tools_arg) = arguments.get("tools").and_then(|v| v.as_array()).cloned()
        && !tools_arg.is_empty()
    {
        let mut allowed: Vec<Value> = Vec::new();
        let mut blocked_slugs: Vec<String> = Vec::new();
        let mut ask_slugs: Vec<String> = Vec::new();

        for t in &tools_arg {
            let slug = t.get("tool_slug").and_then(|v| v.as_str()).unwrap_or("");
            if slug.is_empty() {
                allowed.push(t.clone());
                continue;
            }
            let toolkit = toolkit_from_composio_slug(slug);
            match resolved.toolkit_to_connector.get(&toolkit) {
                Some(&cid) => match perms.decide(cid, slug) {
                    ToolAccess::Denied => blocked_slugs.push(slug.to_string()),
                    ToolAccess::Ask => ask_slugs.push(slug.to_string()),
                    ToolAccess::Allowed => allowed.push(t.clone()),
                },
                None => allowed.push(t.clone()),
            }
        }

        if allowed.is_empty() && (!blocked_slugs.is_empty() || !ask_slugs.is_empty()) {
            if !ask_slugs.is_empty() {
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!("Tool(s) require user approval for this agent: {ask_slugs:?}."),
                    json!({ "server": "composio" }),
                );
            }
            return err(
                req_id,
                codes::TOOL_BLOCKED,
                format!("All requested tools are blocked for this agent: {blocked_slugs:?}."),
            );
        }
        if !blocked_slugs.is_empty() || !ask_slugs.is_empty() {
            tracing::info!(user = %perms.user_id, agent = %perms.agent_id, ?blocked_slugs, ?ask_slugs, forwarding = allowed.len(), "partial composio multi-execute filter");
            arguments["tools"] = json!(allowed);
        }
    }

    tracing::info!(tool = %tool_name, forwarded_as = %original, kind = ?server.kind, "routing tool call");

    match state
        .providers
        .mcp
        .call_tool(server, req_id, &original, &arguments, DEFAULT_CALL_TIMEOUT, traceparent)
        .await
    {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!(server = %server.name, tool = %original, error = %e, "backend tool call failed");
            err(req_id, codes::INTERNAL_ERROR, format!("Backend '{}' failed to execute '{}'", server.name, original))
        }
    }
}

/// True when a Composio toolkit maps to a connector that is disabled for the agent.
fn connector_disabled(resolved: &ResolvedSession, perms: &PermissionContext, toolkit: &str) -> bool {
    resolved
        .toolkit_to_connector
        .get(&toolkit.to_ascii_lowercase())
        .map(|cid| !perms.is_connector_enabled(*cid))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::McpConfig;
    use crate::permissions::PermissionRule;
    use crate::provider::{GenericMcpProvider, Providers};
    use crate::types::Stance;

    fn test_state() -> McpState {
        let db = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("lazy pool construction must not touch the network");
        let redis = redis::Client::open("redis://127.0.0.1:1/").expect("lazy redis client");
        McpState {
            db,
            redis,
            http_client: reqwest::Client::new(),
            guarded_http_client: reqwest::Client::new(),
            config: McpConfig {
                composio_api_key: None,
                composio_base_url: "http://localhost".to_string(),
                composio_webhook_secret: None,
                gateway_public_url: None,
                session_ttl_seconds: 60,
                perm_cache_ttl_seconds: 60,
                manifest_ttl_seconds: 60,
                oauth_state_signing_key: "test".to_string(),
            },
            providers: Providers { composio: None, mcp: GenericMcpProvider::new(reqwest::Client::new()) },
            authorizer: std::sync::Arc::new(crate::authorizer::OssConnectorAuthorizer),
        }
    }

    /// A resolved session with one Composio backend at `url` and a single
    /// connected `gmail` toolkit mapped to `cid`.
    fn gmail_session(url: &str, cid: Uuid) -> ResolvedSession {
        ResolvedSession {
            servers: vec![MCPServerConfig {
                connector_id: Uuid::nil(),
                kind: ServerType::Composio,
                name: "composio".into(),
                url: url.into(),
                headers: HashMap::new(),
                transport: "streamable_http".into(),
            }],
            connected_toolkits: vec!["gmail".into()],
            toolkit_to_connector: HashMap::from([("gmail".to_string(), cid)]),
        }
    }

    fn perms(rules: Vec<PermissionRule>, disabled: &[Uuid]) -> PermissionContext {
        PermissionContext {
            user_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            disabled_connectors: disabled.iter().copied().collect(),
            rules,
            hash: "h".into(),
        }
    }

    fn rule(cid: Uuid, pat: &str, stance: Stance) -> PermissionRule {
        PermissionRule { connector_id: cid, tool_pattern: pat.into(), stance }
    }

    // ── Round 3: direct Composio tool calls must be permission-enforced ──────

    #[tokio::test]
    async fn composio_direct_tool_with_block_rule_is_denied_before_backend() {
        // url points nowhere reachable — a Denied decision must return before any
        // backend call, so this must not hang or error on the network.
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(vec![rule(cid, "GMAIL_SEND_*", Stance::Block)], &[]);
        let res = handle_tools_call(
            &test_state(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
    }

    #[tokio::test]
    async fn composio_direct_tool_on_disabled_connector_is_denied() {
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(vec![], &[cid]); // whole connector disabled for the agent
        let res = handle_tools_call(
            &test_state(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
    }

    #[tokio::test]
    async fn composio_direct_tool_with_ask_rule_returns_tool_ask() {
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(vec![rule(cid, "*", Stance::Ask)], &[]);
        let res = handle_tools_call(
            &test_state(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_ASK), "{res}");
    }

    #[tokio::test]
    async fn composio_allowed_direct_tool_reaches_backend() {
        let mut backend = mockito::Server::new_async().await;
        let hit = backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .expect(1)
            .create_async()
            .await;

        let cid = Uuid::new_v4();
        let resolved = gmail_session(&format!("{}/mcp", backend.url()), cid);
        let p = perms(vec![], &[]); // default allow
        let res = handle_tools_call(
            &test_state(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["result"]["ok"], json!(true), "{res}");
        hit.assert_async().await;
    }

    #[tokio::test]
    async fn composio_cross_toolkit_metatool_is_not_caught_by_per_toolkit_check() {
        // COMPOSIO_SEARCH_TOOLS resolves to toolkit "composio" (no connector), so
        // even with the gmail connector disabled it must NOT be denied by the
        // per-toolkit check — it proceeds to the backend (meta-tools are not
        // connector-scoped; MANAGE_CONNECTIONS/MULTI_EXECUTE do their own filtering).
        let mut backend = mockito::Server::new_async().await;
        let hit = backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#)
            .expect(1)
            .create_async()
            .await;

        let cid = Uuid::new_v4();
        let resolved = gmail_session(&format!("{}/mcp", backend.url()), cid);
        let p = perms(vec![], &[cid]); // gmail disabled — irrelevant to a meta-tool
        let res = handle_tools_call(
            &test_state(),
            &json!(1),
            &json!({ "name": "COMPOSIO_SEARCH_TOOLS", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert!(res.get("error").is_none(), "a cross-toolkit meta-tool must not be blocked by the per-toolkit check: {res}");
        hit.assert_async().await;
    }
}
