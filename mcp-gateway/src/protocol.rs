//! MCP JSON-RPC handlers for the aggregating gateway.
//!
//! Each handler takes a parsed request and returns a JSON-serializable
//! [`serde_json::Value`] the route layer (`oss/server/src/mcp/`) sends back to
//! the agent. Port of the PoC's `mcp_protocol.py`.
//!
//! Permission enforcement (Claude-style deny → ask → allow):
//!   * Generic MCP tools: filtered at `tools/list` and re-checked at `tools/call`.
//!   * Composio meta-tools: never filtered at list. `COMPOSIO_MULTI_EXECUTE_TOOL`
//!     args are inspected — blocked/ask tool slugs stripped or rejected;
//!     `COMPOSIO_MANAGE_CONNECTIONS` toolkit list is filtered against the agent's
//!     disabled servers so a disabled toolkit can't be (re)connected.

use uuid::Uuid;
use serde_json::{Value, json};

use crate::aggregator;
use crate::permissions::{self, PermissionContext, toolkit_from_composio_slug};
use crate::provider::generic::DEFAULT_CALL_TIMEOUT;
use crate::router;
use crate::session;
use crate::state::McpState;
use crate::types::{MCPServerConfig, PROTOCOL_VERSION, Stance, codes};

// ─── JSON-RPC helpers ───────────────────────────────────────────────────────

fn ok(req_id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
}

fn err(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": code, "message": message.into() } })
}

/// Build a JSON-RPC error object. Exposed so the server route layer can
/// report identity failures (invalid/missing delegation token) that happen
/// before it has enough to call [`handle_request`] — everything else that can
/// go wrong is handled inside this module.
pub fn rpc_error(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    err(req_id, code, message)
}

// ─── Top-level dispatch ─────────────────────────────────────────────────────

/// Full JSON-RPC dispatch for the agent-facing gateway: notification
/// short-circuit, `initialize`/`ping`, and permission/session-aware
/// `tools/list` + `tools/call` routing. This is the single entry point the
/// server route (`oss/server/src/mcp/gateway.rs`) calls after resolving
/// identity — everything below here is pure protocol logic with no
/// dependency on `AppState`, so it's reusable as-is by any route (including a
/// future EE-specific one) that can supply a validated `(user_id, agent_id)`.
///
/// Returns `None` for a notification (a request with no `id`) — the caller
/// should respond `202 Accepted` with an empty body in that case, per the MCP
/// JSON-RPC convention.
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

    // Methods that need no session/permissions.
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
            handle_tools_list(
                state,
                &req_id,
                &resolved.servers,
                &resolved.connected_toolkits,
                &perms,
                traceparent,
            )
            .await
        }
        "tools/call" => {
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            handle_tools_call(state, &req_id, &params, &resolved.servers, &perms, traceparent).await
        }
        other => err(&req_id, codes::METHOD_NOT_FOUND, format!("Method not found: {other}")),
    };
    Some(result)
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `initialize` — gateway capability handshake. No session/permissions needed.
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
    match aggregator::aggregate_tools(state, perms.user_id, servers, connected_toolkits, perms, traceparent)
        .await
    {
        Ok(tools) => ok(req_id, json!({ "tools": tools })),
        Err(e) => err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
    }
}

