//! Agent-facing aggregating gateway — `POST /api/mcp`.
//!
//! Deliberately NOT behind `require_auth`: agents authenticate with their own
//! deploy-time credential (`Authorization: Bearer $MCP_GATEWAY_TOKEN`, minted
//! by `mcp::wiring` and stored hashed in `agent_gateway_tokens`), and the user
//! identity is resolved server-side from the request's `traceparent` via the
//! `flows` row + `flow_participants` check — see
//! docs/MCP_GATEWAY_AGENT_AUTH.md §2.4. This handler is thin — identity, usage
//! tracking, flow events — all protocol logic lives in
//! `nasiko_mcp_gateway::protocol`.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::protocol;
use nasiko_mcp_gateway::types::codes;

use crate::state::AppState;
use crate::usage::TokenUsageBuilder;

/// Agent-facing MCP JSON-RPC gateway. Generic `tools/list` / `tools/call`
/// JSON-RPC 2.0; request and response bodies are protocol-defined, not
/// REST-shaped, so they are documented as free-form objects.
///
/// Authorization rules (docs/MCP_GATEWAY_AGENT_AUTH.md §2.4), all failing
/// closed:
/// 1. bearer token missing/unknown/revoked → 401
/// 2. `tools/list` (and initialize/ping) → allowed with agent-only identity
/// 3. `tools/call` with no/unknown/dead-flow traceparent → 403
/// 4. `tools/call` where the agent is not a recorded flow participant → 403
/// 5. identity store unreachable → 403
#[utoipa::path(
    post,
    path = "/api/mcp",
    tag = "mcp",
    params(
        ("Authorization" = String, Header, description = "`Bearer <MCP_GATEWAY_TOKEN>` — the per-agent gateway credential injected into the container env at deploy time"),
        ("traceparent" = Option<String>, Header, description = "W3C trace context naming the flow this call belongs to — required for `tools/call`; the user identity is resolved from the flow record"),
    ),
    request_body(content = Object, description = "JSON-RPC 2.0 request: `tools/list` or `tools/call`"),
    responses(
        (status = 200, description = "JSON-RPC 2.0 response (result or error object)", body = Object),
        (status = 202, description = "Notification accepted (no `id` in request); empty object body", body = Object),
        (status = 401, description = "Missing/unknown/revoked gateway token"),
        (status = 403, description = "`tools/call` outside a live flow the agent participates in, or identity store unavailable"),
    ),
)]
pub async fn mcp_gateway(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // Rule 1: authenticate the agent by its deploy-time gateway credential.
    let Some(token) = bearer_token(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            "missing Authorization: Bearer <MCP_GATEWAY_TOKEN>",
        )
            .into_response();
    };
    dispatch(&state, token, &headers, body).await
}

/// URL-credential form of [`mcp_gateway`] — `POST /api/mcp/s/{token}`.
///
/// Identical in every respect except where the agent credential is read from:
/// the path instead of the `Authorization` header. It exists because MCP, unlike
/// the OpenAI SDKs behind the LLM router, has no env-var convention for
/// credentials — frameworks that register MCP servers declaratively often expose
/// a `url` and no header hook, leaving the URL as the only place a credential
/// can travel.
///
/// The trade-off is deliberate and documented (docs/MCP_GATEWAY_AGENT_AUTH.md):
/// a credential in a URL is easier to leak than one in a header, so the server
/// redacts this path before it reaches a span or log line
/// (`crate::mcp::redact_credential_uri`). What keeps the exposure bounded is
/// that this credential proves only *which agent* is calling — `tools/call`
/// still requires a `traceparent` naming a live flow the agent participates in,
/// so a leaked URL on its own cannot invoke a tool.
#[utoipa::path(
    post,
    path = "/api/mcp/s/{token}",
    tag = "mcp",
    params(
        ("token" = String, Path, description = "The per-agent `MCP_GATEWAY_TOKEN`, carried in the path for MCP clients that cannot set headers. Pre-composed as `MCP_GATEWAY_CONNECT_URL` in the container env."),
        ("traceparent" = Option<String>, Header, description = "W3C trace context naming the flow this call belongs to — required for `tools/call`; the user identity is resolved from the flow record"),
    ),
    request_body(content = Object, description = "JSON-RPC 2.0 request: `tools/list` or `tools/call`"),
    responses(
        (status = 200, description = "JSON-RPC 2.0 response (result or error object)", body = Object),
        (status = 202, description = "Notification accepted (no `id` in request); empty object body", body = Object),
        (status = 401, description = "Unknown/revoked gateway token"),
        (status = 403, description = "`tools/call` outside a live flow the agent participates in, or identity store unavailable"),
    ),
)]
pub async fn mcp_gateway_via_url(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if token.trim().is_empty() {
        return (StatusCode::UNAUTHORIZED, "empty gateway token").into_response();
    }
    dispatch(&state, token.trim(), &headers, body).await
}

