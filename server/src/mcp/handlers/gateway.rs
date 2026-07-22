//! Agent-facing aggregating gateway — `POST /api/mcp`.
//!
//! Deliberately NOT behind `require_auth`: an agent's only credential is the
//! delegation token (`x-nasiko-agent-token`, minted by `agent_proxy`). This
//! handler is thin — identity, usage tracking, flow events — all protocol logic
//! lives in `nasiko_mcp_gateway::protocol`.

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

const HEADER_AGENT_TOKEN: &str = "x-nasiko-agent-token";

/// Auth layer for `POST /api/mcp` — validates the delegation token and inserts a
/// `Claims { sub: user_id, .. }`, replacing `require_auth` for this one route.
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
    let Ok((user_id, _agent_id)) = nasiko_auth::jwt::validate_delegation_token(&jwt_secret, &token)
    else {
        return (
            StatusCode::UNAUTHORIZED,
            "invalid or expired delegation token",
        )
            .into_response();
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
    let Ok(user_id) = claims.sub.parse::<Uuid>() else {
        return rpc_error(
            &body_req_id(&body),
            codes::INTERNAL_ERROR,
            "invalid user identity",
        );
    };
    let Some(agent_id) = acting_agent_id(&headers, &claims.sub) else {
        return rpc_error(
            &body_req_id(&body),
            codes::INVALID_PARAMS,
            "missing or invalid x-nasiko-agent-token — a delegation token is required",
        );
    };

    let traceparent = headers
        .get(nasiko_flow::TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok());
    let method = body
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let tool_name = body
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let started = std::time::Instant::now();
    let Some(result) =
        protocol::handle_request(&state.mcp, user_id, agent_id, &body, traceparent).await
    else {
        return (StatusCode::ACCEPTED, Json(json!({}))).into_response();
    };

    if method == "tools/call" {
        let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
        let success = result.get("error").is_none();
        record_tool_usage(
            &state, user_id, agent_id, &tool_name, latency_ms, success, None,
        );

        if result
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_i64())
            == Some(codes::TOOL_ASK)
            && let Some(flow_ctx) = traceparent.and_then(nasiko_flow::FlowContext::from_traceparent)
        {
            // Prefer the connector name the protocol layer attached (tool prefixes
            // are opaque connector-id hex); fall back to the prefix, then composio.
            let server = result
                .get("error")
                .and_then(|e| e.get("data"))
                .and_then(|d| d.get("server"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    tool_name
                        .split_once("__")
                        .map(|(s, _)| s.to_string())
                        .unwrap_or_else(|| "composio".to_string())
                });
            state
                .flow_events
                .publish(
                    &flow_ctx.flow_id,
                    nasiko_flow::FlowEvent::ToolApprovalRequired {
                        agent_id: agent_id.to_string(),
                        server,
                        tool: tool_name.clone(),
                    },
                )
                .await;
        }
    }

    Json(result).into_response()
}

fn body_req_id(body: &Value) -> Value {
    body.get("id").cloned().unwrap_or(Value::Null)
}

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

/// Validate the delegation token and return the acting agent's UUID, only if its
/// `sub` matches the authenticated `user_id` (defense in depth).
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