/// `tools/call` — route by namespace, enforce stance, intercept Composio
/// meta-tools, forward to the backend.
pub async fn handle_tools_call(
    state: &McpState,
    req_id: &Value,
    params: &Value,
    servers: &[MCPServerConfig],
    perms: &PermissionContext,
    traceparent: Option<&str>,
) -> Value {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let mut arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let (server, original) = match router::route_tool(tool_name, servers) {
        Ok(pair) => pair,
        Err(e) => return err(req_id, codes::INVALID_PARAMS, e.to_string()),
    };

    // ── Permission check for generic MCP tool calls ─────────────────────────
    if server.name != "composio" {
        match perms.get_stance(&server.name, &original) {
            Stance::Block => {
                tracing::info!(user = %perms.user_id, agent = %perms.agent_id, server = %server.name, tool = %original, "tool blocked");
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Tool '{tool_name}' is blocked for this agent."),
                );
            }
            Stance::Ask => {
                tracing::info!(user = %perms.user_id, agent = %perms.agent_id, server = %server.name, tool = %original, "tool requires approval");
                return err(
                    req_id,
                    codes::TOOL_ASK,
                    format!(
                        "Tool '{tool_name}' requires user approval before it can run. \
                         Please grant access in the agent settings."
                    ),
                );
            }
            Stance::Allow => {}
        }
    }

    // ── COMPOSIO_MANAGE_CONNECTIONS interception ────────────────────────────
    // A disabled server toggle must also stop (re)connecting that toolkit's OAuth,
    // not just tool execution.
    if tool_name == "COMPOSIO_MANAGE_CONNECTIONS" && !perms.disabled_servers.is_empty()
        && let Some(requested) = arguments.get("toolkits").and_then(|v| v.as_array()) {
            let requested: Vec<String> = requested
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let blocked: Vec<String> = requested
                .iter()
                .filter(|tk| !perms.is_server_enabled(&tk.to_ascii_lowercase()))
                .cloned()
                .collect();
            if !blocked.is_empty() {
                let allowed: Vec<String> =
                    requested.iter().filter(|tk| !blocked.contains(tk)).cloned().collect();
                if allowed.is_empty() {
                    return err(
                        req_id,
                        codes::TOOL_BLOCKED,
                        format!(
                            "Toolkit(s) are disabled for this agent: {blocked:?}. \
                             Enable them in agent settings before connecting."
                        ),
                    );
                }
                arguments["toolkits"] = json!(allowed);
            }
        }

    // ── COMPOSIO_MULTI_EXECUTE_TOOL interception ────────────────────────────
    if tool_name == "COMPOSIO_MULTI_EXECUTE_TOOL" && perms.has_any_restriction()
        && let Some(tools_arg) = arguments.get("tools").and_then(|v| v.as_array()).cloned()
            && !tools_arg.is_empty() {
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
                    if !perms.is_server_enabled(&toolkit) {
                        blocked_slugs.push(slug.to_string());
                    } else {
                        match perms.get_stance(&toolkit, slug) {
                            Stance::Block => blocked_slugs.push(slug.to_string()),
                            Stance::Ask => ask_slugs.push(slug.to_string()),
                            Stance::Allow => allowed.push(t.clone()),
                        }
                    }
                }

                // Nothing forwardable: "ask" is a softer signal than "block", so
                // surface the approval message when any tool needs approval.
                if allowed.is_empty() && (!blocked_slugs.is_empty() || !ask_slugs.is_empty()) {
                    if !ask_slugs.is_empty() {
                        return err(
                            req_id,
                            codes::TOOL_ASK,
                            format!(
                                "Tool(s) require user approval before running for this agent: \
                                 {ask_slugs:?}. Please grant access in the agent settings."
                            ),
                        );
                    }
                    return err(
                        req_id,
                        codes::TOOL_BLOCKED,
                        format!(
                            "All requested tools are blocked for this agent: {blocked_slugs:?}. \
                             Update tool permissions in agent settings to allow them."
                        ),
                    );
                }

                if !blocked_slugs.is_empty() || !ask_slugs.is_empty() {
                    tracing::info!(
                        user = %perms.user_id, agent = %perms.agent_id,
                        ?blocked_slugs, ?ask_slugs, forwarding = allowed.len(),
                        "partial composio multi-execute filter",
                    );
                    arguments["tools"] = json!(allowed);
                }
            }

    tracing::info!(tool = %tool_name, server = %server.name, forwarded_as = %original, "routing tool call");

    match state
        .providers
        .mcp
        .call_tool(server, req_id, &original, &arguments, DEFAULT_CALL_TIMEOUT, traceparent)
        .await
    {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!(server = %server.name, tool = %original, error = %e, "backend tool call failed");
            err(
                req_id,
                codes::INTERNAL_ERROR,
                format!("Backend '{}' failed to execute '{}'", server.name, original),
            )
        }
    }
}
