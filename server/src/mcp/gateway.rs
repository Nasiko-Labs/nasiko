//! Agent-facing aggregating gateway — `POST /api/mcp`.
//!
//! Speaks MCP JSON-RPC 2.0. **This route is deliberately NOT behind
//! `require_auth`** — a deployed agent never holds the calling user's real
//! session JWT (`agent_proxy.rs` strips `Authorization`/`Cookie` before
//! forwarding to a container, on purpose, so an agent can never replay the
//! user's platform credentials). Instead [`require_delegation`] is mounted as
//! this route's own auth layer: it validates the short-lived delegation JWT
//! (`x-nasiko-agent-token`, minted by `agent_proxy::agent_proxy` /
//! `router::a2a_dispatch` when the server forwards a request to an agent) and
//! synthesizes a `Claims` from its `sub` claim — that JWT is the *only*
//! credential an agent ever has for this route. Errors are returned as
//! JSON-RPC error objects (HTTP 200), since the caller is an MCP client, not a
//! REST client.
//!
//! `mcp_gateway` below is deliberately thin: it only does the things that
//! genuinely need `AppState`/`Claims` (identity extraction, usage tracking,
//! flow events). All MCP JSON-RPC protocol logic — notification handling,
//! `initialize`/`ping`, permission/session-aware `tools/list`+`tools/call`
//! dispatch — lives in `nasiko_mcp_gateway::protocol::handle_request`, so it's
//! reusable by any route that can supply a validated `(user_id, agent_id)`,
//! including a future EE-specific one, without copy-pasting this dispatch.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::protocol;
use nasiko_mcp_gateway::types::codes;

use crate::auth::Claims;
use crate::state::AppState;
use crate::usage::TokenUsageBuilder;

/// Header carrying the delegation JWT minted by `agent_proxy::agent_proxy`.
/// Its `act` claim is the acting agent's UUID — never trust this header's
/// value without validating the token's signature first (see `acting_agent_id`).
const HEADER_AGENT_TOKEN: &str = "x-nasiko-agent-token";

/// Auth layer for `POST /api/mcp` — replaces `require_auth` for this one route.
///
/// An agent's only credential is the delegation token, never a session JWT, so
/// this validates it directly and inserts a `Claims { sub: user_id, .. }` into
/// request extensions, exactly what `require_auth` would have inserted for a
/// normal user request. `mcp_gateway` below re-validates the same token to
/// extract `agent_id` — this double-validate costs a few microseconds and
/// keeps `acting_agent_id`'s existing tested logic untouched.
pub async fn require_delegation(mut req: Request, next: Next) -> Response {
    let jwt_secret = match std::env::var("JWT_SECRET") {
        Ok(s) => s,
        Err(_) => return (StatusCode::UNAUTHORIZED, "delegation auth unavailable").into_response(),
    };
    let Some(token) = req
        .headers()
        .get(HEADER_AGENT_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return (StatusCode::UNAUTHORIZED, "missing x-nasiko-agent-token").into_response();
    };
    let Ok((user_id, _agent_id)) = nasiko_auth::jwt::validate_delegation_token(&jwt_secret, &token) else {
        return (StatusCode::UNAUTHORIZED, "invalid or expired delegation token").into_response();
    };
    req.extensions_mut().insert(Claims {
        sub: user_id,
        username: String::new(),
        is_superuser: false,
    });
    next.run(req).await
}

