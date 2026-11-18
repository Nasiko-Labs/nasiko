//! Agent-facing aggregating gateway — `POST /api/mcp`.
//!
//! Deliberately NOT behind `require_auth`: agents authenticate with their own
//! deploy-time credential (`Authorization: Bearer $MCP_GATEWAY_TOKEN`, minted
//! by `mcp::wiring` and stored hashed in `agent_gateway_tokens`), and the user
//! identity is resolved server-side — from the request's `traceparent` via the
//! `flows` row + `flow_participants` check, or, for a CLI-bound local
//! coding-agent row or a discovered A2A agent's row with no flow (or none
//! that resolves), from the row's own owner instead — see
//! docs/MCP_GATEWAY_AGENT_AUTH.md §2.4 rule 3b. This handler is thin —
//! identity, usage tracking, flow events — all protocol logic lives in
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

use crate::mcp::identity::deny;
use crate::state::AppState;

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
///     (`coding_agent_integration_id` set) or an A2A agent the CLI discovered
///     on the user's machine (`discovered_on` set) — such a row is never
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
        ("traceparent" = Option<String>, Header, description = "W3C trace context naming the flow this call belongs to — required for `tools/call`, except for a CLI-bound local coding-agent row (`coding_agent_integration_id` set) or a discovered A2A agent (`discovered_on` set), which resolves to its owner when the flow lookup fails; otherwise the user identity is resolved from the flow record"),
        ("MCP-Protocol-Version" = Option<String>, Header, description = "Negotiated MCP protocol version; an unsupported value is rejected with 400 on implemented methods"),
    ),
    request_body(content = Object, description = "JSON-RPC 2.0 request: `tools/list` or `tools/call`"),
    responses(
        (status = 200, description = "JSON-RPC 2.0 response (result or error object)", body = Object),
        (status = 202, description = "Notification accepted (no `id` in request); empty object body", body = Object),
        (status = 400, description = "Unsupported or undecodable `MCP-Protocol-Version` on a method this gateway implements"),
        (status = 401, description = "Missing/unknown/revoked gateway token"),
        (status = 403, description = "`tools/call` outside a live flow the agent participates in (except a local coding-agent row or a discovered A2A agent, which resolves to its owner instead), or identity store unavailable"),
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
/// (`coding_agent_integration_id` set) or a discovered A2A agent
/// (`discovered_on` set, minted by `nasiko connect <agent>`): such a row has no
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
        ("traceparent" = Option<String>, Header, description = "W3C trace context naming the flow this call belongs to — required for `tools/call`, except for a CLI-bound local coding-agent row (`coding_agent_integration_id` set) or a discovered A2A agent (`discovered_on` set), which resolves to its owner when the flow lookup fails; otherwise the user identity is resolved from the flow record"),
        ("MCP-Protocol-Version" = Option<String>, Header, description = "Negotiated MCP protocol version; an unsupported value is rejected with 400 on implemented methods"),
    ),
    request_body(content = Object, description = "JSON-RPC 2.0 request: `tools/list` or `tools/call`"),
    responses(
        (status = 200, description = "JSON-RPC 2.0 response (result or error object)", body = Object),
        (status = 202, description = "Notification accepted (no `id` in request); empty object body", body = Object),
        (status = 400, description = "Unsupported or undecodable `MCP-Protocol-Version` on a method this gateway implements"),
        (status = 401, description = "Unknown/revoked gateway token"),
        (status = 403, description = "`tools/call` outside a live flow the agent participates in (except a local coding-agent row or a discovered A2A agent, which resolves to its owner instead), or identity store unavailable"),
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

    // `invoked_tool_name`, not `params.name`: a `nasiko_call_tool` call is
    // recorded — usage, metrics, the approval event below — as the inner
    // tool it ran, resolved by the same parse the protocol layer dispatches on.
    let tool_name = body
        .get("params")
        .map(protocol::invoked_tool_name)
        .unwrap_or("")
        .to_string();
    let header_traceparent = headers
        .get(nasiko_flow::TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok());

    // Two doors onto the same tail:
    //   - a coding-agent *session token* (NAS-839) carries {agent, user, session,
    //     flow} in signed claims, so it needs neither the ngt_ lookup nor a
    //     traceparent — the signature proves the user and agent, and the flow is
    //     the claim or a later turn's flow that `attributed_flow` verified (owned
    //     by that user, the agent a participant), so it is the verified flow for
    //     everything downstream;
    //   - otherwise the deploy-time ngt_ gateway token names the agent and the
    //     user is resolved from the request's traceparent + flow (the A2A path,
    //     docs/MCP_GATEWAY_AGENT_AUTH.md §2.4), with rule 3b for CLI-bound
    //     coding-agent rows.
    //
    // `verified_flow_id` travels alongside `user_id` (rather than being
    // re-derived from `traceparent` further down) because it's the one thing
    // this function actually proved — by signature plus the ownership and
    // participant check on the session path, or
    // against `flows`/`flow_participants` on the A2A path. The raw
    // `traceparent` a caller sends is not proof of anything on its own. On the
    // owner-fallback branch below, `user_id` did NOT come from a verified flow,
    // so `verified_flow_id` must be `None` even though `traceparent` may still
    // be a well-formed (just unresolvable, or rejected) value:
    // `protocol::handle_request` signs `verified_flow_id`, never `traceparent`,
    // into the identity header it forwards to system backends, specifically so
    // a caller can't launder an unverified trace id into a header a backend is
    // told to trust unconditionally.
    let (agent_id, user_id, verified_flow_id, synthesized_tp): (
        Uuid,
        Uuid,
        Option<String>,
        Option<String>,
    ) = match try_session_identity(state, token, header_traceparent).await {
        SessionResolution::Session {
            agent_id,
            user_id,
            flow_id,
        } => {
            let synthesized = synth_traceparent(&flow_id);
            (agent_id, user_id, Some(flow_id), Some(synthesized))
        }
        SessionResolution::Deny(resp) => return resp,
        SessionResolution::NotSession => {
            let agent_id = match nasiko_mcp_gateway::agent_tokens::authenticate(&state.db, token)
                .await
            {
                Ok(Some(id)) => id,
                Ok(None) => {
                    return (StatusCode::UNAUTHORIZED, "unknown or revoked gateway token")
                        .into_response();
                }
                // Rule 5: identity store unreachable → fail closed (403, not
                // a retryable 5xx that could be read as "try without auth"),
                // matching FlowGuard's GuardUnavailable posture.
                Err(e) => {
                    tracing::error!(error = %e, "mcp gateway: token lookup failed — failing closed");
                    return (StatusCode::FORBIDDEN, "identity store unavailable").into_response();
                }
            };

            // Resolve the user identity. `tools/call` (rules 3+4) requires the
            // traceparent to name a live flow this agent was dispatched into —
            // the flow's user is the authorization subject — except for a
            // CLI-bound local coding-agent row or a discovered A2A agent (rule
            // 3b, `agent_owner` below), which has no flow to require and
            // resolves to its owner instead.
            // Read-only methods (initialize/ping/tools/list, rule 2) work
            // agent-only: the flow user when one resolves, else the agent's
            // owner (startup-time tool discovery happens outside any flow).
            let (user_id, verified_flow_id) = match flow_user(state, header_traceparent, agent_id)
                .await
            {
                Ok((user_id, flow_id)) => (user_id, Some(flow_id)),
                Err(denial) => match agent_owner(state, agent_id).await {
                    // Deliberate policy (MCP_GATEWAY_AGENT_AUTH.md §5, rule
                    // 3b): a local coding agent — a row the CLI bound with
                    // `coding_agent_integration_id` — or an A2A agent the CLI
                    // discovered (`discovered_on`) is never dispatched
                    // through the proxy, so it never has a flow; it acts as
                    // its owner instead, even for `tools/call`. Same
                    // predicate the LLM router uses for the same rows
                    // (`oss/llm-router/src/resolver/mod.rs`,
                    // `coding_agent_integration_id IS NOT NULL` /
                    // `discovered_on IS NOT NULL`), so the
                    // gateway and the router agree on which rows count as
                    // "personal desks" — though not on how they're billed:
                    // the router always attributes these rows to their
                    // owner with no flow at all, while the gateway takes a
                    // verified flow first (above) and only reaches this
                    // fallback when one doesn't resolve. That difference is
                    // unreachable today: a coding-agent row is never
                    // dispatched into, so nothing ever has an endpoint of
                    // its own to route a flow to.
                    Ok(Some((owner, true))) => (owner, None),
                    // Every other flow-less `tools/call` stays denied
                    // (rules 3+4).
                    Ok(_) if method == "tools/call" => return denial,
                    // Read-only methods (rule 2): agent-only identity via
                    // the owner — unchanged from before the coding-agent
                    // policy existed.
                    Ok(Some((owner, false))) => (owner, None),
                    Ok(None) => {
                        return (
                            StatusCode::UNAUTHORIZED,
                            "agent no longer exists — gateway token is stale",
                        )
                            .into_response();
                    }
                    // Rule 5 again: an unreachable identity store is not
                    // "the agent is gone". Reporting 401 here would tell a
                    // healthy agent its credential is stale and trigger a
                    // pointless rotate/redeploy.
                    Err(e) => {
                        tracing::error!(error = %e, %agent_id, "mcp gateway: owner lookup failed — failing closed");
                        return (StatusCode::FORBIDDEN, "identity store unavailable")
                            .into_response();
                    }
                },
            };
            (agent_id, user_id, verified_flow_id, None)
        }
    };

    // The session path supplies a flow via its claims; the A2A path uses the
    // request's own header.
    let traceparent: Option<&str> = synthesized_tp.as_deref().or(header_traceparent);

    let started = std::time::Instant::now();

    // For a `tools/call`, wrap the execution in an exported `execute_tool` span
    // so the invocation surfaces in the trace timeline's Tool lane — the
    // server-side counterpart of the coding-agent path, which synthesises the
    // same span shape (`gen_ai.operation.name = "execute_tool"` + `tool.name`)
    // from its ACP turn report (`coding_agent_otlp.rs`). The MCP path never
    // emitted a span, so a tool call was recorded only as a `token_usage` row,
    // never in Tempo. Only a real tool invocation gets a span —
    // `tools/list`/`initialize`/`ping` do not. Model/provider are deliberately
    // left unset so the read-path classifier buckets it as a tool, not an LLM.
    // The span joins the caller's trace via the inbound traceparent (the same
    // one `handle_request` forwards), so it nests under the agent-call subtree
    // beside the LLM spans; with no traceparent (owner-fallback coding-agent
    // row) it is a local child instead.
    // Recording the tool's inputs/outputs on the span is gated by the same
    // content flag the hop and LLM spans honour, so a deployment that opts out
    // of content capture gets the Tool lane without the payloads.
    let capture_content = state.config.otel_capture_content;
    // One id for the exported span and the stored row, so the Tempo materializer's
    // `gen_ai.tool.call.id` and `tool_calls.call_id` name the same call.
    let call_id: String = Uuid::new_v4().to_string();
    let tool_span = (method == "tools/call").then(|| {
        let span = tracing::info_span!(
            "execute_tool",
            // The *exported* span name is tool-specific (`execute_tool <tool>`),
            // while `gen_ai.operation.name` stays the generic `execute_tool` the
            // Tool-lane classifier keys on. Retry/recovery detection
            // (`unrecovered_error_spans`, and the UI's `unrecoveredErrors`)
            // pairs an error span with a later success under the same parent by
            // *name*, so a single shared name would let a failed tool be hidden
            // by an unrelated tool that happened to succeed next. The
            // coding-agent OTLP path already names its tool spans this way
            // (`coding_agent_otlp.rs`).
            otel.name = %tool_span_name(&tool_name),
            otel.kind = "client",
            otel.status_code = tracing::field::Empty,
            gen_ai.operation.name = "execute_tool",
            gen_ai.agent.id = %agent_id,
            tool.name = %tool_name,
            gen_ai.tool.call.id = %call_id,
            tool.success = tracing::field::Empty,
            tool.arguments = tracing::field::Empty,
            tool.result = tracing::field::Empty,
            error.message = tracing::field::Empty,
        );
        // The arguments are known before the call, so record them now; the
        // read path reads `tool.arguments` as the span's input. Use the real
        // tool's arguments, unwrapping a `nasiko_call_tool` wrapper, to match
        // the `tool.name` recorded above.
        if capture_content
            && let Some(args) = body.get("params").map(protocol::invoked_tool_arguments)
        {
            span.record(
                "tool.arguments",
                crate::coding_agent_otlp::bounded_json(&args).as_str(),
            );
        }
        // Parent the tool span to the inbound trace ONLY when the flow is
        // proven. `verified_flow_id` is `Some` exactly when this call's trace was
        // verified — on the A2A path `flow_user` derived the trace id from this
        // very `traceparent` and checked it against `flows`/`flow_participants`,
        // and on the session path the signed claims did — so the header's trace
        // is the caller's own. On the owner-fallback path it is `None` (a coding
        // agent acting as its owner, or a foreign/dead trace id nobody proved);
        // the header is unverified, so the span — and the tool arguments/results
        // it may carry — must never join the trace it names.
        //
        // Detaching has to be explicit: the span is created inside the server's
        // `request` span, which `TraceLayer` already adopted into the raw inbound
        // `traceparent` (`oss/server/src/lib.rs`), so a contextual span inherits
        // that remote trace at export unless its parent is overridden. Setting an
        // empty context roots it in a trace of its own instead of a caller's.
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        span.set_parent(tool_span_parent(verified_flow_id.as_deref(), traceparent));
        span
    });

    let request = protocol::handle_request(
        &state.mcp,
        user_id,
        agent_id,
        &body,
        traceparent,
        verified_flow_id.as_deref(),
    );
    let result = match &tool_span {
        Some(span) => {
            use tracing::Instrument as _;
            request.instrument(span.clone()).await
        }
        None => request.await,
    };
    let Some(result) = result else {
        return (StatusCode::ACCEPTED, Json(json!({}))).into_response();
    };

    if method == "tools/call" {
        let latency_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
        let success = tool_call_succeeded(&result);
        if let Some(span) = &tool_span {
            span.record("tool.success", success);
            if !success {
                span.record("otel.status_code", "ERROR");
            }
            // The output (or the error) is known now; the read path reads
            // `tool.result`/`error.message` as the span's output. Mirrors the
            // coding-agent tool span's shape.
            if capture_content {
                if let Some(output) = result.get("result") {
                    span.record("tool.result", tool_result_for_span(output).as_str());
                }
                if let Some(error) = result.get("error") {
                    span.record(
                        "error.message",
                        crate::coding_agent_otlp::bounded_json(error).as_str(),
                    );
                }
            }
        }
        record_tool_usage(
            state,
            user_id,
            agent_id,
            &tool_name,
            latency_ms,
            success,
            verified_flow_id.as_deref(),
            &call_id,
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

// Rules 3–5 (resolve the flow named by `traceparent` to its user, requiring a
// live flow and recorded participation) live in `crate::mcp::identity`, which
// this handler shares with every other layer that has to make the same
// decision — see that module for why it is not a private helper here.
// Returns `(user_id, flow_id)`: `dispatch` forwards the flow id onward as
// `verified_flow_id`, the only form `protocol::handle_request` may sign into
// the identity header sent to system backends — never a raw, unverified
// `traceparent`.
use crate::mcp::identity::{bearer_token, flow_user};

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
/// The signature proves `{agent_id, user_id}`, so — unlike the A2A path — this needs no
/// live-flow freshness window (a long turn is never rejected for age). The DB touches are the
/// `jti` revocation check, so a token can still be killed before its `exp`, and the flow
/// attribution: a later turn's flow (the traceparent's, else the session's newest) is used only
/// when that user owns it and the agent is a recorded participant, else the claim.
async fn try_session_identity(
    state: &AppState,
    token: &str,
    traceparent: Option<&str>,
) -> SessionResolution {
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
        Ok(false) => {
            let trace_flow = traceparent
                .and_then(nasiko_flow::FlowContext::from_traceparent)
                .map(|ctx| ctx.flow_id);
            let attributed = nasiko_session_token::attribution::attributed_flow(
                &state.db,
                trace_flow.as_deref(),
                &claims,
            )
            .await;
            SessionResolution::Session {
                agent_id,
                user_id,
                flow_id: attributed.flow_id,
            }
        }
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

/// `(owner_id, is_local)` for a live (non-deleted) agent — one query
/// where there used to be two (a plain `agent_owner` for the read-only
/// fallback, a separate `coding_agent_owner` for rule 3b), because the
/// `Err(denial)` arm above always needs both facts about the very same row at
/// once. `is_local` is `coding_agent_integration_id IS NOT NULL`
/// (`oss/migrations/0019_coding_agent_identity.sql`) or `discovered_on IS NOT
/// NULL` (`oss/migrations/0110_discovered_agents.sql`) — deliberately the
/// columns themselves, never agent metadata or name — matching
/// `is_coding_agent` and `is_discovered_agent` in
/// `oss/llm-router/src/resolver/mod.rs`, so the gateway and the LLM router
/// agree on which rows this policy covers. Such a row is single-owner by
/// construction (the CLI only binds or registers rows the login owns;
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
        "SELECT owner_id,
                coding_agent_integration_id IS NOT NULL OR discovered_on IS NOT NULL
         FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
}

/// The largest tool payload recorded onto an `execute_tool` span. Well above the
/// 2,000-char generic content cap (`coding_agent_otlp::bounded_json`) because the
/// trace view only offers a JSON view when the *whole* value parses — a payload
/// truncated mid-structure drops back to raw, unreadable text. Still bounded so
/// one tool call cannot bloat the trace.
const MAX_TOOL_CONTENT_CHARS: usize = 16_000;

/// Render an MCP tool result for the span's `tool.result`. MCP wraps a tool's
/// output in a `{ "content": [{ "type": "text", "text": … }] }` envelope, and
/// the text is itself usually a JSON document — so recorded verbatim it reaches
/// the trace view double-escaped (`{\"messages\":…}`) and, once truncated, no
/// longer parses, which is why the Output pane shows raw text with no JSON view.
/// Unwrap the text parts and parse them when they are JSON, so the real output
/// is stored as structured JSON; non-text shapes (image parts, structured
/// content) fall back to the result as-is. Bounded to `MAX_TOOL_CONTENT_CHARS`.
fn tool_result_for_span(result: &Value) -> String {
    let unwrapped = unwrap_tool_text(result);
    let value = unwrapped.as_ref().unwrap_or(result);
    crate::coding_agent_otlp::truncate_chars(
        &serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
        MAX_TOOL_CONTENT_CHARS,
    )
}

/// The text payload of a CallToolResult: its `text` content parts concatenated,
/// parsed as JSON when the whole thing parses, else kept as a plain string.
/// `None` when the result carries no text parts, so the caller keeps the raw
/// result.
fn unwrap_tool_text(result: &Value) -> Option<Value> {
    let parts = result.get("content")?.as_array()?;
    let text: String = parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    if text.is_empty() {
        return None;
    }
    Some(serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)))
}

/// The OpenTelemetry parent context for a tool call's `execute_tool` span.
///
/// Returns the caller's remote trace context **only** when the flow was proven
/// (`verified_flow_id` is `Some`); otherwise an empty context, which roots the
/// span in its own trace. An unverified inbound `traceparent` — the owner-
/// fallback path, where it may name another user's flow — is never adopted, so
/// the tool span and its captured payloads cannot be stapled onto a trace the
/// caller did not prove it belongs to. Returning an empty context (rather than
/// leaving the span contextual) is what detaches it from the server `request`
/// span that `TraceLayer` already parented to the raw header.
fn tool_span_parent(
    verified_flow_id: Option<&str>,
    traceparent: Option<&str>,
) -> opentelemetry::Context {
    if verified_flow_id.is_some()
        && let Some(cx) = traceparent.and_then(crate::telemetry::remote_context_from_traceparent)
    {
        return cx;
    }
    opentelemetry::Context::new()
}

/// The exported name for a tool call's `execute_tool` span.
///
/// Tool-specific (`execute_tool gmail__send_email`) so retry/recovery detection,
/// which pairs an error span with a later same-named success under the same
/// parent, cannot treat an unrelated tool that succeeded next as a recovery of a
/// tool that failed. Mirrors the coding-agent OTLP convention
/// (`coding_agent_otlp.rs`). Falls back to the bare `execute_tool` when the tool
/// name is missing, rather than leaving a trailing space.
fn tool_span_name(tool_name: &str) -> String {
    if tool_name.is_empty() {
        "execute_tool".to_string()
    } else {
        format!("execute_tool {tool_name}")
    }
}

/// Whether a `tools/call` JSON-RPC response reports success.
///
/// A tool call can fail in two independent shapes, and both must count as a
/// failure or a failed call is exported and billed as a success:
///
/// - a top-level JSON-RPC `error` object — a transport/protocol failure the
///   gateway itself produced, and
/// - a tool-level failure, which MCP returns as a *successful* JSON-RPC
///   response whose `result.isError` is `true` (the tool ran and reported its
///   own error — the shape connector tools and the recovery-tool path use).
///
/// Checking only `error` silently loses the second kind, so the span records
/// `tool.success = true` and the usage row `finish_reason = "ok"` for a call
/// that actually failed.
fn tool_call_succeeded(response: &Value) -> bool {
    response.get("error").is_none()
        && response
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(Value::as_bool)
            != Some(true)
}

// One value per field of the stored call; a params struct would only repeat them.
#[allow(clippy::too_many_arguments)]
fn record_tool_usage(
    state: &AppState,
    user_id: Uuid,
    agent_id: Uuid,
    tool_name: &str,
    latency_ms: i32,
    success: bool,
    flow_id: Option<&str>,
    call_id: &str,
) {
    state.genai_metrics.record_tool_call(
        latency_ms as f64 / 1000.0,
        tool_name,
        &agent_id.to_string(),
    );

    let call = crate::usage::ToolCallRecord {
        call_id: call_id.to_string(),
        user_id,
        agent_id,
        tool_name: tool_name.to_string(),
        success,
        latency_ms,
        metadata: json!({ "tool": tool_name, "success": success, "team_id": null, "flow_id": flow_id }),
        verified_flow_id: flow_id.map(str::to_string),
    };

    let tracker = state.usage_tracker.clone();
    tokio::spawn(async move {
        if let Err(e) = tracker.track_tool_call(call).await {
            tracing::warn!(error = %e, "failed to record mcp_tool_call usage");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_result_unwraps_the_text_envelope_and_parses_inner_json() {
        // MCP wraps the real JSON payload as a string inside a text content part.
        let result = json!({
            "content": [{ "type": "text", "text": "{\"messages\":[{\"id\":\"1\"}]}" }],
            "isError": false
        });
        let rendered = tool_result_for_span(&result);
        // Stored un-escaped and parseable, so the trace view offers a JSON view.
        let parsed: Value = serde_json::from_str(&rendered).expect("parses as JSON");
        assert_eq!(parsed, json!({ "messages": [{ "id": "1" }] }));
    }

    #[test]
    fn tool_result_keeps_plain_text_when_the_part_is_not_json() {
        let result = json!({ "content": [{ "type": "text", "text": "done" }] });
        let rendered = tool_result_for_span(&result);
        assert_eq!(
            serde_json::from_str::<Value>(&rendered).unwrap(),
            json!("done")
        );
    }

    #[test]
    fn tool_result_falls_back_to_the_raw_result_without_text_parts() {
        // No text parts (e.g. an image part or structured content) → raw result.
        let result = json!({ "content": [{ "type": "image", "data": "…" }] });
        let rendered = tool_result_for_span(&result);
        assert_eq!(serde_json::from_str::<Value>(&rendered).unwrap(), result);
    }

    #[test]
    fn tool_result_is_bounded() {
        let big = "x".repeat(MAX_TOOL_CONTENT_CHARS * 2);
        let result = json!({ "content": [{ "type": "text", "text": big }] });
        assert!(tool_result_for_span(&result).chars().count() <= MAX_TOOL_CONTENT_CHARS);
    }

    const SAMPLE_TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    const SAMPLE_TRACE_ID: &str = "0af7651916cd43dd8448eb211c80319c";

    #[test]
    fn verified_flow_parents_the_tool_span_to_the_inbound_trace() {
        use opentelemetry::trace::TraceContextExt as _;
        let cx = tool_span_parent(Some(SAMPLE_TRACE_ID), Some(SAMPLE_TRACEPARENT));
        let sc = cx.span().span_context().clone();
        assert!(sc.is_valid(), "a verified call joins the caller's trace");
        assert_eq!(sc.trace_id().to_string(), SAMPLE_TRACE_ID);
    }

    #[test]
    fn unverified_traceparent_is_not_adopted_as_the_tool_span_parent() {
        use opentelemetry::trace::TraceContextExt as _;
        // Owner-fallback: no proven flow, though the header names a (foreign)
        // trace. The span must detach to its own root, never join that trace.
        let cx = tool_span_parent(None, Some(SAMPLE_TRACEPARENT));
        assert!(
            !cx.span().span_context().is_valid(),
            "an unverified trace is never adopted as the parent"
        );
    }

    #[test]
    fn verified_flow_with_an_unusable_traceparent_detaches() {
        use opentelemetry::trace::TraceContextExt as _;
        // Defensive: proven flow but the header will not parse → own root, not a
        // panic and not an adopted garbage context.
        let cx = tool_span_parent(Some("flow-1"), Some("not-a-traceparent"));
        assert!(!cx.span().span_context().is_valid());
        // And with no header at all.
        let cx = tool_span_parent(Some("flow-1"), None);
        assert!(!cx.span().span_context().is_valid());
    }

    #[test]
    fn tool_span_name_is_tool_specific() {
        // Distinct tools get distinct exported span names, which is what keeps
        // one tool's failure from being hidden by another's success in
        // retry/recovery detection.
        assert_eq!(
            tool_span_name("gmail__send_email"),
            "execute_tool gmail__send_email"
        );
        assert_ne!(
            tool_span_name("gmail__send_email"),
            tool_span_name("calendar__list_events")
        );
        // Prefix preserved so the name still reads as a tool execution.
        assert!(tool_span_name("x").starts_with("execute_tool "));
        // No trailing space when the tool name is missing.
        assert_eq!(tool_span_name(""), "execute_tool");
    }

    #[test]
    fn tool_level_iserror_is_a_failure_despite_no_jsonrpc_error() {
        // MCP tool-level failure: a successful JSON-RPC response whose result
        // carries `isError: true`. Checking only `error` would call this a
        // success and export/record it as one.
        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "quota exceeded" }], "isError": true }
        });
        assert!(!tool_call_succeeded(&response));
    }

    #[test]
    fn top_level_jsonrpc_error_is_a_failure() {
        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32000, "message": "boom" }
        });
        assert!(!tool_call_succeeded(&response));
    }

    #[test]
    fn clean_result_is_a_success() {
        // `isError: false` and no error → success.
        let explicit = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "ok" }], "isError": false }
        });
        assert!(tool_call_succeeded(&explicit));
        // A result with no `isError` field at all is also a success.
        let implicit = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "content": [{ "type": "text", "text": "ok" }] }
        });
        assert!(tool_call_succeeded(&implicit));
    }
}