/// Shared body of both entry points: everything after the credential has been
/// located. Keeping this single means the two forms cannot drift into different
/// authorization behaviour — the URL form is a transport detail, not a weaker
/// door.
async fn dispatch(state: &AppState, token: &str, headers: &HeaderMap, body: Value) -> Response {
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
    let header_traceparent = headers
        .get(nasiko_flow::TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok());

    // Two doors onto the same tail:
    //   - a coding-agent *session token* (NAS-839) carries {agent, user, flow} in
    //     signed claims, so it needs neither the ngt_ lookup nor a traceparent;
    //   - otherwise the deploy-time ngt_ gateway token names the agent and the
    //     user is inferred from the request's traceparent + flow (the A2A path,
    //     docs/MCP_GATEWAY_AGENT_AUTH.md §2.4).
    let (agent_id, user_id, synthesized_tp): (Uuid, Uuid, Option<String>) =
        match try_session_identity(state, token).await {
            SessionResolution::Session {
                agent_id,
                user_id,
                flow_id,
            } => (agent_id, user_id, Some(synth_traceparent(&flow_id))),
            SessionResolution::Deny(resp) => return resp,
            SessionResolution::NotSession => {
                let agent_id = match nasiko_mcp_gateway::agent_tokens::authenticate(
                    &state.db, token,
                )
                .await
                {
                    Ok(Some(id)) => id,
                    Ok(None) => {
                        return (StatusCode::UNAUTHORIZED, "unknown or revoked gateway token")
                            .into_response();
                    }
                    // Rule 5: identity store unreachable → fail closed (403,
                    // not a retryable 5xx that reads as "try without auth").
                    Err(e) => {
                        tracing::error!(error = %e, "mcp gateway: token lookup failed — failing closed");
                        return (StatusCode::FORBIDDEN, "identity store unavailable")
                            .into_response();
                    }
                };

                // `tools/call` (rules 3+4) requires the traceparent to name a live
                // flow this agent was dispatched into — the flow's user is the
                // subject. Read-only methods (rule 2) fall back to the agent owner.
                let user_id = match flow_user(state, header_traceparent, agent_id).await {
                    Ok(user_id) => user_id,
                    Err(denial) => {
                        if method == "tools/call" {
                            return denial;
                        }
                        match agent_owner(state, agent_id).await {
                            Ok(Some(owner)) => owner,
                            Ok(None) => {
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    "agent no longer exists — gateway token is stale",
                                )
                                    .into_response();
                            }
                            // An unreachable identity store is not "the agent is
                            // gone"; a 401 here would trigger a pointless rotate.
                            Err(e) => {
                                tracing::error!(error = %e, %agent_id, "mcp gateway: owner lookup failed — failing closed");
                                return (StatusCode::FORBIDDEN, "identity store unavailable")
                                    .into_response();
                            }
                        }
                    }
                };
                (agent_id, user_id, None)
            }
        };

    // The session path supplies a flow via its claims; the A2A path uses the
    // request's own header.
    let traceparent: Option<&str> = synthesized_tp.as_deref().or(header_traceparent);

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
            state, user_id, agent_id, &tool_name, latency_ms, success, None,
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

/// The `Bearer` value of the `Authorization` header, if any.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Rules 3–5: resolve the flow named by `traceparent` to its user, requiring
/// the flow to be live (`status = 'running'`, younger than the platform's flow
/// timeout — a flow can't legitimately outlive `NASIKO_FLOW_TIMEOUT_SECS`) and
/// the authenticated agent to be a recorded participant. Every failure is a
/// 403 with a descriptive body: presence is not the check, resolution is —
/// there is nothing an agent can fabricate to pass.
#[allow(clippy::result_large_err)]
async fn flow_user(
    state: &AppState,
    traceparent: Option<&str>,
    agent_id: Uuid,
) -> Result<Uuid, Response> {
    let flow_id = traceparent
        .and_then(nasiko_flow::FlowContext::from_traceparent)
        .map(|ctx| ctx.flow_id)
        .ok_or_else(|| {
            deny(format!(
                "traceparent missing or malformed (received: {:?}) — tools/call must carry the W3C trace context of the flow it serves",
                traceparent.unwrap_or("<none>")
            ))
        })?;

    let row: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT f.user_id,
                EXISTS(SELECT 1 FROM flow_participants fp
                       WHERE fp.flow_id = f.flow_id AND fp.agent_id = $2)
         FROM flows f
         WHERE f.flow_id = $1
           AND f.status = 'running'
           AND f.created_at > now() - make_interval(secs => $3)",
    )
    .bind(&flow_id)
    .bind(agent_id)
    .bind(f64::from(state.config.flow_timeout_secs))
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        // Rule 5: fail closed on identity-store failure.
        tracing::error!(error = %e, %flow_id, "mcp gateway: flow lookup failed — failing closed");
        deny("identity store unavailable".to_string())
    })?;

    match row {
        Some((user_id, true)) => Ok(user_id),
        Some((_, false)) => Err(deny(format!(
            "agent {agent_id} is not a participant of flow {flow_id}"
        ))),
        None => Err(deny(format!(
            "traceparent does not resolve to a live flow (trace_id {flow_id})"
        ))),
    }
}

