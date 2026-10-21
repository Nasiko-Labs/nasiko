//! Agent-facing aggregating gateway — `POST /api/mcp`.
//!
//! Deliberately NOT behind `require_auth`: agents authenticate with their own
//! deploy-time credential (`Authorization: Bearer $MCP_GATEWAY_TOKEN`, minted
//! by `mcp::wiring` and stored hashed in `agent_gateway_tokens`), and the user
//! identity is resolved server-side — from the request's `traceparent` via the
//! `flows` row + `flow_participants` check, or, for a CLI-bound local
//! coding-agent row with no flow (or none that resolves), from the row's own
//! owner instead — see docs/MCP_GATEWAY_AGENT_AUTH.md §2.4 rule 3b. This
//! handler is thin — identity, usage tracking, flow events — all protocol
//! logic lives in `nasiko_mcp_gateway::protocol`.

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
/// 3b. …unless the agent's row is a CLI-bound local coding agent
///     (`coding_agent_integration_id` set) — such a row is never
///     dispatched through a flow, so a flow-less `tools/call` resolves to its
///     owner instead of 403 (`agent_owner`, below `dispatch`)
/// 4. `tools/call` where the agent is not a recorded flow participant → 403
/// 5. identity store unreachable → 403
#[utoipa::path(
    post,
    path = "/api/mcp",
    tag = "mcp",
    params(
        ("Authorization" = String, Header, description = "`Bearer <MCP_GATEWAY_TOKEN>` — the per-agent gateway credential injected into the container env at deploy time"),
        ("traceparent" = Option<String>, Header, description = "W3C trace context naming the flow this call belongs to — required for `tools/call`, except for a CLI-bound local coding-agent row (`coding_agent_integration_id` set), which resolves to its owner when the flow lookup fails; otherwise the user identity is resolved from the flow record"),
        ("MCP-Protocol-Version" = Option<String>, Header, description = "Negotiated MCP protocol version; an unsupported value is rejected with 400 on implemented methods"),
    ),
    request_body(content = Object, description = "JSON-RPC 2.0 request: `tools/list` or `tools/call`"),
    responses(
        (status = 200, description = "JSON-RPC 2.0 response (result or error object)", body = Object),
        (status = 202, description = "Notification accepted (no `id` in request); empty object body", body = Object),
        (status = 400, description = "Unsupported or undecodable `MCP-Protocol-Version` on a method this gateway implements"),
        (status = 401, description = "Missing/unknown/revoked gateway token"),
        (status = 403, description = "`tools/call` outside a live flow the agent participates in (except a local coding-agent row, which resolves to its owner instead), or identity store unavailable"),
        (status = 413, description = "Request body over the configured `MCP_GATEWAY_MAX_BODY_BYTES` limit"),
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
/// that this credential proves only *which agent* is calling — for a
/// deployed agent, `tools/call` still requires a `traceparent` naming a live
/// flow the agent participates in, so a leaked URL on its own cannot invoke a
/// tool.
///
/// That bound does NOT hold for a CLI-bound local coding-agent row
/// (`coding_agent_integration_id` set): such a row has no
/// flow to require, so a leaked connect URL for one CAN call tools —
/// attributed to, and scoped to, that row's owner: exactly the connectors the
/// owner can access (their own, plus anything shared to them) that are also
/// enabled for this specific agent row, never another user's. This is the
/// same attribution the LLM router already applies to these rows with no flow
/// at all (`oss/llm-router/src/handlers/chat.rs:312-323`). Nothing prevents
/// such a row from being shared out or having its owner changed; whoever else
/// can see the row, a flow-less call is attributed to its *current*
/// `owner_id`, so an ownership transfer moves this attribution with it. The
/// credential itself has no TTL of its own — it rotates only on re-mint
/// (`nasiko_mcp_gateway::agent_tokens::mint`, same primitive every gateway
/// token uses): the row's owner rotates it with `POST
/// /api/agents/{id}/mcp-token` (owner-only), or kills it outright with
/// `DELETE /api/agents/{id}/mcp-token` (owner or superuser); a deployed agent
/// instead gets a fresh one on its next redeploy. `POST` alone does not cut
/// off a leak immediately, though: the superseded token stays accepted for
/// `ROTATION_GRACE_SECS` after a re-mint, same as a redeploy's rotation —
/// `DELETE` is the one call with no such grace. Absent a `DELETE`, a leaked
/// coding-agent connect URL is live for as long as the row is.
#[utoipa::path(
    post,
    path = "/api/mcp/s/{token}",
    tag = "mcp",
    params(
        ("token" = String, Path, description = "The per-agent `MCP_GATEWAY_TOKEN`, carried in the path for MCP clients that cannot set headers. Pre-composed as `MCP_GATEWAY_CONNECT_URL` in the container env."),
        ("traceparent" = Option<String>, Header, description = "W3C trace context naming the flow this call belongs to — required for `tools/call`, except for a CLI-bound local coding-agent row (`coding_agent_integration_id` set), which resolves to its owner when the flow lookup fails; otherwise the user identity is resolved from the flow record"),
        ("MCP-Protocol-Version" = Option<String>, Header, description = "Negotiated MCP protocol version; an unsupported value is rejected with 400 on implemented methods"),
    ),
    request_body(content = Object, description = "JSON-RPC 2.0 request: `tools/list` or `tools/call`"),
    responses(
        (status = 200, description = "JSON-RPC 2.0 response (result or error object)", body = Object),
        (status = 202, description = "Notification accepted (no `id` in request); empty object body", body = Object),
        (status = 400, description = "Unsupported or undecodable `MCP-Protocol-Version` on a method this gateway implements"),
        (status = 401, description = "Unknown/revoked gateway token"),
        (status = 403, description = "`tools/call` outside a live flow the agent participates in (except a local coding-agent row, which resolves to its owner instead), or identity store unavailable"),
        (status = 413, description = "Request body over the configured `MCP_GATEWAY_MAX_BODY_BYTES` limit"),
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
    let agent_id = match nasiko_mcp_gateway::agent_tokens::authenticate(&state.db, token).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, "unknown or revoked gateway token").into_response();
        }
        // Rule 5: identity store unreachable → fail closed (403, not a retryable
        // 5xx that could be read as "try without auth"), matching FlowGuard's
        // GuardUnavailable posture.
        Err(e) => {
            tracing::error!(error = %e, "mcp gateway: token lookup failed — failing closed");
            return (StatusCode::FORBIDDEN, "identity store unavailable").into_response();
        }
    };

    // `method` must be known before the protocol-version check below, because
    // the check is gated on it: streamable-http clients (openai-agents,
    // pydantic-ai, Claude Code) open a session with a method this gateway
    // doesn't implement (e.g. `server/discover`), carrying whatever protocol
    // version they intend to negotiate, and rely on our `-32601` to trigger
    // their fallback to `initialize`. Rejecting that first probe with a bare
    // 400 breaks the fallback outright, so the header is enforced only for
    // methods this gateway actually answers (`protocol::implements`) — and
    // never for `initialize` itself, which is the negotiation request.
    let method = body
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    // Per the MCP spec, a client that negotiated a protocol version in
    // `initialize` sends `MCP-Protocol-Version` on every subsequent request.
    // The header is optional — a client that never negotiated (or an old
    // client predating this) never sends it — but on a method we do
    // implement, a version we don't (or a header we can't even decode) is
    // rejected outright rather than silently ignored.
    if method != "initialize" && protocol::implements(&method) {
        match headers
            .get(nasiko_mcp_gateway::types::PROTOCOL_VERSION_HEADER)
            .map(|v| v.to_str())
        {
            None => {}
            Some(Ok(v)) if nasiko_mcp_gateway::types::SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => {}
            Some(Ok(v)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "unsupported MCP-Protocol-Version '{v}' (supported: {})",
                        nasiko_mcp_gateway::types::SUPPORTED_PROTOCOL_VERSIONS.join(", ")
                    ),
                )
                    .into_response();
            }
            Some(Err(_)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "MCP-Protocol-Version header is not valid visible ASCII",
                )
                    .into_response();
            }
        }
    }

    let traceparent = headers
        .get(nasiko_flow::TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok());
    // `invoked_tool_name`, not `params.name`: a `nasiko_call_tool` call is
    // recorded — usage, metrics, the approval event below — as the inner
    // tool it ran, resolved by the same parse the protocol layer dispatches on.
    let tool_name = body
        .get("params")
        .map(protocol::invoked_tool_name)
        .unwrap_or("")
        .to_string();

    // Resolve the user identity. `tools/call` (rules 3+4) requires the
    // traceparent to name a live flow this agent was dispatched into — the
    // flow's user is the authorization subject — except for a CLI-bound
    // local coding-agent row (rule 3b, `agent_owner` below), which has
    // no flow to require and resolves to its owner instead. Read-only methods
    // (initialize/ping/tools/list, rule 2) work agent-only: the flow user when
    // one resolves, else the agent's owner (startup-time tool discovery
    // happens outside any flow).
    //
    // `verified_flow_id` travels alongside `user_id` (rather than being
    // re-derived from `traceparent` further down) because it's the one thing
    // this function actually verified against `flows`/`flow_participants` —
    // the raw `traceparent` a caller sends is not proof of anything on its
    // own. On the owner-fallback branch below, `user_id` did NOT come from a
    // verified flow, so `verified_flow_id` must be `None` even though
    // `traceparent` may still be a well-formed (just unresolvable, or
    // rejected) value: `protocol::handle_request` signs `verified_flow_id`,
    // never `traceparent`, into the identity header it forwards to system
    // backends, specifically so a caller can't launder an unverified trace id
    // into a header a backend is told to trust unconditionally.
    let (user_id, verified_flow_id) = match flow_user(state, traceparent, agent_id).await {
        Ok((user_id, flow_id)) => (user_id, Some(flow_id)),
        Err(denial) => match agent_owner(state, agent_id).await {
            // Deliberate policy (MCP_GATEWAY_AGENT_AUTH.md §5, rule 3b):
            // a local coding agent — a row the CLI bound with
            // `coding_agent_integration_id` — is never dispatched through the
            // proxy, so it never has a flow; it acts as its owner instead,
            // even for `tools/call`. Same predicate the LLM router uses for
            // the same rows (`oss/llm-router/src/resolver/mod.rs`,
            // `coding_agent_integration_id IS NOT NULL`), so the gateway and
            // the router agree on which rows count as "personal desks" —
            // though not on how they're billed: the router always attributes
            // these rows to their owner with no flow at all, while the
            // gateway takes a verified flow first (above) and only reaches
            // this fallback when one doesn't resolve. That difference is
            // unreachable today: a coding-agent row is never dispatched into,
            // so nothing ever has an endpoint of its own to route a flow to.
            Ok(Some((owner, true))) => (owner, None),
            // Every other flow-less `tools/call` stays denied (rules 3+4).
            Ok(_) if method == "tools/call" => return denial,
            // Read-only methods (rule 2): agent-only identity via the owner —
            // unchanged from before the coding-agent policy existed.
            Ok(Some((owner, false))) => (owner, None),
            Ok(None) => {
                return (
                    StatusCode::UNAUTHORIZED,
                    "agent no longer exists — gateway token is stale",
                )
                    .into_response();
            }
            // Rule 5 again: an unreachable identity store is not "the agent
            // is gone". Reporting 401 here would tell a healthy agent its
            // credential is stale and trigger a pointless rotate/redeploy.
            Err(e) => {
                tracing::error!(error = %e, %agent_id, "mcp gateway: owner lookup failed — failing closed");
                return (StatusCode::FORBIDDEN, "identity store unavailable").into_response();
            }
        },
    };

    let started = std::time::Instant::now();
    let Some(result) = protocol::handle_request(
        &state.mcp,
        user_id,
        agent_id,
        &body,
        traceparent,
        verified_flow_id.as_deref(),
    )
    .await
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
            // `verified_flow_id`, never a re-parse of the raw `traceparent`:
            // the owner-fallback path (coding-agent row, or a foreign/dead
            // trace id) hands back `None` here specifically because nobody
            // proved this call belongs to that flow. Re-parsing `traceparent`
            // instead would let a coding-agent row — which resolves to its
            // owner precisely because it has no flow of its own — name any
            // OTHER user's live flow and land an approval card, naming an
            // attacker-chosen server/tool, in that user's SSE stream
            // (`a2a_dispatch.rs` forwards `FlowEventBus` events straight into
            // the flow owner's chat).
            && let Some(flow_id) = verified_flow_id.as_deref()
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
                    flow_id,
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
///
/// Returns the flow id alongside the user on success — this is the only place
/// that verifies a `traceparent`-named flow is live and this agent
/// participates in it, so it's also the only place allowed to hand that flow
/// id onward as trustworthy (`dispatch` forwards it to
/// `protocol::handle_request` as `verified_flow_id`, which alone may be signed
/// into the identity header sent to system backends). Callers on the
/// owner-fallback path (this returns `Err`) must never substitute a raw,
/// unverified `traceparent` in its place.
#[allow(clippy::result_large_err)]
async fn flow_user(
    state: &AppState,
    traceparent: Option<&str>,
    agent_id: Uuid,
) -> Result<(Uuid, String), Response> {
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
        Some((user_id, true)) => Ok((user_id, flow_id)),
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

/// `(owner_id, is_coding_agent)` for a live (non-deleted) agent — one query
/// where there used to be two (a plain `agent_owner` for the read-only
/// fallback, a separate `coding_agent_owner` for rule 3b), because the
/// `Err(denial)` arm above always needs both facts about the very same row at
/// once. `is_coding_agent` is `coding_agent_integration_id IS NOT NULL`
/// (`oss/migrations/0019_coding_agent_identity.sql`) — deliberately the
/// column itself, never agent metadata or name — matching `is_coding_agent`
/// in `oss/llm-router/src/resolver/mod.rs`, so the gateway and the LLM router
/// agree on which rows this policy covers. Such a row is single-owner by
/// construction (the CLI only binds rows the login owns;
/// `agent_owner_or_reject` gates every mint), so "the owner" is unambiguous.
///
/// `Ok(None)` means the agent is genuinely gone (401, stale credential);
/// `Err` means the store could not answer (403, fail closed). Collapsing the
/// two would misreport a database outage as a revoked agent.
async fn agent_owner(
    state: &AppState,
    agent_id: Uuid,
) -> Result<Option<(Uuid, bool)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT owner_id, coding_agent_integration_id IS NOT NULL
         FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
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
