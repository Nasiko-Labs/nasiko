//! MCP JSON-RPC handlers for the aggregating gateway.
//!
//! Permission enforcement is two-layer, in order: Layer 1 — can the caller reach
//! this connector at all (owner/grant/composio); Layer 2 — is the tool allowed
//! for this agent. Composio meta-tools are never filtered at list; per-toolkit
//! enforcement happens here at `tools/call` via slug→connector resolution.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::aggregator;
use crate::permissions::{self, PermissionContext, toolkit_from_composio_slug};
use crate::provider::generic::DEFAULT_CALL_TIMEOUT;
use crate::repo;
use crate::router;
use crate::session::{self, ResolvedSession};
use crate::state::McpState;
use crate::types::{MCPServerConfig, PROTOCOL_VERSION, ServerType, Stance, codes};

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

    // ── Generic MCP tool: Layer 1 then Layer 2 ─────────────────────────────
    if server.kind == ServerType::Mcp {
        match repo::can_access_connector(&state.db, perms.user_id, server.connector_id).await {
            Ok(true) => {}
            Ok(false) => {
                return err(req_id, codes::TOOL_BLOCKED, format!("Connector for '{tool_name}' is not available."));
            }
            Err(e) => return err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
        }
        match perms.get_stance(server.connector_id, &original) {
            Stance::Block => {
                return err(req_id, codes::TOOL_BLOCKED, format!("Tool '{tool_name}' is blocked for this agent."));
            }
            Stance::Ask => {
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!("Tool '{tool_name}' requires user approval. Grant access in the agent settings."),
                    json!({ "server": server.name }),
                );
            }
            Stance::Allow => {}
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
                Some(&cid) if !perms.is_connector_enabled(cid) => blocked_slugs.push(slug.to_string()),
                Some(&cid) => match perms.get_stance(cid, slug) {
                    Stance::Block => blocked_slugs.push(slug.to_string()),
                    Stance::Ask => ask_slugs.push(slug.to_string()),
                    Stance::Allow => allowed.push(t.clone()),
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