fn deny(body: String) -> Response {
    (StatusCode::FORBIDDEN, body).into_response()
}

/// Outcome of trying to read a coding-agent session token off the credential.
enum SessionResolution {
    /// A valid, non-revoked session token: use its signed identity directly.
    Session {
        agent_id: Uuid,
        user_id: Uuid,
        flow_id: String,
    },
    /// Not a session token (the ngt_ gateway token, or anything else) — the
    /// caller should fall back to the deploy-time-token + traceparent path.
    NotSession,
    /// A session token that must be rejected (revoked, malformed claims, or the
    /// store could not be reached).
    Deny(Response),
}

/// Try to authenticate the credential as a coding-agent session token.
///
/// The signature is the proof of `{agent_id, user_id, flow_id}`, so — unlike the
/// A2A path — this needs no live-flow freshness window (a long turn is never
/// rejected for age) and no `flow_participants` lookup. The one DB touch is the
/// `jti` revocation check, so a token can still be killed before its `exp`.
async fn try_session_identity(state: &AppState, token: &str) -> SessionResolution {
    let secret = &state.config.agent_jwt_secret;
    // Unconfigured secret ⇒ this deployment doesn't issue session tokens; leave
    // the legacy path untouched.
    if secret.is_empty() {
        return SessionResolution::NotSession;
    }
    let algorithm = nasiko_session_token::parse_algorithm(&state.config.agent_jwt_algorithm);

    let claims = match nasiko_session_token::verify_session_token(token, secret, algorithm) {
        Ok(claims) => claims,
        Err(nasiko_session_token::SessionTokenError::NotSessionToken) => {
            return SessionResolution::NotSession;
        }
        // Expired or otherwise unusable session token: reject rather than fall
        // back (a fallback would just 401 on the ngt_ lookup anyway).
        Err(e) => {
            return SessionResolution::Deny(
                (
                    StatusCode::UNAUTHORIZED,
                    format!("session token rejected: {e}"),
                )
                    .into_response(),
            );
        }
    };

    let agent_id = match Uuid::parse_str(&claims.agent_id) {
        Ok(id) => id,
        Err(_) => {
            return SessionResolution::Deny(deny("session token agent_id is not a UUID".into()));
        }
    };
    let user_id = match Uuid::parse_str(&claims.user_id) {
        Ok(id) => id,
        Err(_) => {
            return SessionResolution::Deny(deny("session token user_id is not a UUID".into()));
        }
    };

    match is_session_token_revoked(&state.db, &claims.jti).await {
        Ok(false) => SessionResolution::Session {
            agent_id,
            user_id,
            flow_id: claims.flow_id,
        },
        Ok(true) => SessionResolution::Deny(deny("session token revoked".into())),
        Err(e) => {
            // Rule 5: fail closed on identity-store failure.
            tracing::error!(error = %e, "mcp gateway: revocation check failed — failing closed");
            SessionResolution::Deny(deny("identity store unavailable".into()))
        }
    }
}

/// True if this `jti` has been revoked.
async fn is_session_token_revoked(db: &sqlx::PgPool, jti: &str) -> sqlx::Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM revoked_session_tokens WHERE jti = $1")
            .bind(jti)
            .fetch_optional(db)
            .await?;
    Ok(found.is_some())
}

/// Build a well-formed `traceparent` for a flow id from a session token, so the
/// rest of the gateway (flow events, tool-approval context) sees the same W3C
/// trace context the A2A path carries in a header.
fn synth_traceparent(flow_id: &str) -> String {
    let mut ctx = nasiko_flow::FlowContext::new_root();
    ctx.flow_id = flow_id.to_owned();
    ctx.to_traceparent()
}

/// Owner of a live (non-deleted) agent — the agent-only identity used for
/// read-only methods outside a flow.
///
/// `Ok(None)` means the agent is genuinely gone (401, stale credential);
/// `Err` means the store could not answer (403, fail closed). Collapsing the
/// two would misreport a database outage as a revoked agent.
async fn agent_owner(state: &AppState, agent_id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1 AND deleted_at IS NULL")
        .bind(agent_id)
        .fetch_optional(&state.db)
        .await
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