pub async fn mcp_gateway(
    State(state): State<AppState>,
    claims: Claims,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // ── Identity — the one thing this route must resolve itself, since it
    // needs `Claims`/`headers` (Axum-specific, not available to the crate) ──
    let Ok(user_id) = claims.sub.parse::<Uuid>() else {
        return rpc_error(&body_req_id(&body), codes::INTERNAL_ERROR, "invalid user identity");
    };
    let Some(agent_id) = acting_agent_id(&headers, &claims.sub) else {
        return rpc_error(
            &body_req_id(&body),
            codes::INVALID_PARAMS,
            "missing or invalid x-nasiko-agent-token — a delegation token is required to call the MCP gateway",
        );
    };

    // Propagated to backend MCP servers so a tool call joins the agent's
    // existing distributed trace instead of showing up as an orphan span.
    let traceparent = headers
        .get(nasiko_flow::TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok());
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let tool_name = body
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // ── All MCP protocol logic lives in the crate ───────────────────────────
    let started = std::time::Instant::now();
    let Some(result) = protocol::handle_request(&state.mcp, user_id, agent_id, &body, traceparent).await
    else {
        // A request with no `id` is an MCP notification — acknowledge, no body.
        return (StatusCode::ACCEPTED, Json(json!({}))).into_response();
    };

    // ── Server-only concerns for `tools/call`: usage tracking + human-in-the-
    // loop flow events. These stay here because `UsageTracker`/`FlowEventBus`
    // are `AppState` types the crate must never depend on (§20 of the design
    // doc). Note: unlike before this refactor, the recorded latency now spans
    // permission/session resolution too (both Redis-cached, so negligible in
    // practice), since that now happens inside `handle_request`.
    if method == "tools/call" {
        let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
        let success = result.get("error").is_none();
        record_tool_usage(
            &state,
            user_id,
            agent_id,
            &tool_name,
            latency_ms,
            success,
            // OSS `Claims` carries no team_id (enterprise-only field) —
            // usage metadata simply omits it in this edition.
            None,
        );

        // Phase-2 human-in-the-loop: when a tool needs approval (ask stance →
        // -32001), also emit a FlowEvent onto the chat's flow so the UI can
        // surface an approval prompt. The flow is identified by the inbound
        // `traceparent` the agent propagates; absent it, we just return -32001.
        if result.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64()) == Some(codes::TOOL_ASK)
            && let Some(flow_ctx) = traceparent.and_then(nasiko_flow::FlowContext::from_traceparent)
        {
            let server = tool_name.split_once("__").map(|(s, _)| s).unwrap_or("composio");
            state
                .flow_events
                .publish(
                    &flow_ctx.flow_id,
                    nasiko_flow::FlowEvent::ToolApprovalRequired {
                        agent_id: agent_id.to_string(),
                        server: server.to_string(),
                        tool: tool_name.clone(),
                    },
                )
                .await;
        }
    }

    Json(result).into_response()
}

/// A best-effort `id` to attach to an identity-failure error — extracted
/// before we know whether the request even has one (a malformed/absent `id`
/// here just means the client sees `null`, which is a normal JSON-RPC id).
fn body_req_id(body: &Value) -> Value {
    body.get("id").cloned().unwrap_or(Value::Null)
}

/// Record a tool call into the platform's observability sinks: OTel GenAI
/// metrics (synchronous, in-process) + `token_usage` (spawned, best-effort — a
/// telemetry failure must never fail or delay the tool response).
fn record_tool_usage(
    state: &AppState,
    user_id: Uuid,
    agent_id: Uuid,
    tool_name: &str,
    latency_ms: i32,
    success: bool,
    team_id: Option<&str>,
) {
    state.genai_metrics.record_tool_call(
        latency_ms as f64 / 1000.0,
        tool_name,
        &agent_id.to_string(),
    );

    let usage = TokenUsageBuilder::new(user_id, "mcp_tool_call", "mcp", tool_name)
        .agent_id(agent_id)
        .latency_ms(latency_ms)
        .finish_reason(if success { "ok" } else { "error" })
        .metadata(json!({ "tool": tool_name, "success": success, "team_id": team_id }))
        .build();

    let tracker = state.usage_tracker.clone();
    tokio::spawn(async move {
        if let Err(e) = tracker.track_tokens(usage).await {
            tracing::warn!(error = %e, "failed to record mcp_tool_call usage");
        }
    });
}

/// Validate the delegation token in `x-nasiko-agent-token` and return the
/// acting agent's UUID — only if the token's signature, expiry, and audience
/// check out AND its `sub` matches the already-authenticated `user_id`
/// (defense in depth: a delegation token stolen from another user's session
/// must not be usable here even if it's otherwise well-formed).
fn acting_agent_id(headers: &HeaderMap, user_id: &str) -> Option<Uuid> {
    let token = headers.get(HEADER_AGENT_TOKEN)?.to_str().ok()?;
    let jwt_secret = std::env::var("JWT_SECRET").ok()?;
    let (delegated_user_id, agent_id) =
        nasiko_auth::jwt::validate_delegation_token(&jwt_secret, token).ok()?;
    if delegated_user_id != user_id {
        return None;
    }
    agent_id.parse().ok()
}

fn rpc_error(id: &Value, code: i64, message: impl Into<String>) -> Response {
    Json(protocol::rpc_error(id, code, message)).into_response()
}
