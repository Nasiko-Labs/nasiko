use axum::{
    Json,
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
};
use futures::StreamExt;
use serde::Serialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use utoipa::ToSchema;
use uuid::Uuid;

use nasiko_react_agent::{
    AgentInfo, AgentSkill as OrcAgentSkill, Orchestrator, OrchestratorConfig, OrchestratorEvent,
    RegistrySource,
};
use nasiko_types::a2a::{self as a2a, JsonRpcRequest, PartContent, StreamResponse};

use nasiko_orchestrator::{AgentSelector, ContextTiers, context_selection};

use nasiko_flow::FlowContext;
use nasiko_llm_router::attachments::{
    AttachmentRef, MARKER_TTL_SECS, MarkerClaims, MarkerSigner, describe_file,
};

use crate::acl::CpCallGuard;
use crate::agent_resolver::{Caller, resolve_loaded};
use crate::agent_wake::AgentWake;
use crate::auth::Claims;
use crate::chat::turn_artifacts::Attachable;
use crate::orchestrator_policy::TurnKind;
use crate::sessions::admission::refuse_session;
use crate::state::AppState;
use crate::telemetry::agent_request::RequestOutcome;

/// Doc-only stand-in for the real request type (`nasiko_types::a2a::JsonRpcRequest`,
/// re-exported from the external `a2a-lf` crate, which has no `ToSchema` impl and
/// can't be given one here). Mirrors its actual JSON-RPC 2.0 shape: `method` is
/// `"message/send"` or `"message/stream"`; `params` is a `SendMessageRequest` —
/// see `oss/docs/A2A_PROTOCOL.md` for the full parts/metadata schema.
/// `metadata.agent_id` selects the dispatch target: absent or `"orchestrator"`
/// routes through the routing engine/ReAct orchestrator, anything else proxies
/// directly to that agent (UUID or name).
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct A2aJsonRpcRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    params: serde_json::Value,
}

/// Doc-only stand-in for the JSON-RPC error envelope `A2aDispatchError` renders.
#[derive(Serialize, ToSchema)]
pub(crate) struct JsonRpcErrorBody {
    code: i32,
    message: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct JsonRpcErrorResponse {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    error: JsonRpcErrorBody,
}

/// Multipart form for `POST /api/orchestrator/a2a/upload` — a required `query`
/// text field, plus any number of additional fields treated as file attachments
/// (each base64-encoded and forwarded to the orchestrator alongside the query).
#[derive(ToSchema)]
#[allow(dead_code)]
pub(crate) struct A2aUploadForm {
    query: String,
}

// TODO: Implement `AgentExecutor` trait from a2a-server-lf to replace manual Axum routing.
// The trait has two methods: execute(&self, ctx: ExecutorContext) -> BoxStream<StreamResponse>
// and cancel(&self, ctx: ExecutorContext) -> BoxStream<StreamResponse>.
// Our handler already returns a stream of StreamResponse — wrap it in the trait impl and let
// a2a-server handle JSON-RPC envelope, SSE serialization, and /.well-known/agent-card.json.

/// TEMP DEBUG: log every inbound header (redacting `authorization`/`cookie`) so we can
/// audit which conversation/trace identifiers arrive natively. Remove after the audit.
pub(crate) fn log_inbound_headers(entry: &str, headers: &HeaderMap) {
    let dump: Vec<String> = headers
        .iter()
        .map(|(name, value)| {
            let n = name.as_str();
            // The gateway's signed `x-nasiko-identity` header (`oss/mcp-gateway/
            // src/identity.rs`) is redacted alongside the other credential-bearing
            // headers — this logger isn't currently wired to the workspace route
            // that header is forwarded to, but a header carrying a caller's
            // identity must never be logged in plain text if it ever is.
            let v = if n.eq_ignore_ascii_case("authorization")
                || n.eq_ignore_ascii_case("cookie")
                || n.eq_ignore_ascii_case(nasiko_mcp_gateway::identity::IDENTITY_HEADER)
            {
                "<redacted>"
            } else {
                value.to_str().unwrap_or("<non-utf8>")
            };
            format!("{n}={v}")
        })
        .collect();
    tracing::info!(
        target: "nasiko::header_audit",
        entry,
        headers = %dump.join("  |  "),
        "inbound request headers"
    );
}

/// Server-side A2A dispatch endpoint. Accepts JSONRPC `message/send` or `message/stream`.
/// Dispatches to the routing engine (no agent_id), ReAct orchestrator (agent_id=orchestrator),
/// or a specific agent directly.
///
/// No gateway required: the server validates the JWT and enforces authorization itself
/// (see `require_auth`/`Claims`). This handler is the production A2A path.
#[utoipa::path(
    post,
    path = "/api/orchestrator/a2a",
    tag = "orchestrator",
    request_body = A2aJsonRpcRequest,
    responses(
        (status = 200, description = "A2A event stream (status/artifact updates, completion)", content_type = "text/event-stream"),
        (status = 400, description = "Missing/invalid params, or an empty message", body = JsonRpcErrorResponse),
        (status = 403, description = "Caller cannot access the targeted agent"),
        (status = 404, description = "Target agent not found or not running", body = JsonRpcErrorResponse),
        (status = 500, description = "Internal error", body = JsonRpcErrorResponse),
        (status = 503, description = "No agents available for routing-engine dispatch", body = JsonRpcErrorResponse),
    ),
)]
pub async fn a2a_dispatch_handler(
    State(state): State<AppState>,
    claims: Claims,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> Result<Response, A2aDispatchError> {
    // TEMP DEBUG: dump the inbound request headers so we can see what identifiers
    // (traceparent/trace_id, session_id, flow_id, x-nasiko-*) arrive natively vs.
    // what we mint. Remove once the header audit is done.
    log_inbound_headers("a2a_dispatch (orchestrator)", &headers);

    // A harness session's task methods carry a task id, not a message, so they are answered
    // before the message parsing below.
    if let Some(method) = crate::sessions::tasks::task_method(&req.method)
        && let Some(reply) = harness_task_reply(&state, &claims, method, &req).await?
    {
        return Ok(reply);
    }

    let params: nasiko_types::a2a::SendMessageRequest = serde_json::from_value(
        req.params
            .clone()
            .ok_or_else(|| A2aDispatchError::InvalidRequest("missing params".into()))?,
    )
    .map_err(|e| A2aDispatchError::InvalidRequest(format!("bad params: {e}")))?;

    // Reconnect to an already-resumed execution's real A2A/SSE events — checked before the
    // text-emptiness validation below, since a reconnect carries no new user message at all.
    // `hitl_id` is the id the frontend already holds (whatever it just POSTed to
    // `/api/hitl/{id}/resolve`); this never invokes the agent — it only attaches to the
    // continuation buffer `deliver()` (the existing, unchanged, browser-decoupled resume
    // dispatcher) is already writing real agent events into. See `oss/server/src/hitl/
    // continuation.rs` for the full mechanism and why a late reconnect still gets everything.
    if let Some(hitl_id) = params
        .metadata
        .as_ref()
        .and_then(|m| m.get("reconnect_after_hitl_id"))
        .and_then(|v| v.as_str())
    {
        let hitl_id = Uuid::parse_str(hitl_id).map_err(|_| {
            A2aDispatchError::InvalidRequest("reconnect_after_hitl_id is not a valid UUID".into())
        })?;
        let user_id = match claims.user_uuid() {
            Ok(id) => id,
            Err(e) => return Ok(e.into_response()),
        };
        return reconnect_stream(&state, hitl_id, user_id, claims.is_superuser).await;
    }

    let text = params
        .message
        .parts
        .iter()
        .filter_map(|p| match &p.content {
            PartContent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    if text.is_empty() {
        return Err(A2aDispatchError::InvalidRequest(
            "message must contain at least one text part".into(),
        ));
    }

    let task_id = Uuid::new_v4().to_string();
    let context_id = params
        .message
        .context_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let agent_id = params
        .metadata
        .as_ref()
        .and_then(|m| m.get("agent_id"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let session_id = params
        .metadata
        .as_ref()
        .and_then(|m| m.get("session_id"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let is_orchestrator = agent_id.is_none() || agent_id.as_deref() == Some("orchestrator");

    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return Ok(e.into_response()),
    };

    // What the caller attached to this turn. Both dispatch branches below carry it: a routed
    // turn hands it to every agent the orchestrator calls, a direct one puts it in the A2A
    // message. Resolved before either, because an attachment belongs to the turn, not to the
    // target that happens to answer it.
    let mut attachments =
        inbound_file_parts(&state, &params, session_id.as_deref(), user_id).await?;

    // Resolved before the history fetch: the target's switch decides IP-4.
    let target_agent = if is_orchestrator {
        None
    } else {
        let target = agent_id.as_deref().unwrap();
        // Resolve the target (UUID or name) to a concrete agent row FIRST, then
        // authorize on the resolved id unconditionally. Gating the authz check
        // behind "does target happen to parse as a UUID" let any name-addressed
        // request (the common case from the UI/CLI) skip the check entirely.
        let agent = resolve_agent(&state, target).await?;
        // Edition-aware view access (superuser short-circuit lives inside the check).
        //
        // Deliberately returns the SAME AgentNotFound response as "no such agent"
        // rather than Forbidden — otherwise a caller could distinguish "exists,
        // but you can't access it" from "doesn't exist", enabling agent-name
        // enumeration by a non-grantee.
        if !crate::acl::can_access_agent(&state, &claims, agent.id).await {
            return Err(A2aDispatchError::AgentNotFound(target.to_string()));
        }
        Some(agent)
    };
    // The caller picks `contextId` and `metadata.session_id`, and both are used to read history
    // and to append messages. A session that already belongs to someone else is answered exactly
    // like a missing agent would be, so ids cannot be probed; superusers keep their access.
    for sid in std::iter::once(context_id.as_str()).chain(session_id.as_deref()) {
        if session_owned_by_other(&state, sid, user_id, claims.is_superuser).await? {
            return Err(A2aDispatchError::Forbidden("session not found".into()));
        }
    }
    // Whoever reads this history decides whether it is compressed: the addressed agent on a direct
    // chat, nobody on an orchestrator-routed one (the Orchestrator's own model reads it, and what
    // reaches an agent afterwards is what that model wrote). Read off the row already in hand
    // rather than re-queried.
    let compress_history = target_agent.as_ref().is_none_or(|a| a.compress_enabled);

    // Code-driving agents (ACP coding agents, harness sessions) have no HTTP endpoint. A harness
    // session keeps its own conversation in its sandbox, so it is resolved here and skips the
    // history fetch and packing below.
    let code_target = match &target_agent {
        Some(agent) if agent.may_drive_code() => {
            crate::coding::proxy::load_agent_target(&state.db, agent.id)
                .await
                .map_err(|e| A2aDispatchError::Internal(e.to_string()))?
        }
        _ => None,
    };
    let code_target = match (code_target, &target_agent) {
        (Some(crate::coding::proxy::AgentTarget::Harness(target)), Some(agent)) => {
            let turn = CodingDispatch {
                agent,
                query: &text,
                user_message: &text,
                context_id: &context_id,
                task_id: &task_id,
                user_id,
            };
            return harness_agent_stream(&state, target, turn, session_id.is_some(), &req.id).await;
        }
        (other, _) => other,
    };

    // Prefer the explicit metadata.session_id (web UI), fall back to the
    // message contextId — the CLI reuses its CP session id as contextId, so
    // multi-turn chats keep their history either way. An unknown id simply
    // fetches zero rows.
    let history_sid = session_id.as_deref().unwrap_or(&context_id);
    // The history below is text: a file sent on an earlier turn rides along with this one, or an
    // agent without a memory of its own answers a follow-up about it without the file.
    carry_earlier_attachments(&state, history_sid, user_id, &params, &mut attachments).await;
    let InboundAttachments {
        parts: file_parts,
        files: attached_files,
    } = attachments;
    let history_store = state.history_vector_store(user_id).await;
    let history = context_selection::fetch_for_user(
        &state.db,
        user_id,
        history_sid,
        &history_store,
        &text,
        &ContextTiers::from_config(&state.config)
            .with_overrides(&state.db)
            .await,
        compress_history,
    )
    .await;

    let query = history.with_current_query(&text);

    if let Some(agent) = target_agent {
        // Coding agents have no HTTP endpoint — they're driven over an ACP stdio
        // turn, not reverse-proxied. Detect and run that path instead of falling
        // through to `agent_stream`, which would try to resolve a (nonexistent)
        // endpoint and 500 with "no endpoint found" (NAS-C1). This is what makes
        // the web UI (which always dispatches here) able to chat coding agents.
        if let Some(crate::coding::proxy::AgentTarget::Coding(target)) = code_target {
            let turn = CodingDispatch {
                agent: &agent,
                query: &query,
                user_message: &text,
                context_id: &context_id,
                task_id: &task_id,
                user_id,
            };
            return coding_agent_stream(&state, target, turn, session_id.is_some()).await;
        }
        // Supplemental per-agent context (L1A) is no longer injected here: it now rides the EE
        // injection proxy, which sits on every outbound forward `forwarding_endpoint()` resolves
        // to — including this one — so injecting it a second time in-process here would double
        // it up. The planner preamble (`context_for_agents`, the routed path below) is the only
        // L1A injection that still happens in-process, because it folds into the LLM's own prompt
        // rather than an outbound forward.
        //
        // Organization policy (L0) is the one thing still delivered in-process on this path. It is
        // appended as its own message part inside `agent_stream` (see `append_org_policy_part`),
        // so it lands ahead of anything the injection proxy appends afterwards: org policy goes
        // before the agent's own knowledge, which goes before the conversation — the ordering IS
        // the precedence, since nothing downstream re-ranks these against each other.
        let org_context = state.prompt_context.org_context().await;
        agent_stream(
            &state,
            agent,
            &query,
            &text,
            org_context.as_deref(),
            &task_id,
            &context_id,
            user_id,
            &file_parts,
            &attached_files,
            session_id,
            history.user_turn_count(),
            Some(&history.report),
        )
        .await
    } else {
        orchestrator_stream(
            &state,
            OrchestratorTurn {
                query: &query,
                raw_text: &text,
                task_id: &task_id,
                context_id: &context_id,
                user_id,
                is_superuser: claims.is_superuser,
                client_owns_transcript: session_id.is_some(),
                transcript_role: "user",
                file_parts,
                attached_files,
                kind: TurnKind::User,
                context_report: Some(&history.report),
            },
        )
        .await
    }
}

/// Attaches to an already-resumed execution's real A2A/SSE events through the same
/// `POST /api/orchestrator/a2a` endpoint the initial turn used — not a new frontend-facing
/// route. Never calls the agent: it only replays-then-live-tails the continuation buffer
/// `hitl/mod.rs::deliver()` (the existing, browser-decoupled resume dispatcher) is already
/// writing the agent's real events into, keyed by the exact `hitl_id` the frontend already holds.
/// A reconnect that arrives after the resume already finished still gets the complete sequence —
/// see `oss/server/src/hitl/continuation.rs` for why there's no "missed it" window.
async fn reconnect_stream(
    state: &AppState,
    hitl_id: Uuid,
    user_id: Uuid,
    is_superuser: bool,
) -> Result<Response, A2aDispatchError> {
    let row = state
        .hitl_store
        .get(hitl_id)
        .await
        .map_err(|e| A2aDispatchError::Internal(e.to_string()))?
        .ok_or_else(|| {
            A2aDispatchError::InvalidRequest("no such HITL request to reconnect to".into())
        })?;

    // Authorize before any row-state check below can leak a distinguishable error message for a
    // row this caller doesn't own — origin, pending-vs-resolved, and continuation-existence were
    // all checked before this, so a caller could enumerate arbitrary UUIDs and learn another
    // user's HITL row's origin and lifecycle state purely from which error came back, without
    // ever passing authorization (found in review). Ownership can't change any of these rows'
    // state, so checking it first changes no legitimate caller's outcome.
    let identity = nasiko_hitl::HitlIdentity {
        user_id,
        is_superuser,
    };
    nasiko_hitl::authorize_hitl_action(&identity, &row, nasiko_hitl::HitlAction::View).map_err(
        |_| A2aDispatchError::Forbidden("not authorized to reconnect to this execution".into()),
    )?;

    // `deliver_maf()` (`hitl/mod.rs`) hands off to the MAF worker over Redis and never touches
    // `continuation_events` at all — a MAF-origin row here would otherwise `watch()` a buffer
    // nothing ever appends to or terminates, hanging the connection forever with no error. Reject
    // cleanly instead; MAF continuation isn't wired up yet (§13.2) — the poll-based discovery
    // surfaces still apply.
    if row.origin == nasiko_hitl::HitlOrigin::Maf {
        return Err(A2aDispatchError::InvalidRequest(
            "reconnect is not available for MAF-origin executions — poll GET /api/maf/execution/{id} instead"
                .into(),
        ));
    }

    // Still pending — nothing has been dispatched for delivery at all, so no buffer exists or
    // ever will until a human resolves this row. `watch()` would otherwise conjure up a fresh,
    // permanently-non-terminal buffer for it.
    if row.status == nasiko_hitl::HitlStatus::Pending {
        return Err(A2aDispatchError::InvalidRequest(
            "this HITL request has not been resolved yet — nothing to reconnect to".into(),
        ));
    }

    // An `mcp_tool` row only ever gets linked to a continuation buffer via
    // `ContinuationRegistry::alias` (`router/hitl.rs::auto_resolve_linked_direct_chat_row`), which
    // only runs when a direct_chat/agent_proxy/maf/orchestrator mirror actually exists — the
    // dedicated `mcp_tool` resume dispatcher (`oss/hitl::dispatcher`) never touches
    // `continuation_events` at all. A standalone, never-mirrored `mcp_tool` row's id would
    // otherwise `watch()` the same kind of buffer nothing will ever terminate.
    if row.origin == nasiko_hitl::HitlOrigin::McpTool && !state.continuation_events.exists(row.id) {
        return Err(A2aDispatchError::InvalidRequest(
            "this HITL request has no agent-visible continuation to reconnect to — poll GET /api/hitl/pending instead"
                .into(),
        ));
    }

    // A real `mcp_tool` row's id is aliased onto its mirror's buffer by
    // `auto_resolve_linked_direct_chat_row` (`router/hitl.rs`,
    // `ContinuationRegistry::alias`) at resolve time, so `row.id` resolves to the right buffer
    // either way — no separate lookup needed here.
    let events = state.continuation_events.watch(row.id);
    let stream = async_stream::stream! {
        futures::pin_mut!(events);
        while let Some(data) = events.next().await {
            yield Ok::<_, Infallible>(Event::default().data(data));
        }
    };
    Ok(Sse::new(stream).into_response())
}

// ─── Orchestrator Path ───────────────────────────────────────────────────────

/// The `chat_messages.role` for a row the platform wrote rather than a person — right now only
/// the HITL resume's continuation. `list_messages` filters it out of the transcript, so it never
/// reaches a chat bubble, while `SessionHistory::fetch` (which reads the table directly) still
/// carries it into the next turn's reasoning.
pub(crate) const INTERNAL_TRANSCRIPT_ROLE: &str = "system";

/// One orchestrator-routed turn. Grouped into a struct because the caller
/// count crossed clippy's argument threshold, and these travel together.
///
/// `pub(crate)` (not module-private): also constructed by `crate::hitl`'s resume dispatcher to
/// trigger a fresh orchestrator turn after a paused sub-agent call resumes (Step 7).
pub(crate) struct OrchestratorTurn<'a> {
    /// The prompt actually sent downstream: history + `raw_text`.
    pub(crate) query: &'a str,
    /// What the caller typed, before history enrichment — this is what gets
    /// persisted, so the next turn doesn't nest an already-glued blob.
    pub(crate) raw_text: &'a str,
    pub(crate) task_id: &'a str,
    pub(crate) context_id: &'a str,
    pub(crate) user_id: Uuid,
    pub(crate) is_superuser: bool,
    /// Caller persists its own turns (web UI) — the server must not also.
    pub(crate) client_owns_transcript: bool,
    /// The `chat_messages.role` `raw_text` is stored under. `"user"` for a real turn; the HITL
    /// resume passes `"system"`, because its `raw_text` is a continuation the platform wrote
    /// ("The archive agent replied: …") and not something the human said. Stored as `"user"` it
    /// drew a user bubble in the transcript quoting the sub-agent back at them, and fed the next
    /// turn a fake user line. It is still persisted — the resumed step has to stay in the
    /// session's own history, especially when the turn re-pauses and no assistant reply follows
    /// — just under a role the transcript does not show.
    pub(crate) transcript_role: &'a str,
    /// File parts uploaded with the request (multipart upload path).
    pub(crate) file_parts: Vec<nasiko_types::a2a::Part>,
    pub(crate) attached_files: Vec<AttachedFile>,
    /// Whether this turn was started by the user or reports on work an earlier
    /// turn already did. The operator's policy may treat the two differently —
    /// see [`TurnKind`].
    pub(crate) kind: TurnKind,
    /// What history selection chose from and kept for this turn. Carried rather than recorded at
    /// the call site because the request span it belongs on is created inside the stream function,
    /// several hundred lines after selection ran.
    pub(crate) context_report: Option<&'a nasiko_orchestrator::ContextReport>,
}

/// Assembles the routed path's preamble in strict precedence order — L0 (org policy) outranks
/// L1A (per-agent facts) outranks L2 (per-user recall) — by construction: each layer is placed
/// ahead of the ones that follow it in the text, so nothing downstream has to re-rank them.
/// `None` at every layer (the common case: OSS, or an EE deployment with nothing configured or
/// recalled yet) must return `None`, not an empty string, so `OrchestratorConfig.preamble` stays
/// exactly what it was before any of these three seams existed.
fn build_preamble(
    org_context: Option<&str>,
    agent_facts: Option<&str>,
    user_context: Option<&str>,
) -> Option<String> {
    let user_section = user_context.map(|user| format!("## What you know about this user\n{user}"));
    let agent_and_user = match (agent_facts, user_section.as_deref()) {
        (None, None) => None,
        (Some(facts), None) => Some(facts.to_string()),
        (None, Some(user)) => Some(user.to_string()),
        (Some(facts), Some(user)) => Some(crate::prompt_context::prefixed(Some(facts), user)),
    };
    match (org_context, agent_and_user) {
        (None, facts) => facts,
        (Some(org), Some(facts)) => Some(crate::prompt_context::prefixed(Some(org), &facts)),
        (Some(org), None) => Some(org.to_string()),
    }
}

pub(crate) async fn orchestrator_stream(
    state: &AppState,
    turn: OrchestratorTurn<'_>,
) -> Result<Response, A2aDispatchError> {
    let OrchestratorTurn {
        query,
        raw_text,
        task_id,
        context_id,
        user_id,
        is_superuser,
        client_owns_transcript,
        transcript_role,
        file_parts,
        attached_files,
        kind,
        context_report,
    } = turn;
    // Orchestrator-routed chats never had a `chat_sessions` row, unlike
    // `agent_proxy.rs`'s `ensure_chat_session` for direct agent chat — so
    // `nasiko sessions`/`history` couldn't find them and `--session-id`
    // resume had no history to actually resume. `agent_id` is NULL here
    // (unlike agent_proxy's fixed target) since the orchestrator can route
    // to a different agent on every turn of the same session.
    //
    // Persist `raw_text` (what the caller actually typed), never `query`
    // (history + raw_text already glued together by `with_current_query` at
    // the call site) — storing the enriched blob here would nest: next
    // turn's fetch would read this already-history-laden row back out and
    // glue *another* copy of it in front of the next message, compounding
    // turn over turn instead of growing linearly with real conversation.
    ensure_orchestrator_chat_session(
        state,
        context_id,
        user_id,
        raw_text,
        transcript_role,
        client_owns_transcript,
    )
    .await;

    let all_agents = AgentSelector::fetch_active_agents(&state.db)
        .await
        .map_err(|e| A2aDispatchError::Internal(e.to_string()))?;

    // Filter to agents the requesting user can access.
    let agent_summaries = if is_superuser {
        all_agents
    } else {
        // Edition-aware access filter. This branch is non-superuser only, so build a
        // minimal identity (username unused by can_access_agent) and delegate to the
        // trait — honoring OSS user-grants and EE team/dept grants alike.
        let identity = nasiko_auth::Identity {
            user_id: user_id.to_string(),
            username: String::new(),
            is_superuser: false,
        };
        let mut accessible = Vec::new();
        for summary in all_agents {
            if state
                .auth
                .can_access_agent(&identity, &summary.id.to_string())
                .await
            {
                accessible.push(summary);
            }
        }
        accessible
    };

    // The harness agents among the candidates: a routed turn cannot drive their sessions.
    let candidate_ids: Vec<Uuid> = agent_summaries.iter().map(|s| s.id).collect();
    let harness_ids: std::collections::HashSet<Uuid> = sqlx::query_scalar(
        "SELECT id FROM agents WHERE id = ANY($1) AND workload_kind = 'harness'",
    )
    .bind(&candidate_ids)
    .fetch_all(&state.db)
    .await
    .map_err(|e| A2aDispatchError::Internal(e.to_string()))?
    .into_iter()
    .collect();

    // What a call may reach, by the name the loop will use. Two candidates with one display name
    // cannot be told apart by a call, so the model is not offered either.
    let candidates = crate::acl::TurnCandidates::new(
        agent_summaries
            .iter()
            .filter(|s| !harness_ids.contains(&s.id))
            .map(|s| (s.name.as_str(), s.id)),
    );

    let mut agents: Vec<AgentInfo> = Vec::new();
    for summary in &agent_summaries {
        if harness_ids.contains(&summary.id) || candidates.is_ambiguous(&summary.name) {
            continue;
        }
        // Every candidate is listed with a placeholder address and no credential: nothing is
        // resolved (so no parked agent is woken and the runtime is not polled per candidate).
        // `CpCallGuard::before_call` resolves the one leg the model picks, by id, once it has
        // cleared every gate, and the call goes to the address that returns
        // (`AgentCallHandle::target`).
        let placeholder = format!("{LEG_RESOLVED_ENDPOINT_PREFIX}{}", summary.id);
        agents.push(AgentInfo {
            id: summary.id.to_string(),
            name: summary.name.clone(),
            description: summary.description.clone(),
            endpoint: placeholder,
            auth_header: None,
            skills: summary
                .skills
                .iter()
                .enumerate()
                .map(|(i, s)| OrcAgentSkill {
                    id: format!("{}-skill-{}", summary.name, i),
                    name: s.name.clone(),
                    description: s.description.clone(),
                    tags: summary.tags.clone(),
                    examples: s.examples.clone(),
                })
                .collect(),
        });
    }

    if agents.is_empty() {
        return Err(A2aDispatchError::NoAgents);
    }

    // The operator's policy, resolved fresh per turn so a settings change takes
    // effect without a restart. `None` on a deployment with no policy to apply,
    // which is what the open-source source always returns.
    let policy = state.orchestrator_policy.chat_policy(&state.db, kind).await;
    // Supplemental per-agent context (e.g. admin-authored knowledge), gathered before the LLM
    // has chosen anything — ranked against `query` (the same text about to reach the LLM) in
    // addition to any pinned content, folded into the preamble next to each candidate's own
    // listing so the planner can answer a zero-leg question ("how many leave days do I get")
    // directly, or route with that context already in hand. A no-op on OSS
    // (`NoopPromptContextProvider`); see `crate::prompt_context`.
    //
    // This reaches only the *planner's own* preamble, and that's now permanent rather than a v1
    // gap: a minted sub-leg's actual outbound request (built in `nasiko-react-agent`'s
    // `A2aTool::call()`, a lower-level OSS crate with no path to this EE-installed context)
    // carries its own L1A separately, injected by the EE injection proxy that
    // `forwarding_endpoint()` routes every outbound forward through — see
    // the edition's forwarding hop. The planner preamble can't move to that proxy because
    // it isn't an outbound forward at all; it's text folded into the LLM's own prompt before any
    // agent is chosen.
    // From `agents` (the roster actually rendered below), not `agent_summaries` — a candidate
    // dropped as a harness agent or an ambiguous name would waste a domain lookup (and possibly an
    // embedding call) on an id the preamble loop below can never reach anyway.
    let agent_ids: Vec<Uuid> = agents
        .iter()
        .filter_map(|a| Uuid::parse_str(&a.id).ok())
        .collect();
    let supplemental_context = state
        .prompt_context
        .context_for_agents(&agent_ids, query)
        .await;
    let agent_facts = if supplemental_context.is_empty() {
        None
    } else {
        let mut text = String::from("## Known facts about specific agents\n");
        for info in &agents {
            if let Ok(id) = Uuid::parse_str(&info.id)
                && let Some(facts) = supplemental_context.get(&id)
            {
                text.push_str(&format!("\n{}:\n{facts}\n", info.name));
            }
        }
        Some(text)
    };

    // User-scoped recall — unlike `agent_facts` above, not tied to any candidate agent, so it is
    // the one channel that reaches a **zero-leg** answer (the planner responds directly, without
    // delegating): a minted sub-leg's own outbound call carries no path back to this trait, and a
    // zero-leg turn never reaches one at all. `None` on OSS and on an EE deployment with nothing
    // user-scoped to add yet — see `PromptContextProvider::context_for_user`.
    let user_context = state.prompt_context.context_for_user(user_id, query).await;

    // Organization-wide context, resolved once for the whole turn. It reaches the routed path
    // TWICE, and both are needed: the planner's own preamble, because a zero-leg turn is answered
    // by the planner and never reaches an agent at all; and `outbound_prefix`, because a minted
    // sub-leg is a fresh conversation with a different model that never sees this preamble. One
    // resolution, two deliveries — a fan-out must not pay for this per leg.
    let org_context = state.prompt_context.org_context().await;
    // Captured before `org_context` is moved into the config below, so the span can report
    // whether policy was in force on this turn, and how large it was: `org_applied` alone is a
    // boolean, so a block that grew toward `MAX_POLICY_CHARS` would otherwise be invisible in
    // observability.
    let org_applied = org_context.is_some();
    let org_policy_chars = org_context.as_deref().map(str::len).unwrap_or(0);
    let preamble = build_preamble(
        org_context.as_deref(),
        agent_facts.as_deref(),
        user_context.as_deref(),
    );
    // IP-3 skips opted-out agents' results; unreadable switches turn it off for the turn.
    let uncompressed_tools = match opted_out_agent_tools(&state.db).await {
        Ok(tools) => Some(tools),
        Err(e) => {
            tracing::warn!(error = %e, "IP-3 off for this turn: agent switches unreadable");
            None
        }
    };

    // Whose key pays for this turn's model calls, recorded on each usage row it writes.
    let saved_key = nasiko_orchestrator::user_key::saved_openai_key(&state.db, user_id).await;
    let key_source = super::usage_meta::KeySource::for_saved_key(saved_key.is_some());
    let api_key_source = if saved_key.is_some() {
        nasiko_react_agent::ApiKeySource::Saved
    } else {
        nasiko_react_agent::ApiKeySource::Server
    };
    let mut config = OrchestratorConfig {
        // `state.config.openai_model` is already loaded via `env_or("OPENAI_MODEL",
        // "gpt-4o-mini")` (oss/config/src/lib.rs) — read that shared, validated
        // default rather than re-reading the env var here with a placeholder
        // fallback ("deepseek-v4-flash") that doesn't exist on a real OpenAI
        // endpoint and silently 404s every orchestrator call when OPENAI_MODEL
        // is unset.
        // Unset `OPENAI_MODEL` → empty, so a router-backed turn uses the router's default
        // model instead of the compiled `gpt-4o-mini` (unservable on e.g. Vertex-only).
        model: crate::state::explicit_openai_model(&state.config.openai_model).unwrap_or_else(
            || {
                let has_openai_key: bool = saved_key.is_some();
                if has_openai_key {
                    state.config.openai_model.clone()
                } else {
                    String::new()
                }
            },
        ),
        base_url: std::env::var("OPENAI_BASE_URL").ok(),
        // Only the user's saved key goes direct to OpenAI; platform credentials (including
        // `OPENAI_API_KEY`) are picked by the router via `DEFAULT_PROVIDER`.
        api_key: saved_key,
        api_key_source,
        max_turns: 10,
        temperature: Some(0.2),
        policy: policy.clone(),
        preamble,
        outbound_prefix: org_context,
        attachment_note: None,
        clock: state.agent_clock(),
        // IP-3. Read here rather than defaulted, because `ContextConfig::default()` is
        // deliberately inert — without this the compressor is compiled in but unreachable.
        context: nasiko_react_agent::ContextConfig {
            compress: nasiko_compress::Policy {
                enabled: state.config.react_compress_enabled && uncompressed_tools.is_some(),
                min_bytes: state.config.react_compress_min_bytes,
                ..Default::default()
            },
            uncompressed_tools: uncompressed_tools.unwrap_or_default(),
            ..nasiko_react_agent::ContextConfig::default()
        },
    };

    // Real root span for this exchange. Its ids seed the FlowContext, so the
    // traceparent forwarded to agents names a span that actually exists in
    // Tempo — previously agents parented to a phantom random span id and the
    // user→orchestrator hop was invisible in traces. GenAI agent semconv:
    // this is the orchestrator's invoke_agent span, carrying the user query
    // and final reply (content gated on the platform capture flag).
    let dispatch_span = tracing::info_span!(
        "a2a.dispatch",
        otel.kind = "server",
        gen_ai.operation.name = "invoke_agent",
        gen_ai.agent.name = "orchestrator",
        session.id = %context_id,
        nasiko.prompt_context.org_applied = org_applied,
        nasiko.prompt_context.org_policy_chars = org_policy_chars,
        gen_ai.input.messages = tracing::field::Empty,
        gen_ai.output.messages = tracing::field::Empty,
    );
    let capture_content = state.config.otel_capture_content;
    if capture_content {
        dispatch_span.record(
            "gen_ai.input.messages",
            crate::telemetry::genai_text_message("user", query).as_str(),
        );
    }
    let flow_ctx = crate::telemetry::flow_context_from_span(&dispatch_span)
        .unwrap_or_else(FlowContext::new_root);
    let flow_id = flow_ctx.flow_id.clone();
    let traceparent = crate::telemetry::traceparent_for(&flow_ctx);
    // The inbound traceparent seeds this trace id, and it is the caller's to send, so it can
    // name a flow that belongs to someone else. Refuse before any per-flow state is touched
    // (including the telemetry below, which is keyed by this trace id); `open_flow` further down
    // is the race-free backstop.
    refuse_foreign_flow(state, &flow_id, user_id).await?;

    // Selection ran in the caller, before any of this existed; `dispatch_span` is the span the
    // trace page looks for, so the report is attached here. `agent_id` stays unset — routing has
    // not chosen one yet — and the read path resolves it through `flows`, which this turn
    // writes as soon as it dispatches.
    if let Some(report) = context_report {
        crate::telemetry::record_context_report(
            &state.db,
            &dispatch_span,
            report,
            crate::telemetry::ContextRecord {
                user_id,
                agent_id: None,
                session_id: Some(context_id.to_string()),
                trace_id: Some(flow_id.clone()),
                span_id: Some(flow_ctx.parent_span_id.clone()),
            },
        );
    }

    state.flow_guard.init_flow(&flow_ctx, "orchestrator").await;

    // Carry the A2A context_id so the LLM gateway keys its decision cache on the
    // conversation, not this turn's trace id — mirrors the direct-agent proxy
    // (`agent_proxy.rs`). `derive_boundary_signals` reads `metadata->>'context_id'`.
    // `user_message` is what the LLM gateway's salience gate and classifier score;
    // only a user-started turn has one — a HITL resume's `raw_text` is platform-written.
    //
    // Re-opens on conflict for the same reason as the proxy path: a repeat
    // request under one traceparent must not inherit the `completed` status the
    // previous one left, or strict attribution denies the agent's LLM calls.
    let flow_metadata = match kind {
        TurnKind::User => serde_json::json!({ "context_id": context_id, "user_message": raw_text }),
        TurnKind::Continuation => serde_json::json!({ "context_id": context_id }),
    };
    open_dispatch_flow(
        state,
        crate::flows::NewFlow {
            flow_id: &flow_id,
            user_id,
            root_agent_id: None,
            root_agent_name: "orchestrator",
            title: Some(query),
            metadata: &flow_metadata,
            session_id: Some(context_id),
        },
    )
    .await?;
    config.attachment_note =
        attachment_note(state.marker_signer.as_deref(), &attached_files, &flow_id);
    let routed_query = with_attachment_list(query, &file_parts, &attached_files);

    // Flow origin: this flow_id is registered in `flows` and IS the trace id inside the
    // traceparent forwarded downstream. The gateway maps an agent's forwarded trace id
    // back to this row (see derive_boundary_signals) — so compare this flow_id against the
    // gateway's `nasiko::llm_router::boundary` log to spot a broken trace-propagation chain.
    tracing::info!(
        target: "nasiko::flow",
        %flow_id,
        context_id = %context_id,
        task_id = %task_id,
        %traceparent,
        "orchestrator flow started — registered flows row; forwarding traceparent (trace_id == flow_id) to agents"
    );

    let caller_uuid: Option<Uuid> = None;
    let guard = CpCallGuard::new(
        state.db.clone(),
        state.flow_guard.clone(),
        flow_ctx,
        caller_uuid,
        state.spend_gate.clone(),
        user_id,
        context_id.to_string(),
        state.genai_metrics.agent_call_meter(),
        state.agent_wake.clone(),
        crate::acl::TurnLegs {
            // Resolves each leg's target once the call has cleared every gate. The guard asks
            // the wake seam itself, before it spends flow budget, so the resolver does not.
            targets: Arc::new(
                crate::agent_resolver::ServerTargetResolver::after_caller_wake(state.clone()),
            ),
            user_id,
            // A resumed turn's own prompt tells the model not to call anyone again; the guard
            // makes that binding, so a continuation cannot reach any agent.
            candidates: if kind == TurnKind::Continuation {
                crate::acl::TurnCandidates::default()
            } else {
                candidates.clone()
            },
        },
    );

    // `A2aClient`'s own default is deliberately short — it is shared with agent
    // card / discovery fetches, where a long hang is the wrong behaviour. This
    // client makes real agent turns, including the `message/send` fallback taken
    // by agents that reject `message/stream`, so it carries the agent budget.
    let a2a_client = nasiko_react_agent::A2aClient::with_http_client(state.agent_client.clone())
        .with_timeout(std::time::Duration::from_secs(
            state.config.agent_call_timeout_secs,
        ))
        .with_headers(vec![("traceparent".to_string(), traceparent)]);

    // Each agent the orchestrator calls authenticates to /api/mcp with its own
    // deploy-time MCP_GATEWAY_TOKEN; the user binding rides the forwarded
    // traceparent + the flow_participants record `CpCallGuard` writes per leg.
    let mut orchestrator = Orchestrator::new(config, RegistrySource::Static(agents))
        .with_a2a_client(a2a_client)
        .with_guard(guard);
    orchestrator
        .init()
        .await
        .map_err(|e| A2aDispatchError::Internal(e.to_string()))?;

    // Taken before the run, read after it: `run_stream` compresses inside a spawned task holding a
    // *clone* of the context manager, so this shared handle is the only way the turn's IP-3
    // measurement is visible out here (see `CompressionTally`).
    let compression_tally = orchestrator.compression_tally();

    let mut rx = {
        let _entered = dispatch_span.enter();
        orchestrator.run_stream(&routed_query, file_parts)
    };
    let task_id = task_id.to_string();
    let context_id = context_id.to_string();
    let artifact_id = Uuid::new_v4().to_string();
    let flow_events = state.flow_events.clone();
    let mut flow_rx = state.flow_events.subscribe(&flow_id).await;
    let flow_id_cleanup = flow_id.clone();
    let db = state.db.clone();
    let hitl_store = state.hitl_store.clone();
    let usage_tracker = state.usage_tracker.clone();
    let genai_metrics = state.genai_metrics.clone();
    let orchestrator_model = state.config.openai_model.clone();
    let orchestrator_start = Instant::now();
    let mut full_reply = String::new();
    let observability = state.observability.clone();
    // The same policy the loop ran under, kept for the persist step below: only
    // the policy that produced a refusal can reliably recognise one.
    let policy_for_persist = policy.clone();
    // Accumulated orchestrator-turn usage for the terminal `usage_meta` event.
    let mut turn_usage = super::usage_meta::TurnUsage::default();
    let pricing = state.pricing.clone();

    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

        // Emit trace_id so the UI can link this response to its distributed trace.
        {
            let meta_msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                "type": "trace_meta", "trace_id": flow_id,
            })));
            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, meta_msg))));
        }

        let mut content_started = false;
        // Persist on Usage, before yielding a terminal event. Only attribution
        // waits for the next ToolCall; client disconnect cannot lose a pending row.
        let mut pending_usage: Option<crate::usage::TraceUsageKey> = None;

        loop {
            tokio::select! {
                biased;

                maybe_event = rx.recv() => {
                    let Some(event) = maybe_event else { break };
                    match event {
                        OrchestratorEvent::Thinking { content } => {
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({"type": "thinking", "content": content})));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::ToolCall { agent, message, turn, policy_score } => {
                            if let Some(key) = pending_usage.take() {
                                // By the candidate's id, not a name lookup: a name is only unique per owner.
                                let result = sqlx::query(
                                    "UPDATE trace_usage SET agent_id = COALESCE(agent_id, $3) WHERE trace_id = $1 AND agent_name = $2 AND user_id = $4",
                                ).bind(&key.trace_id).bind(&key.agent_name).bind(candidates.get(&agent)).bind(user_id).execute(&db).await;
                                if let Err(error) = result {
                                    tracing::warn!(%error, "failed to attribute orchestrator usage");
                                }
                            }

                            // An earlier pause's `awaiting_human` step is closed by
                            // `hitl/mod.rs::close_resumed_flow_step`, not here: a resumed turn is a
                            // new `orchestrator_stream` call with its own flow id, so keying on
                            // this turn's matched nothing — and the orchestrator usually answers
                            // directly after a resume, so no ToolCall arrives at all.
                            let _ = sqlx::query(
                                r#"INSERT INTO flow_steps (flow_id, step_order, depth, agent_name, caller_agent_name, input_summary, status, created_at)
                                   VALUES ($1, $2, 1, $3, 'orchestrator', $4, 'running', now())"#,
                            )
                            .bind(&flow_id_cleanup)
                            .bind(turn as i32)
                            .bind(&agent)
                            .bind(&message)
                            .execute(&db)
                            .await;

                            // Record agent invocation in OTel
                            genai_metrics.record_invocation(&agent, "");

                            let mut payload = json!({
                                "type": "tool_call",
                                "agent": agent,
                                "message": message,
                                "turn": turn,
                            });
                            // Whatever score the operator's policy attached to this
                            // delegation — on the wire so it is visible in the UI's step
                            // row, not only in server logs. Added only when there is one,
                            // rather than sent as an explicit null, so an unconfigured
                            // deployment emits exactly the event it emitted before this
                            // seam existed.
                            if let Some(score) = policy_score {
                                payload["policy_score"] = json!(score);
                            }
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(payload));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::ToolResult { agent, result, success, turn, duration_ms } => {
                            let status_str = if success { "completed" } else { "failed" };
                            let _ = sqlx::query(
                                r#"UPDATE flow_steps SET status = $3, output_summary = $4,
                                   latency_ms = EXTRACT(EPOCH FROM (now() - created_at))::integer * 1000,
                                   completed_at = now()
                                   WHERE flow_id = $1 AND step_order = $2"#,
                            )
                            .bind(&flow_id_cleanup)
                            .bind(turn as i32)
                            .bind(status_str)
                            .bind(&result)
                            .execute(&db)
                            .await;

                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "tool_result",
                                "agent": agent,
                                "result": result,
                                "success": success,
                                "turn": turn,
                                "duration_ms": duration_ms,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::SubStatus { agent, message } => {
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "sub_status",
                                "agent": agent,
                                "message": message,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::SubContent { agent, content } => {
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "sub_content",
                                "agent": agent,
                                "content": content,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::SubData { via_agent, data } => {
                            // Relay the nested agent's own structured step
                            // (already tagged with its own `type`, e.g.
                            // weave's `agent_invoke`/`agent_result` for its
                            // sub-agents) instead of leaving it collapsed
                            // inside the single opaque ToolCall/ToolResult
                            // above — this is what lets the UI show a called
                            // orchestrator's own sub-agent spawn/finish.
                            // `via_agent` is added for attribution only;
                            // every other field is exactly what the nested
                            // agent sent, unmodified.
                            let mut payload = data;
                            if let Some(obj) = payload.as_object_mut() {
                                obj.entry("via_agent").or_insert_with(|| json!(via_agent));
                            }
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(payload));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::PolicyRejected { agent, reason, turn, kind } => {
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "policy_rejected",
                                "agent": agent,
                                "reason": reason,
                                "turn": turn,
                                "kind": kind,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                        }
                        OrchestratorEvent::Usage { usage } => {
                            let input_tokens = usage.input_tokens;
                            let output_tokens = usage.output_tokens;
                            let model = usage.model.clone();
                            let priced = super::usage_meta::PricedTurn::price(&pricing, usage).await;
                            turn_usage.add(&priced);

                            pending_usage = match priced.persist(&usage_tracker, user_id, &flow_id_cleanup, None, key_source).await {
                                Ok(id) => Some(id),
                                Err(error) => {
                                    tracing::warn!(%error, "failed to persist orchestrator usage");
                                    None
                                }
                            };

                            // Also record in OTel GenAI metrics
                            genai_metrics.record_tokens(
                                input_tokens,
                                output_tokens,
                                &model,
                                "orchestrator",
                                "",
                            );
                        }
                        OrchestratorEvent::Content { content } => {
                            full_reply.push_str(&content);
                            yield Ok(to_sse(a2a::artifact_event(a2a::text_chunk(
                                &task_id, &context_id, &artifact_id, &content, content_started, false,
                            ))));
                            content_started = true;
                        }
                        OrchestratorEvent::AwaitingHuman { agent, agent_id, pause } => {
                            // Close out the flow_steps row this pause interrupted instead of
                            // leaving it at 'running' forever — only ToolResult (never sent on a
                            // pause) closed it before. This event carries no turn/step_order (unlike
                            // ToolCall/ToolResult), so matching by (flow_id, agent_name, running)
                            // alone would be ambiguous if the same agent was called more than once
                            // earlier in this flow and an earlier call's own close-out silently
                            // failed (these UPDATEs are fire-and-forget) — that stale 'running' row
                            // could then be the one this UPDATE hits instead of the current call's.
                            // The subquery picks only the most recently inserted still-running row
                            // for this agent (highest step_order), which is always the current call.
                            let _ = sqlx::query(
                                "UPDATE flow_steps SET status = 'awaiting_human', completed_at = now()
                                 WHERE id = (
                                     SELECT id FROM flow_steps
                                      WHERE flow_id = $1 AND agent_name = $2 AND status = 'running'
                                      ORDER BY step_order DESC
                                      LIMIT 1
                                 )",
                            )
                            .bind(&flow_id_cleanup)
                            .bind(&agent)
                            .execute(&db)
                            .await;

                            let hitl_kind = match pause.kind {
                                a2a::AwaitingHumanKind::InputRequired => nasiko_hitl::HitlKind::InputRequired,
                                a2a::AwaitingHumanKind::AuthRequired => nasiko_hitl::HitlKind::AuthRequired,
                            };

                            // hitl_requests.agent_id is a real FK to agents(id) — agent_id here is
                            // always the sub-agent's own UUID string (AgentInfo::id, set from the
                            // agents-table row this orchestrator discovered, never a display name).
                            // Goes through the real `HitlStore` (not raw SQL) so a duplicate pause
                            // for the same task_id is handled idempotently — `create()` catches the
                            // uq_hitl_pending_per_task collision and returns the existing row
                            // instead of erroring — and so `expires_at` gets a real TTL instead of
                            // never expiring.
                            let persisted = match Uuid::parse_str(&agent_id) {
                                Ok(sub_agent_id) => {
                                    // `pause_question`, not an inline `json!` — direct chat and
                                    // the orchestrator's own follow-up pauses (`hitl/mod.rs`) both
                                    // go through it (via `build_pause_question`), hoisting
                                    // `auth_url`/`provider`/`expected_input` (and the
                                    // selectable-options extension) to the top level; building
                                    // this row's `question` by hand instead meant a consumer
                                    // reading `question.auth_url` got the OAuth link on one row
                                    // in a paused chain but not another, and every
                                    // orchestrator-origin options question rendered as a plain
                                    // text box.
                                    let question = a2a::pause_question(
                                        &pause.message,
                                        (!pause.metadata.is_null()).then(|| pause.metadata.clone()),
                                        Some(pause.task_id.as_str()),
                                    );
                                    hitl_store
                                        .create(nasiko_hitl::NewHitlRequest::orchestrator(
                                            hitl_kind,
                                            sub_agent_id,
                                            user_id,
                                            pause.task_id.clone(),
                                            pause.context_id.clone(),
                                            context_id.clone(),
                                            question,
                                        ))
                                        .await
                                        .map_err(|e| e.to_string())
                                }
                                Err(e) => Err(e.to_string()),
                            };

                            // Never report a fake success: if the pending question couldn't be
                            // recorded, the human will never see it, so the turn must fail loudly
                            // rather than silently close the stream as if it were fine.
                            let hitl_row = match persisted {
                                Ok(row) => row,
                                Err(e) => {
                                    tracing::error!(error = %e, %agent, "failed to persist HITL pending question");
                                    let _ = sqlx::query(
                                        "UPDATE flows SET status = 'failed', error_message = $2 WHERE flow_id = $1",
                                    )
                                    .bind(&flow_id_cleanup)
                                    .bind(&e)
                                    .execute(&db)
                                    .await;
                                    yield Ok(to_sse(a2a::status_event(a2a::failed(
                                        &task_id, &context_id,
                                        &format!("could not record pending question for {agent}: {e}"),
                                    ))));
                                    break;
                                }
                            };

                            // Mirrors `persist_direct_chat_pause`'s convention — a paused flow is
                            // not a completed one. Guarded (`status = 'running'`) at the shared
                            // epilogue below so this stamp isn't immediately overwritten back to
                            // 'completed'.
                            let _ = sqlx::query("UPDATE flows SET status = 'paused' WHERE flow_id = $1")
                                .bind(&flow_id_cleanup)
                                .execute(&db)
                                .await;
                            // Stash the original flow_id for the resume dispatcher (same
                            // rationale as `persist_direct_chat_pause`).
                            let _ = sqlx::query(
                                "UPDATE hitl_requests SET resume_state = resume_state || $2 WHERE id = $1",
                            )
                            .bind(hitl_row.id)
                            .bind(serde_json::json!({ "flow_id": &flow_id_cleanup }))
                            .execute(&db)
                            .await;

                            // NOTE: no chat_messages checkpoint is written here yet — how a
                            // resumed-turn checkpoint should be tagged in chat_messages is still
                            // an open decision (tracker's "Open decision 3" / review finding S5);
                            // writing one now would mean guessing a shape the resume dispatcher
                            // (Step 7, not yet built) might not actually read.
                            let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                "type": "awaiting_human",
                                "agent": agent,
                                "message": pause.message,
                            })));
                            yield Ok(to_sse(a2a::status_event(a2a::awaiting_human(&task_id, &context_id, pause.kind, msg))));
                            // Frontend discovery (matches direct chat's own convention, §"deliver
                            // pause metadata on the live stream"): carries `hitl_row.id`, minted
                            // and durably committed by `hitl_store.create()` above, so the id on
                            // the wire always already exists in Postgres by the time a client can
                            // observe it. `Some(&agent)` — unlike direct chat, which never sends
                            // this because its one stream is always with the one agent the human
                            // is already talking to, the orchestrator can delegate to any of
                            // several agents, so the frontend needs to know which one is asking.
                            yield Ok(build_hitl_stream_event(&hitl_store, &task_id, &context_id, &hitl_row, Some(&agent)).await);
                            break;
                        }
                        OrchestratorEvent::Done { .. } => {
                            if content_started {
                                yield Ok(to_sse(a2a::artifact_event(a2a::text_chunk(
                                    &task_id, &context_id, &artifact_id, "", true, true,
                                ))));
                            }

                            let summary = super::usage_meta::summarize_flow_usage(
                                &db,
                                observability.as_ref(),
                                &flow_id_cleanup,
                                &turn_usage,
                                orchestrator_start.elapsed().as_millis() as i64,
                            )
                            .await;

                            // Mirrors `agent_proxy.rs` persisting the agent's reply after
                            // a completed turn — without this, `--session-id` resume has
                            // no recorded history to actually resume, even though the
                            // session row and user message (above) now exist.
                            if !full_reply.is_empty() {
                                // A refusal is persisted so the human still sees it, but
                                // tagged so it never re-enters the next turn's reasoning —
                                // otherwise the model reads its own refusal back as this
                                // conversation's established behaviour and keeps refusing.
                                // The policy that produced the text is the one asked to
                                // recognise it, so this cannot drift from what was emitted.
                                let is_refusal = policy_for_persist
                                    .as_ref()
                                    .is_some_and(|p| p.is_refusal(&full_reply));
                                super::usage_meta::insert_assistant_message(
                                    &db,
                                    &context_id,
                                    &full_reply,
                                    &summary,
                                    &flow_id_cleanup,
                                    is_refusal,
                                )
                                .await;
                            }

                            // Terminal usage summary — the UI renders this as the
                            // message's token/duration/cost chips.
                            {
                                let usage_msg = a2a::agent_message(&context_id, &task_id,
                                    a2a::data_part(summary.to_data_part(&flow_id_cleanup)));
                                yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, usage_msg))));
                            }

                            // Record the final reply on the dispatch span
                            // (moved into this stream so it also spans the
                            // full streamed exchange, not just the handler).
                            if capture_content && !full_reply.is_empty() {
                                dispatch_span.record(
                                    "gen_ai.output.messages",
                                    crate::telemetry::genai_text_message("assistant", &full_reply)
                                        .as_str(),
                                );
                            }

                            // Record overall operation duration in OTel
                            genai_metrics.record_operation(
                                orchestrator_start.elapsed().as_secs_f64(),
                                "orchestrate",
                                &orchestrator_model,
                                "orchestrator",
                                "",
                            );

                            yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));
                            break;
                        }
                        OrchestratorEvent::Error { message } => {
                            super::usage_meta::insert_turn_error(&db, &context_id, &message, &flow_id_cleanup).await;
                            yield Ok(to_sse(a2a::status_event(a2a::failed(&task_id, &context_id, &message))));
                            break;
                        }
                    }
                }

                flow_event = flow_rx.recv() => {
                    let Ok(fe) = flow_event else { continue };
                    let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(serde_json::to_value(&fe).unwrap_or_default()));
                    yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                }
            }
        }

        // IP-3's turn-level saving. Emitted here rather than at each `push_tool_result` because
        // the loop's spawned task is finished by the time this line runs — the stream exits when
        // the orchestrator's channel closes — so the tally is complete and this is the one point
        // where it and the request's identity are both in scope. A pause (HITL) reaches here too,
        // and the results compressed before the pause are a real saving that the resumed turn does
        // not re-compress, so it is recorded rather than discarded.
        crate::telemetry::record_tool_result_savings(
            &db,
            &dispatch_span,
            &compression_tally,
            crate::telemetry::ToolResultRecord {
                user_id,
                agent_id: None,
                session_id: Some(context_id.clone()),
                flow_id: Some(flow_id_cleanup.clone()),
            },
        );

        // Guarded on `status = 'running'` so a pause (which already stamped 'paused'/'failed'
        // above, before breaking the loop) isn't immediately overwritten back to 'completed' —
        // Done/Error never change `flows.status` before reaching here, so this is a no-op change
        // in behavior for either of those.
        let _ = sqlx::query(
            r#"UPDATE flows SET status = 'completed',
               duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
               completed_at = now()
               WHERE flow_id = $1 AND status = 'running'"#,
        )
        .bind(&flow_id_cleanup)
        .execute(&db)
        .await;

        flow_events.remove(&flow_id_cleanup).await;
    };

    Ok(Sse::new(stream).into_response())
}

// ─── Direct Agent Path ───────────────────────────────────────────────────────

/// Resolve a caller-supplied target (either the agent's UUID or its name) to a
/// concrete, running agent row. Callers MUST authorize on the returned `id`
/// before using it — this function does no access control of its own.
/// Rejects a dispatch whose flow id already belongs to another user. See
/// [`crate::flows::OpenFlowError::ForeignFlow`] for why the flow id cannot be trusted.
async fn refuse_foreign_flow(
    state: &AppState,
    flow_id: &str,
    user_id: Uuid,
) -> Result<(), A2aDispatchError> {
    match crate::flows::flow_belongs_to_another(&state.db, flow_id, user_id).await {
        Ok(false) => Ok(()),
        Ok(true) => Err(foreign_flow_rejection(flow_id, user_id)),
        Err(e) => Err(A2aDispatchError::Internal(e.to_string())),
    }
}

/// Opens the dispatch's `flows` row. The row is load-bearing for the agent's MCP and LLM
/// authorization, and an unwritten row has no known owner, so a failure here stops the dispatch
/// rather than forwarding without it.
async fn open_dispatch_flow(
    state: &AppState,
    flow: crate::flows::NewFlow<'_>,
) -> Result<(), A2aDispatchError> {
    let (flow_id, user_id) = (flow.flow_id.to_string(), flow.user_id);
    crate::flows::open_flow(&state.db, flow)
        .await
        .map_err(|e| match e {
            crate::flows::OpenFlowError::ForeignFlow => foreign_flow_rejection(&flow_id, user_id),
            crate::flows::OpenFlowError::Db(e) => A2aDispatchError::Internal(e.to_string()),
        })
}

fn foreign_flow_rejection(flow_id: &str, user_id: Uuid) -> A2aDispatchError {
    tracing::warn!(%flow_id, %user_id, "a2a dispatch: traceparent names another user's flow");
    A2aDispatchError::Forbidden("traceparent does not belong to this user".into())
}

async fn resolve_agent(state: &AppState, target: &str) -> Result<AgentRow, A2aDispatchError> {
    // Excludes `is_internal` agents unconditionally, including for the owning
    // superuser — this is the platform's only generic A2A entry point, and an
    // internal agent (e.g. Weave's dashboard-generator) must be reachable
    // exclusively through its own dedicated route, never here, or the
    // superuser-ACL-bypass would leak it into ordinary chat
    // history/usage tracking.
    //
    // Also reads the columns `agent_resolver` routes on, so the forward resolves from this row
    // instead of reading it again. `keeps_session_memory` is only ever true for an internal
    // agent, which this query excludes.
    sqlx::query_as::<_, AgentRow>(
        "SELECT id, name, status, minimal_code_enabled, skills, compress_enabled, \
                owner_id, workload_kind, image, url, transport_path, writable, deleted_at, \
                is_internal, false AS keeps_session_memory \
         FROM agents \
         WHERE (id::text = $1 OR name = $1) AND status IN ('running', 'parked') AND NOT is_internal \
           AND deleted_at IS NULL",
    )
    .bind(target)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| A2aDispatchError::Internal(e.to_string()))?
    .ok_or_else(|| A2aDispatchError::AgentNotFound(target.to_string()))
}

/// Folds organization policy (L0) into an already-built request as its own message part, never
/// by prepending to the text part a caller already set: that text may be history-glued with
/// `session_history.rs`'s `"Current message: "` marker, and any agent that follows the
/// platform's own documented convention for extracting just the current turn
/// (`oss/llm-router::latest_user_query`) silently discards whatever was prepended ahead of that
/// marker inside the same part. A no-op when `org_context` is `None` — most turns.
fn append_org_policy_part(req: &mut JsonRpcRequest, org_context: Option<&str>) {
    let Some(org) = org_context else { return };
    // `nasiko_types::a2a::build_request` (what builds every `req` this function ever sees) always
    // uses the proto-style method names, so this is always `PartTagging::Untagged` in practice —
    // still read from `req.method` rather than hardcoded, so this stays correct if that ever
    // changes. See `nasiko_agent_proxy::context_part`'s module doc for why the tag matters at all:
    // this platform's real agents (a2a-lf-based) reject an unexpected `"kind"` field outright.
    let tagging = nasiko_agent_proxy::context_part::tagging_for_method(&req.method);
    let Some(message) = req.params.as_mut().and_then(|p| p.get_mut("message")) else {
        return;
    };
    // `org_context()` already returns a complete, self-headed block ("## Organization
    // Policy\n\n..."), unlike L1A's raw facts — no extra heading to add here.
    let _ = nasiko_agent_proxy::context_part::append_context_part(message, org, tagging);
}

/// Appends the current date and time as its own part, after organization policy, on the same
/// terms the agent proxy does (`crate::current_time`). Every request this path builds opens a
/// task, so none is a continuation.
fn append_current_time_part(
    req: &mut JsonRpcRequest,
    clock: nasiko_types::agent_clock::AgentClock,
) {
    let tagging = nasiko_agent_proxy::context_part::tagging_for_method(&req.method);
    if let Some(message) = req.params.as_mut().and_then(|p| p.get_mut("message")) {
        crate::current_time::append_current_time(message, clock, tagging);
    }
}

/// Orchestrator tool names of the agents whose Token optimization switch is off.
async fn opted_out_agent_tools(
    db: &sqlx::PgPool,
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM agents \
         WHERE status = 'running' AND deleted_at IS NULL AND NOT is_internal \
           AND NOT compress_enabled",
    )
    .fetch_all(db)
    .await?;
    Ok(names
        .iter()
        .map(|name| nasiko_react_agent::A2aTool::tool_name(name))
        .collect())
}

#[allow(clippy::too_many_arguments)]
async fn agent_stream(
    state: &AppState,
    agent: AgentRow,
    // The user's actual message — persisted (`flows.title`) and traced
    // (`gen_ai.input.messages`) verbatim. Per-agent supplemental context (L1A) no longer gets
    // folded into this text at all — that injection now happens at the EE injection proxy
    // instead, on every outbound forward `forwarding_endpoint()` resolves — including this one.
    // What's actually sent to the agent is `effective_query` below, which may still differ from
    // this for an unrelated reason (the minimal-code ladder addendum).
    query: &str,
    // What the caller typed, before history packing — recorded on the flow for the LLM gateway.
    user_message: &str,
    // Organization policy (L0), if any is currently configured. Appended as a separate `parts[]`
    // entry on the outbound request rather than prepended to the query text: that text may already
    // be history-glued with `session_history.rs`'s `"Current message: "` marker, and any agent that
    // follows the platform's own documented convention for extracting just the current turn
    // (`oss/llm-router::latest_user_query`) silently discards whatever was prepended ahead of that
    // marker inside the same part. See `nasiko_agent_proxy::context_part`'s module doc.
    org_context: Option<&str>,
    task_id: &str,
    context_id: &str,
    user_id: Uuid,
    file_parts: &[nasiko_types::a2a::Part],
    attached_files: &[AttachedFile],
    // The web UI's chat session id (metadata.session_id) — `None` for CLI/TUI callers that
    // address history by contextId alone. Threaded through purely so a HITL pause created on
    // this turn can be tagged with `chat_session_id`, letting `chat/routes.rs::list_messages`
    // surface it on session load without a separate discovery call. Not used for anything else
    // in this function — history lookup already happened in the caller.
    //
    // Owned, not `&str`: both pause branches below build an `async_stream::stream! {}` that
    // `Sse::new(...).into_response()` requires to be `'static` — a caller-borrowed `&str`
    // cannot satisfy that, only a value this function owns and moves into the generator can.
    session_id: Option<String>,
    // Prior user turns in this session (`history.user_turn_count()` at the call
    // site) — picks which minimal-code ladder variant to inject below. A fresh
    // session has nothing in its workspace yet to search for; forcing the full
    // search-first ladder there only spends tokens finding nothing. An
    // established session (several turns in) leans the other way — see
    // nasiko-coding-policy's minimal_code_addendum() doc comment.
    prior_turn_count: usize,
    // As `OrchestratorTurn::context_report`: this function owns the `a2a.dispatch` span the
    // report has to be attached to.
    context_report: Option<&nasiko_orchestrator::ContextReport>,
) -> Result<Response, A2aDispatchError> {
    let org_applied = org_context.is_some();
    // `org_applied` alone is a boolean — a block that grew toward `MAX_POLICY_CHARS` would
    // otherwise be invisible in observability.
    let org_policy_chars = org_context.map(str::len).unwrap_or(0);

    // Real root span for the direct-agent exchange — same rationale as the
    // orchestrator path: the forwarded traceparent must name an exported span.
    let dispatch_span = tracing::info_span!(
        "a2a.dispatch",
        otel.kind = "server",
        gen_ai.operation.name = "invoke_agent",
        gen_ai.agent.name = %agent.name,
        session.id = %context_id,
        nasiko.prompt_context.org_applied = org_applied,
        nasiko.prompt_context.org_policy_chars = org_policy_chars,
        gen_ai.input.messages = tracing::field::Empty,
        gen_ai.output.messages = tracing::field::Empty,
    );
    let capture_content = state.config.otel_capture_content;
    if capture_content {
        dispatch_span.record(
            "gen_ai.input.messages",
            crate::telemetry::genai_text_message("user", query).as_str(),
        );
    }
    let flow_ctx = crate::telemetry::flow_context_from_span(&dispatch_span)
        .unwrap_or_else(FlowContext::new_root);
    let flow_id = flow_ctx.flow_id.clone();
    // The inbound traceparent seeds this trace id, and it is the caller's to send, so it can
    // name a flow that belongs to someone else. Refuse before any per-flow state is touched
    // (including the telemetry below, which is keyed by this trace id); `open_flow` further down
    // is the race-free backstop.
    refuse_foreign_flow(state, &flow_id, user_id).await?;

    // Woken, then resolved by kind, once the flow is known, so a wake is announced on its event
    // stream. The wake comes first: a parked agent has no endpoint until its workload is back.
    state
        .agent_wake
        .before_forward(agent.id, Some(&flow_id))
        .await
        .map_err(A2aDispatchError::Unavailable)?;
    let caller = Caller {
        flow_id: Some(flow_id.clone()),
    };
    let target = match resolve_loaded(state, &agent.route, &caller).await {
        Ok(resolved) => resolved.into_forward_target().ok_or_else(|| {
            A2aDispatchError::Internal(format!(
                "agent '{}' is a harness agent and has no endpoint to forward to",
                agent.name
            ))
        })?,
        Err(e) => return Err(A2aDispatchError::Resolve(e)),
    };

    if let Some(report) = context_report {
        crate::telemetry::record_context_report(
            &state.db,
            &dispatch_span,
            report,
            crate::telemetry::ContextRecord {
                user_id,
                agent_id: Some(agent.id),
                session_id: Some(context_id.to_string()),
                trace_id: Some(flow_id.clone()),
                span_id: Some(flow_ctx.parent_span_id.clone()),
            },
        );
    }

    state.flow_guard.init_flow(&flow_ctx, &agent.name).await;

    // Minimal-code eligibility + holdout, decided once up front so the flow row recorded below and
    // the addendum injection further down (which only skips when withheld) can't disagree about
    // which arm this flow landed in. See `nasiko_coding_policy::in_minimal_code_holdout` for why a
    // control arm is withheld at all — same reasoning as the brevity directive's own holdout.
    let minimal_code_eligible =
        agent.minimal_code_enabled && crate::catalog::models::has_coding_skills(&agent.skills);
    let minimal_code_holdout = minimal_code_eligible
        && nasiko_coding_policy::in_minimal_code_holdout(
            &flow_id,
            state.config.minimal_code_holdout_pct,
        );

    // Carry the A2A context_id so the LLM gateway keys its decision cache on the
    // conversation, not this turn's trace id — same reason as the orchestrator
    // branch above and `agent_proxy.rs`. Without it `derive_boundary_signals`
    // falls back to the per-turn flow_id, so every turn of one conversation looks
    // like a new conversation and the sticky decision is never reused.
    //
    // `user_message` (the text before history packing) feeds the LLM gateway's
    // salience gate and classifier — see the orchestrator branch above.
    //
    // Re-opens on conflict — see the orchestrator branch above: a repeat request
    // under one traceparent must not inherit the previous one's `completed`.
    let mut flow_metadata =
        serde_json::json!({ "context_id": context_id, "user_message": user_message });
    if minimal_code_eligible {
        flow_metadata["minimal_code"] = if minimal_code_holdout {
            serde_json::json!({ "applied": false, "skipped": "holdout" })
        } else {
            serde_json::json!({ "applied": true })
        };
    }
    open_dispatch_flow(
        state,
        crate::flows::NewFlow {
            flow_id: &flow_id,
            user_id,
            root_agent_id: Some(agent.id),
            root_agent_name: &agent.name,
            title: Some(query),
            metadata: &flow_metadata,
            session_id: Some(context_id),
        },
    )
    .await?;
    // Participant record — load-bearing for MCP gateway / LLM router auth
    // (docs/MCP_GATEWAY_AGENT_AUTH.md §2.4); same synchronous pre-forward write
    // as the flows row above.
    crate::flows::record_participant(&state.db, &flow_id, agent.id).await;
    let attachment_note = attachment_note(state.marker_signer.as_deref(), attached_files, &flow_id);

    // Minimal-code ladder injection: only what's actually FORWARDED to the
    // agent gets the addendum appended — `query` itself stays untouched, so
    // everything above that already used it (the flow title, the span's
    // captured input, flows) keeps showing the real conversation,
    // not an implementation detail. The agent needs zero code of its own to
    // support this: it just sees a longer task description, exactly as it
    // would if a human had pasted the same extra paragraph in by hand.
    //
    // `prior_turn_count` picks the ladder variant: a session's first turn has
    // an empty workspace, so the full "search the codebase first" ladder just
    // spends tokens finding nothing there — confirmed empirically (chat
    // 2026-09-22) to cost more per turn than not having the ladder on at all,
    // on exactly this kind of from-scratch task. See nasiko-coding-policy's
    // minimal_code_addendum() doc comment for the three-tier reasoning.
    // Built from `query`: on this path L1A (per-agent supplemental context) no longer gets
    // folded in here at all — it now rides the EE injection proxy on the real outbound forward
    // (`forwarding_endpoint()`, see this function's call site) — so `query` is already the whole
    // text this dispatch is meant to send; there is no separate `outbound_query` left to build
    // from.
    let effective_query = if minimal_code_eligible && !minimal_code_holdout {
        let addendum = nasiko_coding_policy::minimal_code_addendum(prior_turn_count);
        tracing::info!(
            agent_id = %agent.id,
            %context_id,
            prior_turn_count,
            "a2a_dispatch: injecting minimal-code ladder"
        );
        format!("{query}\n{addendum}")
    } else {
        if minimal_code_holdout {
            tracing::debug!(
                agent_id = %agent.id,
                %context_id,
                "a2a_dispatch: minimal-code ladder withheld for this flow's holdout arm"
            );
        }
        query.to_string()
    };

    // Streaming first (`message/stream`): agents that stream (all the Rust
    // seed agents, and python a2a-sdk servers) deliver live tokens and tool
    // activity. The SSE loop below terminates itself on the first terminal
    // status event because the python a2a-sdk SSE producer never closes the
    // stream on its own (upstream bug). Agents that answer with plain JSON
    // (or reject `message/stream`) fall through to the non-streaming branch,
    // which retries with `message/send`.
    let mut req_body = if file_parts.is_empty() {
        nasiko_types::a2a::build_stream_request(&effective_query, Some(context_id))
    } else {
        nasiko_types::a2a::build_stream_request_with_parts(
            &effective_query,
            Some(context_id),
            file_parts,
        )
    };
    append_attachment_note(&mut req_body, attachment_note.as_deref());
    append_org_policy_part(&mut req_body, org_context);
    append_current_time_part(&mut req_body, state.agent_clock());

    // No per-request MCP credential: the agent authenticates to /api/mcp with
    // its own deploy-time MCP_GATEWAY_TOKEN; the forwarded traceparent + the
    // flow_participants record written above carry the user binding.
    let build_agent_req = || {
        let mut req = state
            .agent_client
            .post(&target.url)
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .header(
                nasiko_agent_proxy::headers::HEADER_TRACEPARENT,
                crate::telemetry::traceparent_for(&flow_ctx),
            )
            // Agent turns can legitimately run long (long tool calls,
            // multi-step orchestration), so this hop carries the agent budget.
            .timeout(std::time::Duration::from_secs(
                state.config.agent_call_timeout_secs,
            ));
        // Whatever `forwarding_endpoint()` asked to have attached (the injection proxy's own
        // `Authorization` credential when memory is enabled; empty otherwise).
        for (name, value) in &target.headers {
            req = req.header(name, value);
        }
        req
    };

    // In flight until the agent's reply is complete. Starts as `Error`, so the early returns
    // below count as failures.
    let mut request_guard = state
        .genai_metrics
        .begin_agent_request(&agent.id.to_string());

    let response = build_agent_req()
        .json(&req_body)
        .send()
        .await
        .map_err(|e| A2aDispatchError::Internal(format!("agent request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // The agent's own body is logged, never returned: it is whatever that server emits for
        // an error — an HTML stack trace, in the case this was written for — and may name its
        // internals. The caller gets `agent_rejection_message` instead.
        tracing::warn!(
            agent = %agent.name,
            %status,
            body = %body.chars().take(512).collect::<String>(),
            "agent rejected the dispatch"
        );
        return Err(A2aDispatchError::AgentRejected {
            status,
            message: agent_rejection_message(status, &agent.name, !file_parts.is_empty()),
        });
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let task_id = task_id.to_string();
    let context_id = context_id.to_string();
    let flow_events = state.flow_events.clone();
    let mut flow_rx = state.flow_events.subscribe(&flow_id).await;
    let db = state.db.clone();
    let observability = state.observability.clone();
    let hitl_store = state.hitl_store.clone();
    let agent_id_for_hitl = agent.id;
    let agent_start = Instant::now();
    let call_timeout_secs = state.config.agent_call_timeout_secs;

    if content_type.contains("text/event-stream") {
        let byte_stream = response.bytes_stream();
        let spend_gate = state.spend_gate.clone();
        let flow_id_cleanup = flow_id;
        // Dropping the generator (client disconnect) skips the code after the relay loop, which
        // is the only place the outcome is settled.
        request_guard.set_outcome(RequestOutcome::Cancelled);

        let stream = async_stream::stream! {
            // Hold the dispatch span for the life of the stream so its duration
            // covers the full exchange. (The agent's reply streams through
            // opaque SSE frames here; the agent's own span records the output.)
            let _dispatch_span = dispatch_span;
            yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

            // Emit trace_id so the UI can link this response to its distributed trace.
            {
                let meta_msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                    "type": "trace_meta", "trace_id": flow_id_cleanup,
                })));
                yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, meta_msg))));
            }

            let mut buffer = String::new();
            let mut pinned = std::pin::pin!(byte_stream);
            let mut agent_done = false;
            let mut agent_error: Option<String> = None;
            let mut final_disposition = a2a::StreamDisposition::Continue;
            let mut pause_payload: Option<String> = None;
            // The task's last word, whichever way the stream ended: a pause or a dropped stream
            // leaves it open, a terminal event closes it (the reaper must not park mid-task).
            let mut last_task: Option<crate::agents::open_tasks::Observed> = None;
            // A direct agent narrates each tool call as a free-form "{tool}: {preview}"
            // Working-status message, never the structured tool_call data part the orchestrator
            // emits. These track the open tool so the loop can synthesise the structured
            // tool_call/tool_result parts the chat renders as tool rows — see
            // `tool_from_status_text`. `(turn seq, tool name, started-at)`.
            let mut open_tool: Option<(u64, String, Instant)> = None;
            let mut tool_seq: u64 = 0;

            loop {
                if agent_done { break; }

                tokio::select! {
                    biased;

                    chunk_result = pinned.next() => {
                        let Some(chunk_result) = chunk_result else {
                            agent_done = true;
                            continue;
                        };
                        let chunk = match chunk_result {
                            Ok(c) => c,
                            Err(e) => {
                                request_guard.set_outcome(RequestOutcome::Error);
                                // The reply was cut off, so the turn did not complete. Without a
                                // reason here the tail below would report it "completed".
                                if agent_error.is_none() {
                                    agent_error = Some(stream_cut_reason(&e, call_timeout_secs));
                                }
                                agent_done = true;
                                continue;
                            }
                        };
                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim_end_matches('\r').to_string();
                            buffer = buffer[line_end + 1..].to_string();

                            // `strip_prefix("data:")`, not `"data: "` — the space is spec-optional
                            // (a spec-legal `data:{...}` frame with no space was silently skipped
                            // here, dropping any pause/terminal event that happened to ride one).
                            if let Some(data) = line.strip_prefix("data:").map(str::trim) {
                                if data.is_empty() {
                                    continue;
                                }
                                if agent_error.is_none() {
                                    agent_error = extract_failure_message(data);
                                }
                                if let Some(observed) = serde_json::from_str::<serde_json::Value>(data)
                                    .ok()
                                    .and_then(|event| crate::agents::open_tasks::observe(&event))
                                {
                                    last_task = Some(observed);
                                }
                                let normalized = normalize_agent_event(data, &task_id, &context_id);
                                yield Ok(Event::default().data(normalized));
                                // Translate the agent's free-form tool narration into the
                                // structured tool_call/tool_result parts the chat renders as tool
                                // rows, so a direct chat streams tool activity like a routed one,
                                // with no change to the agents. The agent reports no per-tool
                                // completion, so the next tool call (or the terminal event below)
                                // closes the one before it.
                                if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                                    for sse in a2a::classify_sse_event(&event) {
                                        let a2a::SseEvent::StatusText(text) = &sse else { continue };
                                        let Some((name, preview)) = tool_from_status_text(text) else { continue };
                                        if let Some((seq, prev, started)) = open_tool.take() {
                                            let done = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                                "type": "tool_result", "tool": prev, "success": true,
                                                "turn": seq, "duration_ms": started.elapsed().as_millis() as u64,
                                            })));
                                            yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, done))));
                                        }
                                        tool_seq += 1;
                                        let call = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                                            "type": "tool_call", "tool": name, "message": preview, "turn": tool_seq,
                                        })));
                                        yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, call))));
                                        open_tool = Some((tool_seq, name.to_string(), Instant::now()));
                                    }
                                }
                                // The python a2a-sdk never closes its SSE stream;
                                // end the exchange on the task's terminal (or paused) event.
                                let disposition = a2a::classify_stream_disposition(data);
                                if disposition != a2a::StreamDisposition::Continue {
                                    agent_done = true;
                                    final_disposition = disposition;
                                    if disposition == a2a::StreamDisposition::Paused {
                                        pause_payload = Some(data.to_string());
                                    }
                                    break;
                                }
                            }
                        }
                    }

                    flow_event = flow_rx.recv() => {
                        let Ok(fe) = flow_event else { continue };
                        let msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(serde_json::to_value(&fe).unwrap_or_default()));
                        yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, msg))));
                    }
                }
            }

            // Close the last tool the agent never reported finishing: its terminal event or
            // final answer is the completion. Without this the row would hang "running".
            if let Some((seq, prev, started)) = open_tool.take() {
                let done = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                    "type": "tool_result", "tool": prev, "success": true,
                    "turn": seq, "duration_ms": started.elapsed().as_millis() as u64,
                })));
                yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, done))));
            }

            if let Some(observed) = &last_task {
                crate::agents::open_tasks::note_observed(&db, agent_id_for_hitl, observed).await;
            }

            // The agent's part is over; what follows is bookkeeping, not in-flight agent load.
            request_guard.settle(RequestOutcome::from_success(
                agent_error.is_none() && final_disposition != a2a::StreamDisposition::Failed,
            ));
            drop(request_guard);

            if final_disposition == a2a::StreamDisposition::Paused {
                // §8 (HITL plan): a Paused disposition is not "the task is done" — persist the
                // pause and stop relaying, without the completed/failed tail below.
                //
                // Ordering is the point here: the agent's own input-required/auth-required event
                // was already yielded above, inside the relay loop, before we knew whether a
                // `hitl_requests` row would even exist. This second, synthetic frame is yielded
                // ONLY after `persist_direct_chat_pause` returns `Ok` — i.e. only once the row is
                // durably committed and its id is known — so the frontend can never observe a
                // HITL id that doesn't yet exist in Postgres.
                let pause_data = pause_payload.as_deref().unwrap_or("{}");
                match persist_direct_chat_pause(
                    &hitl_store,
                    &db,
                    nasiko_hitl::HitlOrigin::DirectChat,
                    agent_id_for_hitl,
                    user_id,
                    &context_id,
                    &task_id,
                    session_id.as_deref(),
                    &flow_id_cleanup,
                    pause_data,
                )
                .await
                {
                    Ok(row) => {
                        yield Ok(build_hitl_stream_event(&hitl_store, &task_id, &context_id, &row, None).await);
                    }
                    Err(error_event) => {
                        yield Ok(error_event);
                    }
                }
                // Same cleanup every other terminal branch of this stream performs — a pause is
                // a terminal state for THIS flow_id (resume mints its own, see `hitl/mod.rs`), so
                // leaving the entry behind would orphan its broadcast::Sender in the process-wide
                // FlowEventBus map for the life of the server.
                flow_events.remove(&flow_id_cleanup).await;
            } else {
                // Terminal usage summary: duration always; tokens/cost only when the
                // agent's calls were platform-paid through the LLM gateway.
                {
                    let summary = super::usage_meta::summarize_flow_usage(
                        &db,
                        observability.as_ref(),
                        &flow_id_cleanup,
                        &super::usage_meta::TurnUsage::default(),
                        agent_start.elapsed().as_millis() as i64,
                    )
                    .await;
                    let usage_msg = a2a::agent_message(&context_id, &task_id,
                        a2a::data_part(summary.to_data_part(&flow_id_cleanup)));
                    yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, usage_msg))));
                }

                // A refused model call is why the turn failed, whatever the agent said about it.
                let agent_error = spend_gate.refusal_in_flow(&flow_id_cleanup).await.or(agent_error);
                if let Some(err) = &agent_error {
                    yield Ok(to_sse(a2a::status_event(a2a::failed(&task_id, &context_id, err))));
                } else {
                    yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));
                }

                // Guarded on 'running' like the other completion sites: a terminal
                // 'failed'/'paused' is never downgraded by a later completion. A turn that ended
                // in an error is recorded as failed, with the reason the user was shown.
                let _ = sqlx::query(
                    r#"UPDATE flows SET status = CASE WHEN $2::TEXT IS NULL THEN 'completed' ELSE 'failed' END,
                       error_message = $2,
                       duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
                       completed_at = now()
                       WHERE flow_id = $1 AND status = 'running'"#,
                ).bind(&flow_id_cleanup).bind(agent_error.as_deref()).execute(&db).await;
                flow_events.remove(&flow_id_cleanup).await;
            }
        };

        Ok(Sse::new(stream).into_response())
    } else {
        // Non-streaming JSON — wrap as A2A stream
        let mut resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| A2aDispatchError::Internal(format!("invalid agent JSON: {e}")))?;

        // A non-success first reply returned early above; the retry's status is checked below.
        let mut reply_status_ok = true;

        // The agent rejected `message/stream` (e.g. method not found) —
        // retry once with plain `message/send`, carrying the same attachments as the first try.
        if resp_body.get("error").is_some() {
            let mut retry_body = nasiko_types::a2a::build_send_request_with_parts(
                &effective_query,
                Some(&context_id),
                file_parts,
            );
            append_attachment_note(&mut retry_body, attachment_note.as_deref());
            append_org_policy_part(&mut retry_body, org_context);
            append_current_time_part(&mut retry_body, state.agent_clock());
            let retry = build_agent_req()
                .json(&retry_body)
                .send()
                .await
                .map_err(|e| A2aDispatchError::Internal(format!("agent request failed: {e}")))?;
            reply_status_ok = retry.status().is_success();
            resp_body = retry
                .json()
                .await
                .map_err(|e| A2aDispatchError::Internal(format!("invalid agent JSON: {e}")))?;
        }

        // A non-streaming reply can pause too (an agent that only implements
        // `message/send`, or whose `message/stream` fell back here) — §8 applies here just as
        // much as it does to the streaming branch; without this check the pause was silently
        // flattened into `extract_text`'s ordinary-reply fallback and lost entirely.
        // A transport or parse failure above returned early with the guard still `Error`.
        request_guard.set_outcome(non_streaming_reply_outcome(reply_status_ok, &resp_body));

        crate::agents::open_tasks::note_response(&db, agent_id_for_hitl, &resp_body).await;

        let raw_body = resp_body.to_string();
        if a2a::classify_stream_disposition(&raw_body) == a2a::StreamDisposition::Paused {
            let question_text = build_pause_question(&raw_body)["message"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let event = match pause_kind(&raw_body) {
                nasiko_hitl::HitlKind::AuthRequired => {
                    a2a::auth_required(&task_id, &context_id, &question_text)
                }
                _ => a2a::input_required(&task_id, &context_id, &question_text),
            };
            let flow_id_cleanup = flow_id.clone();
            let pause_result = persist_direct_chat_pause(
                &hitl_store,
                &db,
                nasiko_hitl::HitlOrigin::DirectChat,
                agent_id_for_hitl,
                user_id,
                &context_id,
                &task_id,
                session_id.as_deref(),
                &flow_id_cleanup,
                &raw_body,
            )
            .await;
            flow_events.remove(&flow_id_cleanup).await;

            // Same ordering as the streaming branch: the agent's own status event (`event`,
            // built above) is queued first, but the synthetic HITL metadata frame — the one
            // that actually carries `hitl_requests.id` — is only ever queued once persistence
            // has returned `Ok`, never before.
            let hitl_event = match pause_result.as_ref() {
                Ok(row) => Some(
                    build_hitl_stream_event(&hitl_store, &task_id, &context_id, row, None).await,
                ),
                Err(_) => None,
            };
            let error_event = pause_result.err();

            let stream = async_stream::stream! {
                yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));
                yield Ok(to_sse(a2a::status_event(event)));
                if let Some(hitl_event) = hitl_event {
                    yield Ok(hitl_event);
                }
                if let Some(error_event) = error_event {
                    yield Ok(error_event);
                }
            };
            return Ok(Sse::new(stream).into_response());
        }

        let text = nasiko_types::a2a::extract_text(resp_body.get("result").unwrap_or(&resp_body))
            .unwrap_or_else(|| "No response".into());

        if capture_content {
            dispatch_span.record(
                "gen_ai.output.messages",
                crate::telemetry::genai_text_message("assistant", &text).as_str(),
            );
        }

        let refusal = state.spend_gate.refusal_in_flow(&flow_id).await;
        let artifact_id = Uuid::new_v4().to_string();
        let flow_id_cleanup = flow_id;

        let stream = async_stream::stream! {
            yield Ok::<_, Infallible>(to_sse(a2a::status_event(a2a::working(&task_id, &context_id))));

            // Emit trace_id so the UI can link this response to its distributed trace.
            {
                let meta_msg = a2a::agent_message(&context_id, &task_id, a2a::data_part(json!({
                    "type": "trace_meta", "trace_id": flow_id_cleanup,
                })));
                yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, meta_msg))));
            }

            yield Ok(to_sse(a2a::artifact_event(a2a::text_chunk(
                &task_id, &context_id, &artifact_id, &text, false, true,
            ))));

            // Terminal usage summary — same contract as the streaming branch.
            {
                let summary = super::usage_meta::summarize_flow_usage(
                    &db,
                    observability.as_ref(),
                    &flow_id_cleanup,
                    &super::usage_meta::TurnUsage::default(),
                    agent_start.elapsed().as_millis() as i64,
                )
                .await;
                let usage_msg = a2a::agent_message(&context_id, &task_id,
                    a2a::data_part(summary.to_data_part(&flow_id_cleanup)));
                yield Ok(to_sse(a2a::status_event(a2a::working_with_message(&task_id, &context_id, usage_msg))));
            }

            if let Some(reason) = &refusal {
                yield Ok(to_sse(a2a::status_event(a2a::failed(&task_id, &context_id, reason))));
            } else {
                yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));
            }

            // Guarded on 'running' like the other completion sites: a terminal
            // 'failed'/'paused' is never downgraded by a later completion.
            let _ = sqlx::query(
                r#"UPDATE flows SET status = CASE WHEN $2::TEXT IS NULL THEN 'completed' ELSE 'failed' END,
                   error_message = $2,
                   duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
                   completed_at = now()
                   WHERE flow_id = $1 AND status = 'running'"#,
            ).bind(&flow_id_cleanup).bind(refusal.as_deref()).execute(&db).await;
            flow_events.remove(&flow_id_cleanup).await;
        };

        Ok(Sse::new(stream).into_response())
    }
}

// ─── Router Stats ─────────────────────────────────────────────────────────────

/// One row of `RouterStatsResponse` — mirrors the ad hoc `serde_json::json!`
/// object the handler builds per row (numeric averages are stringified to
/// preserve `rust_decimal` precision through JSON).
#[derive(Serialize, ToSchema)]
pub(crate) struct RouterStatsRow {
    agent_name: Option<String>,
    selection_count: Option<i64>,
    successful_calls: Option<i64>,
    failed_calls: Option<i64>,
    avg_agent_latency_ms: Option<String>,
    avg_selection_latency_ms: Option<String>,
    avg_stage1_candidates: Option<String>,
    avg_stage2_candidates: Option<String>,
    date: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RouterStatsResponse {
    data: Vec<RouterStatsRow>,
    total: usize,
}

/// Aggregated agent-selection stats from the `agent_selection_stats`
/// materialized view (newest date, most-selected agent first; max 200 rows).
#[utoipa::path(
    get,
    path = "/api/orchestrator/stats",
    tag = "orchestrator",
    responses(
        (status = 200, description = "Routing/selection stats", body = RouterStatsResponse),
    ),
)]
pub async fn router_stats_handler(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    let rows = sqlx::query_as::<_, StatsRow>(
        r#"SELECT
            selected_agent_name AS agent_name,
            selection_count,
            successful_calls,
            failed_calls,
            avg_agent_latency_ms,
            avg_selection_latency_ms,
            avg_stage1_candidates,
            avg_stage2_candidates,
            date::text AS date
        FROM agent_selection_stats
        ORDER BY date DESC, selection_count DESC
        LIMIT 200"#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(%e, "router_stats_handler: db error");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        )
    })?;

    let data: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "agent_name":              r.agent_name,
                "selection_count":         r.selection_count,
                "successful_calls":        r.successful_calls,
                "failed_calls":            r.failed_calls,
                "avg_agent_latency_ms":    r.avg_agent_latency_ms.map(|v| v.to_string()),
                "avg_selection_latency_ms":r.avg_selection_latency_ms.map(|v| v.to_string()),
                "avg_stage1_candidates":   r.avg_stage1_candidates.map(|v| v.to_string()),
                "avg_stage2_candidates":   r.avg_stage2_candidates.map(|v| v.to_string()),
                "date":                    r.date,
            })
        })
        .collect();

    Ok(axum::Json(
        serde_json::json!({ "data": data, "total": data.len() }),
    ))
}

#[derive(sqlx::FromRow)]
struct StatsRow {
    agent_name: Option<String>,
    selection_count: Option<i64>,
    successful_calls: Option<i64>,
    failed_calls: Option<i64>,
    avg_agent_latency_ms: Option<rust_decimal::Decimal>,
    avg_selection_latency_ms: Option<rust_decimal::Decimal>,
    avg_stage1_candidates: Option<rust_decimal::Decimal>,
    avg_stage2_candidates: Option<rust_decimal::Decimal>,
    date: Option<String>,
}

// ─── Upload Handler ───────────────────────────────────────────────────────────

/// `POST /api/a2a/upload` — multipart/form-data A2A dispatch entry point.
///
/// Accepts:
/// - `query`   (text field, required) — the user's question
/// - Any number of additional fields treated as file attachments
///
/// Each file is base64-encoded and forwarded alongside the query to the orchestrator.
#[utoipa::path(
    post,
    path = "/api/orchestrator/a2a/upload",
    tag = "orchestrator",
    request_body(content = A2aUploadForm, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "A2A event stream (status/artifact updates, completion)", content_type = "text/event-stream"),
        (status = 400, description = "Missing/invalid multipart, or an empty query field", body = JsonRpcErrorResponse),
        (status = 500, description = "Internal error", body = JsonRpcErrorResponse),
        (status = 503, description = "No agents available", body = JsonRpcErrorResponse),
    ),
)]
pub async fn a2a_upload_handler(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart,
) -> Result<Response, A2aDispatchError> {
    let mut query = String::new();
    let mut collected_files: Vec<nasiko_types::a2a::Part> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| A2aDispatchError::InvalidRequest(format!("multipart error: {e}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        let file_name = field.file_name().map(|s| s.to_string());
        let content_type = field.content_type().map(|s| s.to_string());

        let bytes = field
            .bytes()
            .await
            .map_err(|e| A2aDispatchError::InvalidRequest(format!("field read error: {e}")))?;

        if field_name == "query" {
            query = String::from_utf8(bytes.to_vec()).map_err(|_| {
                A2aDispatchError::InvalidRequest("query must be valid UTF-8".into())
            })?;
        } else {
            collected_files.push(nasiko_types::a2a::file_part(
                bytes.to_vec(),
                file_name.or(Some(field_name)),
                content_type,
            ));
        }
    }
    let file_count = collected_files.len();

    if query.trim().is_empty() {
        return Err(A2aDispatchError::InvalidRequest(
            "multipart must include a non-empty 'query' text field".into(),
        ));
    }

    let task_id = Uuid::new_v4().to_string();
    let context_id = Uuid::new_v4().to_string();
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return Ok(e.into_response()),
    };

    tracing::info!(
        user_id = %user_id,
        file_count,
        "a2a dispatch upload: orchestrating query with {} file(s)",
        file_count
    );

    // `query`/`raw_text` are the same value here — this multipart path always
    // mints a fresh `context_id` above, so there's no prior history to fetch
    // or enrich `query` with in the first place.
    orchestrator_stream(
        &state,
        OrchestratorTurn {
            query: &query,
            raw_text: &query,
            task_id: &task_id,
            context_id: &context_id,
            user_id,
            is_superuser: claims.is_superuser,
            // Fresh context id, minted here — no client-side session owns
            // this transcript, so the server persists the turn.
            client_owns_transcript: false,
            transcript_role: "user",
            file_parts: collected_files,
            attached_files: Vec::new(),
            kind: TurnKind::User,
            // This path mints a fresh context id, so no history was selected and there is nothing
            // to report. A zero row here would count as a request that optimized nothing.
            context_report: None,
        },
    )
    .await
}

// ─── Inbound attachments ─────────────────────────────────────────────────────

/// Attachments honoured per turn, matching `chat::routes::MAX_FILES_PER_UPLOAD` — the most one
/// upload call can produce, so a client that uploads and sends in one go never hits this.
const MAX_ATTACHED_FILES: usize = 10;

/// Total attachment bytes forwarded in one turn. These are base64'd into the A2A message body
/// (`PartContent::Raw`), so the budget is what we are willing to inflate by 4/3 and push to an
/// agent in a single request — well above what multimodal models accept per call (Anthropic caps
/// a PDF at 32 MB, OpenAI an audio file at 25 MB). Over it, the turn is refused with a message
/// the user can act on rather than quietly reaching the agent without the file it was about.
const MAX_ATTACHED_BYTES: u64 = 30 * 1024 * 1024;
const ATTACHMENT_READ_CONCURRENCY: usize = 4;

/// What the caller attached to this turn, in the two forms this endpoint accepts.
///
/// - **Inline `raw` parts** on the message — a CLI or API caller carrying the bytes itself.
///   Bounded by axum's blanket 2 MiB body limit, which this route doesn't lift.
/// - **`metadata.file_ids`** naming rows already uploaded through
///   `POST /api/chat/sessions/{id}/files`. The browser path: the bytes are read from storage
///   here instead of riding in the JSON-RPC body, which is the only way an attachment larger
///   than that same 2 MiB limit can be sent at all.
///
/// Ids resolve only through a session the caller owns, so one user can never attach another's
/// upload by guessing a UUID, and only through object storage. Order follows the caller's list.
async fn inbound_file_parts(
    state: &AppState,
    params: &nasiko_types::a2a::SendMessageRequest,
    session_id: Option<&str>,
    user_id: Uuid,
) -> Result<InboundAttachments, A2aDispatchError> {
    let mut parts: Vec<a2a::Part> = params
        .message
        .parts
        .iter()
        .filter(|p| matches!(p.content, PartContent::Raw(_)))
        .cloned()
        .collect();
    let inline_bytes: u64 = parts
        .iter()
        .map(|p| match &p.content {
            PartContent::Raw(data) => data.len() as u64,
            _ => 0,
        })
        .sum();

    let ids = listed_file_ids(params)?;
    if ids.len() + parts.len() > MAX_ATTACHED_FILES {
        return Err(A2aDispatchError::InvalidRequest(format!(
            "at most {MAX_ATTACHED_FILES} attachments per message"
        )));
    }
    if ids.is_empty() {
        check_attached_total(inline_bytes)?;
        return Ok(InboundAttachments {
            parts,
            files: Vec::new(),
        });
    }

    let rows = sqlx::query_as::<_, AttachedRow>(
        "SELECT cmf.id, cmf.session_id, cmf.filename, cmf.mime_type, cmf.size_bytes, \
                cmf.storage_uri \
           FROM chat_message_files cmf \
           JOIN chat_sessions cs ON cs.session_id = cmf.session_id \
          WHERE cmf.id = ANY($1) AND cs.user_id = $2 \
            AND ($3::text IS NULL OR cmf.session_id = $3)",
    )
    .bind(&ids)
    .bind(user_id)
    .bind(session_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| A2aDispatchError::Internal(format!("attachment lookup failed: {e}")))?;

    let mut by_id: std::collections::HashMap<Uuid, AttachedRow> =
        rows.into_iter().map(|row| (row.id, row)).collect();
    let mut resolved = Vec::with_capacity(ids.len());
    let mut stored_bytes = 0u64;
    for id in &ids {
        let Some(row) = by_id.remove(id) else {
            return Err(A2aDispatchError::InvalidRequest(format!(
                "attachment {id} was not found"
            )));
        };
        let attached = if row.storage_uri.starts_with(CHAT_UPLOAD_PREFIX) {
            Resolved::ChatUpload(row)
        } else {
            Resolved::Elsewhere(elsewhere_attachment(state, user_id, row).await?)
        };
        stored_bytes += attached.size_bytes();
        resolved.push(attached);
    }
    check_attached_total(inline_bytes + stored_bytes)?;

    let fetched: Vec<Fetched> =
        futures::stream::iter(resolved.into_iter().map(|attached| async move {
            match attached {
                Resolved::ChatUpload(row) => {
                    let read = state.oci_storage.get_blob(&row.storage_uri).await;
                    Fetched::ChatUpload(row, read)
                }
                Resolved::Elsewhere(file) => Fetched::Elsewhere(file),
            }
        }))
        .buffered(ATTACHMENT_READ_CONCURRENCY)
        .collect()
        .await;

    let mut files = Vec::with_capacity(fetched.len());
    for attached in fetched {
        let (row, read) = match attached {
            Fetched::ChatUpload(row, read) => (row, read),
            Fetched::Elsewhere(file) => {
                files.push(file);
                continue;
            }
        };
        match read {
            Ok(bytes) => parts.push(a2a::file_part(
                Vec::from(bytes),
                Some(row.filename.clone()),
                Some(row.mime_type.clone()),
            )),
            Err(nasiko_runtime::BlobStoreError::NotFound(_)) => {
                return Err(A2aDispatchError::InvalidRequest(format!(
                    "attachment '{}' is no longer stored",
                    row.filename
                )));
            }
            Err(e) => {
                tracing::warn!(id = %row.id, %e, "attachment bytes unreadable");
                return Err(A2aDispatchError::Unavailable(format!(
                    "attachment '{}' could not be read; try again",
                    row.filename
                )));
            }
        }
        files.push(row.into_chat_upload());
    }

    Ok(InboundAttachments { parts, files })
}

/// The stored uploads `metadata.file_ids` names, without repeats, in the caller's order.
fn listed_file_ids(
    params: &nasiko_types::a2a::SendMessageRequest,
) -> Result<Vec<Uuid>, A2aDispatchError> {
    let mut ids: Vec<Uuid> = Vec::new();
    let listed = params
        .metadata
        .as_ref()
        .and_then(|m| m.get("file_ids"))
        .and_then(|v| v.as_array());
    for value in listed.into_iter().flatten() {
        let raw = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        let id = Uuid::parse_str(&raw).map_err(|_| {
            A2aDispatchError::InvalidRequest(format!("attachment {raw} was not found"))
        })?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Put the files attached on earlier turns of `session_id` ahead of this turn's own.
///
/// The history a turn carries is text, so an agent that keeps no conversation of its own would
/// otherwise meet a follow-up question about a PDF without the PDF. Only files a saved user
/// message claimed count (an upload dropped before sending never does). They are taken newest
/// first while they fit what this turn's own files leave of `MAX_ATTACHED_FILES` and
/// `MAX_ATTACHED_BYTES`, and sent in the order the user sent them. Best effort: a file that can
/// no longer be read is left out with a warning, rather than refusing every later turn of the
/// chat over a file the user did not send with it.
async fn carry_earlier_attachments(
    state: &AppState,
    session_id: &str,
    user_id: Uuid,
    params: &nasiko_types::a2a::SendMessageRequest,
    turn: &mut InboundAttachments,
) {
    // Already validated by `inbound_file_parts`; listed here only to leave them out.
    let this_turn = listed_file_ids(params).unwrap_or_default();
    let rows = sqlx::query_as::<_, AttachedRow>(
        "SELECT cmf.id, cmf.session_id, cmf.filename, cmf.mime_type, cmf.size_bytes, \
                cmf.storage_uri \
           FROM chat_message_files cmf \
           JOIN chat_sessions cs ON cs.session_id = cmf.session_id \
          WHERE cmf.session_id = $1 AND cs.user_id = $2 \
            AND cmf.message_id IS NOT NULL AND cmf.id <> ALL($3) \
          ORDER BY cmf.created_at DESC, cmf.id",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(&this_turn)
    .fetch_all(&state.db)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(%e, "earlier attachments lookup failed; the turn goes without them");
            return;
        }
    };

    let (mut room_files, mut room_bytes) = turn.room_left();
    let mut chosen = Vec::new();
    for row in rows {
        if room_files == 0 {
            break;
        }
        let size = row.size_bytes.max(0) as u64;
        if size > room_bytes {
            continue;
        }
        room_files -= 1;
        room_bytes -= size;
        chosen.push(row);
    }
    chosen.reverse();

    let carried: Vec<_> = futures::stream::iter(
        chosen
            .into_iter()
            .map(|row| carried_attachment(state, user_id, row)),
    )
    .buffered(ATTACHMENT_READ_CONCURRENCY)
    .collect()
    .await;
    let (parts, files): (Vec<_>, Vec<_>) = carried.into_iter().flatten().unzip();
    turn.parts.splice(0..0, parts.into_iter().flatten());
    turn.files.splice(0..0, files);
}

/// One earlier file read for this turn: its bytes as a part when it is a chat upload, and what
/// the agent is told about it. `None`, with a warning, when it can no longer be read.
async fn carried_attachment(
    state: &AppState,
    user_id: Uuid,
    row: AttachedRow,
) -> Option<(Option<a2a::Part>, AttachedFile)> {
    let id = row.id;
    if !row.storage_uri.starts_with(CHAT_UPLOAD_PREFIX) {
        return match elsewhere_attachment(state, user_id, row).await {
            Ok(file) => Some((None, file)),
            Err(e) => {
                tracing::warn!(%id, error = ?e, "an earlier attachment is left out of this turn");
                None
            }
        };
    }
    match state.oci_storage.get_blob(&row.storage_uri).await {
        Ok(bytes) => Some((
            Some(a2a::file_part(
                Vec::from(bytes),
                Some(row.filename.clone()),
                Some(row.mime_type.clone()),
            )),
            row.into_chat_upload(),
        )),
        Err(e) => {
            tracing::warn!(%id, %e, "an earlier attachment is left out of this turn");
            None
        }
    }
}

enum Resolved {
    ChatUpload(AttachedRow),
    Elsewhere(AttachedFile),
}

enum Fetched {
    ChatUpload(
        AttachedRow,
        Result<bytes::Bytes, nasiko_runtime::BlobStoreError>,
    ),
    Elsewhere(AttachedFile),
}

impl Resolved {
    fn size_bytes(&self) -> u64 {
        match self {
            Self::ChatUpload(row) => row.size_bytes.max(0) as u64,
            Self::Elsewhere(file) => file.size_bytes,
        }
    }
}

async fn elsewhere_attachment(
    state: &AppState,
    user_id: Uuid,
    row: AttachedRow,
) -> Result<AttachedFile, A2aDispatchError> {
    let source = state.turn_artifacts.get();
    match source
        .attachment(&state.db, user_id, &row.storage_uri)
        .await
    {
        Attachable::File(file) => Ok(AttachedFile {
            reference: file.reference,
            filename: row.filename,
            media_type: file.media_type,
            size_bytes: file.size_bytes,
        }),
        Attachable::Refused => Err(A2aDispatchError::InvalidRequest(format!(
            "'{}' can't be attached to a message",
            row.filename
        ))),
        Attachable::Unavailable => {
            tracing::warn!(id = %row.id, "attachment source unavailable");
            Err(A2aDispatchError::Unavailable(format!(
                "attachment '{}' could not be read; try again",
                row.filename
            )))
        }
    }
}

#[derive(sqlx::FromRow)]
struct AttachedRow {
    id: Uuid,
    session_id: String,
    filename: String,
    mime_type: String,
    size_bytes: i64,
    storage_uri: String,
}

impl AttachedRow {
    /// A chat upload as the agent is told about it.
    fn into_chat_upload(self) -> AttachedFile {
        AttachedFile {
            reference: AttachmentRef::ChatUpload {
                session_id: self.session_id,
                file_id: self.id,
            },
            filename: self.filename,
            media_type: self.mime_type,
            size_bytes: self.size_bytes.max(0) as u64,
        }
    }
}

pub(crate) struct InboundAttachments {
    pub(crate) parts: Vec<a2a::Part>,
    pub(crate) files: Vec<AttachedFile>,
}

impl InboundAttachments {
    /// What these files leave of the per-turn limits: a count, then bytes. A chat upload is in
    /// both lists (its bytes in `parts`, its reference in `files`), an inline part only in
    /// `parts`, and a file read from elsewhere only in `files`.
    fn room_left(&self) -> (usize, u64) {
        let inline_bytes: u64 = self
            .parts
            .iter()
            .map(|p| match &p.content {
                PartContent::Raw(data) => data.len() as u64,
                _ => 0,
            })
            .sum();
        let (count, bytes) = self
            .files
            .iter()
            .filter(|f| !matches!(f.reference, AttachmentRef::ChatUpload { .. }))
            .fold((self.parts.len(), inline_bytes), |(n, b), f| {
                (n + 1, b + f.size_bytes)
            });
        (
            MAX_ATTACHED_FILES.saturating_sub(count),
            MAX_ATTACHED_BYTES.saturating_sub(bytes),
        )
    }
}

pub(crate) struct AttachedFile {
    reference: AttachmentRef,
    filename: String,
    media_type: String,
    size_bytes: u64,
}

fn attachment_note(
    signer: Option<&MarkerSigner>,
    files: &[AttachedFile],
    flow_id: &str,
) -> Option<String> {
    let signer = signer?;
    if files.is_empty() {
        return None;
    }
    let expires_at = chrono::Utc::now().timestamp() + MARKER_TTL_SECS;
    let lines: Vec<String> = files
        .iter()
        .map(|file| {
            signer.marker_line(&MarkerClaims {
                reference: file.reference.clone(),
                filename: file.filename.clone(),
                media_type: file.media_type.clone(),
                size_bytes: file.size_bytes,
                flow_id: flow_id.to_string(),
                expires_at,
            })
        })
        .collect();
    Some(lines.join("\n"))
}

fn with_attachment_list<'q>(
    query: &'q str,
    parts: &[a2a::Part],
    files: &[AttachedFile],
) -> std::borrow::Cow<'q, str> {
    let inline = parts.iter().filter_map(|part| match &part.content {
        PartContent::Raw(data) => Some(describe_file(
            part.filename.as_deref().unwrap_or("unnamed"),
            part.media_type
                .as_deref()
                .unwrap_or("application/octet-stream"),
            data.len() as u64,
        )),
        _ => None,
    });
    let chips = files
        .iter()
        .filter(|file| matches!(file.reference, AttachmentRef::Workspace { .. }))
        .map(|file| describe_file(&file.filename, &file.media_type, file.size_bytes));
    let listed: Vec<String> = inline.chain(chips).collect();
    if listed.is_empty() {
        return std::borrow::Cow::Borrowed(query);
    }
    std::borrow::Cow::Owned(format!(
        "{query}\n\n[The user attached: {}]",
        listed.join(", ")
    ))
}

fn append_attachment_note(req: &mut JsonRpcRequest, note: Option<&str>) {
    let Some(note) = note else { return };
    let tagging = nasiko_agent_proxy::context_part::tagging_for_method(&req.method);
    let Some(message) = req.params.as_mut().and_then(|p| p.get_mut("message")) else {
        return;
    };
    let _ = nasiko_agent_proxy::context_part::append_user_text(message, note, tagging);
}

const CHAT_UPLOAD_PREFIX: &str = "chat-files/";

fn check_attached_total(total: u64) -> Result<(), A2aDispatchError> {
    if total > MAX_ATTACHED_BYTES {
        return Err(A2aDispatchError::InvalidRequest(format!(
            "attachments total {:.1} MB, over the {} MB per-message limit",
            total as f64 / (1024.0 * 1024.0),
            MAX_ATTACHED_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// What the caller is told when an agent answers a dispatch with an error status. The agent's own
/// body never reaches them (it is that server's error page, not ours), so this line has to carry
/// enough on its own to act on.
///
/// `413` is named explicitly because it has one overwhelmingly common cause with one obvious
/// remedy: attachments ride inside the A2A message base64'd (~4/3 of the file), and a Node agent
/// built on Express caps a JSON body at `body-parser`'s 100 KB default — two orders of magnitude
/// below [`MAX_ATTACHED_BYTES`], which is what this platform is willing to forward. Nothing on
/// our side can lift that cap, so the only honest advice is a smaller file or a change in the
/// agent. Every other status is a generic upstream refusal.
fn agent_rejection_message(status: StatusCode, agent: &str, had_attachments: bool) -> String {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        return if had_attachments {
            format!(
                "agent '{agent}' refused this message as too large — its HTTP server caps the \
                 request body below the size of the attachment. Send a smaller file, or raise \
                 that limit in the agent."
            )
        } else {
            format!("agent '{agent}' refused this message as too large")
        };
    }
    format!(
        "agent '{agent}' rejected this request (HTTP {})",
        status.as_u16()
    )
}

fn to_sse(event: StreamResponse) -> Event {
    Event::default().data(a2a::to_sse_data(&event))
}

/// Whether `session_id` names an existing chat session that belongs to a user other than
/// `user_id`. Superusers own every session for this purpose.
async fn session_owned_by_other(
    state: &AppState,
    session_id: &str,
    user_id: Uuid,
    is_superuser: bool,
) -> Result<bool, A2aDispatchError> {
    if is_superuser {
        return Ok(false);
    }
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM chat_sessions WHERE session_id = $1")
            .bind(session_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| A2aDispatchError::Internal(format!("session lookup failed: {e}")))?;
    Ok(owner.is_some_and(|owner| owner != user_id))
}

/// Upsert a `chat_sessions` row and persist the user's message for an
/// orchestrator-routed chat, mirroring what `agent_proxy.rs`'s
/// `ensure_chat_session` already does for direct agent chat. `agent_id` is
/// left `NULL` (unlike agent_proxy's fixed target) since the orchestrator
/// can route to a different agent on every turn of the same session.
/// Fire-and-forget: session bookkeeping here is a nice-to-have for
/// `sessions`/`history`/resume. It does not decide access: callers do (the
/// chat entry point, `a2a_dispatch_handler`, refuses another user's session
/// with `session_owned_by_other` before it gets here).
/// `client_owns_transcript` suppresses the message insert for callers that
/// persist their own turns via `/api/chat/sessions/{id}/messages` (the web UI,
/// identified by an explicit `metadata.session_id`). It used to be harmless
/// only because the UI sent a throwaway random `contextId`, so the two writes
/// landed in different sessions; now that contextId *is* the session id, doing
/// both would store every user message twice.
async fn ensure_orchestrator_chat_session(
    state: &AppState,
    context_id: &str,
    user_id: Uuid,
    query: &str,
    role: &str,
    client_owns_transcript: bool,
) {
    let title = {
        let t = query.trim();
        if t.is_empty() {
            "New chat".to_string()
        } else if t.len() > 60 {
            let mut n = 60;
            while !t.is_char_boundary(n) {
                n -= 1;
            }
            format!("{}…", &t[..n])
        } else {
            t.to_string()
        }
    };

    let _ = sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title, session_type) \
         VALUES ($1, $2, NULL, '/api/orchestrator/a2a', $3, 'orchestrator') \
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(context_id)
    .bind(user_id)
    .bind(&title)
    .execute(&state.db)
    .await;

    if client_owns_transcript {
        return;
    }

    // 10s dedup guard: mirrors the one in agent_proxy.rs — the CLI's A2A
    // method negotiation can hit this path twice for the same logical
    // message when it retries under a different JSON-RPC method name.
    let _ = sqlx::query(
        "INSERT INTO chat_messages (session_id, role, content) \
         SELECT $1, $2, $3 WHERE NOT EXISTS ( \
             SELECT 1 FROM chat_messages \
             WHERE session_id = $1 AND role = $2 AND content = $3 \
               AND timestamp > now() - INTERVAL '10 seconds')",
    )
    .bind(context_id)
    .bind(role)
    .bind(query)
    .execute(&state.db)
    .await;
}

/// Answer a task request addressed (by `params.metadata.agent_id`, a name or a UUID) to a harness
/// agent. `None` when it is not addressed to one, so the request falls through to the message
/// handling as before.
async fn harness_task_reply(
    state: &AppState,
    claims: &Claims,
    method: crate::sessions::tasks::TaskMethod,
    req: &JsonRpcRequest,
) -> Result<Option<Response>, A2aDispatchError> {
    let rpc = serde_json::to_value(req).unwrap_or(serde_json::Value::Null);
    let Some(target) = rpc
        .pointer("/params/metadata/agent_id")
        .and_then(|v| v.as_str())
    else {
        return Ok(None);
    };
    // "orchestrator" is the router, not an agent: a task method for it falls through as before.
    if target == "orchestrator" {
        return Ok(None);
    }
    let agent = resolve_agent(state, target).await?;
    // The same answer as an unknown agent when the caller may not see this one.
    if !crate::acl::can_access_agent(state, claims, agent.id).await {
        return Err(A2aDispatchError::AgentNotFound(target.to_string()));
    }
    let is_harness = agent.may_drive_code()
        && matches!(
            crate::coding::proxy::load_agent_target(&state.db, agent.id)
                .await
                .map_err(|e| A2aDispatchError::Internal(e.to_string()))?,
            Some(crate::coding::proxy::AgentTarget::Harness(_))
        );
    if !is_harness {
        return Ok(None);
    }
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return Ok(Some(e.into_response())),
    };
    Ok(Some(
        crate::sessions::tasks::handle(state, agent.id, user_id, method, &rpc).await,
    ))
}

/// What the dispatch path knows about one turn to a code-driving agent.
struct CodingDispatch<'a> {
    agent: &'a AgentRow,
    /// History-packed text — recorded on the flow (`flows.title`) like `agent_stream`.
    query: &'a str,
    /// The raw user message — the turn's prompt and what's persisted to the transcript.
    user_message: &'a str,
    context_id: &'a str,
    task_id: &'a str,
    user_id: Uuid,
}

/// Run a coding-agent turn for the orchestrator-dispatch path and stream it back
/// as A2A SSE. The dispatch path has no coding awareness of its own, so this does
/// the minimal pre-turn bookkeeping the coding turn needs — a `chat_sessions` row
/// (the lease keys on it), a `flows` row + participant (so the session token is
/// authorizable), and the user message — then hands off to the shared
/// `coding::entry::run_coding_turn_sse`, exactly as `agent_proxy` does (NAS-C1).
async fn coding_agent_stream(
    state: &AppState,
    target: crate::coding::proxy::CodingAgentTarget,
    turn: CodingDispatch<'_>,
    client_owns_transcript: bool,
) -> Result<Response, A2aDispatchError> {
    // Session row (the coding lease UPDATEs it) + user message. `agent_id` is set,
    // so the session is tied to this coding agent for runtime reconcile + the UI.
    ensure_coding_chat_session(
        state,
        turn.context_id,
        turn.user_id,
        turn.agent.id,
        turn.user_message,
        client_owns_transcript,
    )
    .await;
    let flow_id = open_coding_flow(state, &turn).await;

    crate::coding::entry::run_coding_turn_sse(
        state,
        turn.agent.id,
        &target,
        Some(turn.user_id),
        turn.context_id,
        turn.user_message,
        &flow_id,
        turn.task_id,
    )
    .await
    .map_err(|e| match e {
        crate::coding::turn::CodingTurnError::NotConfigured(message) => {
            A2aDispatchError::Unavailable(message)
        }
        e => {
            tracing::error!(error = %e, agent_id = %turn.agent.id, "coding-agent dispatch turn failed to start");
            A2aDispatchError::Internal(e.to_string())
        }
    })
}

/// Run a harness-session turn for the dispatch path. Resolving the session comes first and is
/// the ownership check: a contextId owned by someone else is `SessionNotFound`, and nothing is
/// written under it (no transcript row, no flow).
async fn harness_agent_stream(
    state: &AppState,
    target: crate::coding::proxy::HarnessTarget,
    turn: CodingDispatch<'_>,
    client_owns_transcript: bool,
    request_id: &a2a::JsonRpcId,
) -> Result<Response, A2aDispatchError> {
    // Before the session, the transcript and the flow are written: a refused turn leaves none.
    if let Some(refusal) = refuse_session(state, turn.user_id, &target.harness_id).await {
        return Err(A2aDispatchError::SessionRefused {
            refusal,
            id: serde_json::to_value(request_id).unwrap_or(serde_json::Value::Null),
        });
    }
    let session = crate::sessions::SessionStore::new(state.db.clone())
        .find_or_create_for_harness(turn.agent.id, turn.user_id, turn.context_id)
        .await
        .map_err(|e| A2aDispatchError::Internal(e.to_string()))?
        .ok_or(A2aDispatchError::SessionNotFound)?;
    ensure_coding_chat_session(
        state,
        turn.context_id,
        turn.user_id,
        turn.agent.id,
        turn.user_message,
        client_owns_transcript,
    )
    .await;
    let flow_id = open_coding_flow(state, &turn).await;

    crate::sessions::turn::run_harness_turn_sse(
        state,
        crate::sessions::turn::HarnessTurnRequest {
            target: &target,
            session,
            user_id: turn.user_id,
            prompt: turn.user_message,
            flow_id: &flow_id,
            task_id: turn.task_id,
        },
    )
    .await
    .map_err(|e| match e {
        crate::coding::turn::CodingTurnError::Busy
        | crate::coding::turn::CodingTurnError::Superseded => A2aDispatchError::SessionBusy,
        crate::coding::turn::CodingTurnError::NotConfigured(message) => {
            A2aDispatchError::Unavailable(message)
        }
        e => {
            tracing::error!(error = %e, agent_id = %turn.agent.id, "harness dispatch turn failed to start");
            A2aDispatchError::Internal(e.to_string())
        }
    })
}

/// Open the flow a code-driving turn's session token binds to — same shape as `agent_stream`'s
/// own flow setup, so the gateway/router authorize the agent's calls off the signed claims
/// instead of a traceparent. Returns the flow id.
async fn open_coding_flow(state: &AppState, turn: &CodingDispatch<'_>) -> String {
    let dispatch_span = tracing::info_span!(
        "a2a.dispatch",
        otel.kind = "server",
        gen_ai.operation.name = "invoke_agent",
        gen_ai.agent.name = %turn.agent.name,
        session.id = %turn.context_id,
    );
    let flow_ctx = crate::telemetry::flow_context_from_span(&dispatch_span)
        .unwrap_or_else(FlowContext::new_root);
    let flow_id = flow_ctx.flow_id.clone();
    state
        .flow_guard
        .init_flow(&flow_ctx, &turn.agent.name)
        .await;
    let flow_metadata = json!({ "context_id": turn.context_id, "user_message": turn.user_message });
    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, status, metadata, session_id)
           VALUES ($1, $2, $3, $4, $5, 'running', $6, $7)
           ON CONFLICT (flow_id) DO UPDATE SET status = 'running', completed_at = NULL"#,
    )
    .bind(&flow_id)
    .bind(turn.user_id)
    .bind(turn.agent.id)
    .bind(&turn.agent.name)
    .bind(turn.query)
    .bind(&flow_metadata)
    .bind(turn.context_id)
    .execute(&state.db)
    .await;
    crate::flows::record_participant(&state.db, &flow_id, turn.agent.id).await;
    flow_id
}

/// Upsert the `chat_sessions` row for a coding-agent dispatch turn (with `agent_id`
/// set, unlike [`ensure_orchestrator_chat_session`]) and persist the user message
/// unless the client owns its transcript. Fire-and-forget bookkeeping.
async fn ensure_coding_chat_session(
    state: &AppState,
    context_id: &str,
    user_id: Uuid,
    agent_id: Uuid,
    user_message: &str,
    client_owns_transcript: bool,
) {
    let title = {
        let t = user_message.trim();
        if t.is_empty() {
            "New chat".to_string()
        } else if t.len() > 60 {
            let mut n = 60;
            while !t.is_char_boundary(n) {
                n -= 1;
            }
            format!("{}…", &t[..n])
        } else {
            t.to_string()
        }
    };

    let _ = sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(context_id)
    .bind(user_id)
    .bind(agent_id)
    .bind(format!("/api/agents/{agent_id}"))
    .bind(&title)
    .execute(&state.db)
    .await;

    if client_owns_transcript {
        return;
    }
    // 10s dedup guard, same as the other transcript writers.
    let _ = sqlx::query(
        "INSERT INTO chat_messages (session_id, role, content) \
         SELECT $1, 'user', $2 WHERE NOT EXISTS ( \
             SELECT 1 FROM chat_messages \
             WHERE session_id = $1 AND role = 'user' AND content = $2 \
               AND timestamp > now() - INTERVAL '10 seconds')",
    )
    .bind(context_id)
    .bind(user_message)
    .execute(&state.db)
    .await;
}

/// Not a real address: the prefix of the endpoint a routed turn's candidate carries until its leg is
/// resolved by id in `CpCallGuard::before_call`.
const LEG_RESOLVED_ENDPOINT_PREFIX: &str = "nasiko-leg://";

/// Normalize a Python a2a-sdk JSONRPC event to CP native StreamResponse format.
/// Extract the error message if this SSE event represents a TASK_STATE_FAILED status update.
/// The per-agent metrics outcome of a non-streaming agent reply: a success status, no JSON-RPC
/// `error`, and a task that did not end failed. The task state is read with the same wire-shape
/// classifier the streaming relay ends its exchange on, which (unlike `extract_failure_message`)
/// also reads the task-wrapped reply a non-streaming agent sends.
fn non_streaming_reply_outcome(status_ok: bool, body: &serde_json::Value) -> RequestOutcome {
    let task_failed =
        a2a::classify_stream_disposition(&body.to_string()) == a2a::StreamDisposition::Failed;
    RequestOutcome::from_success(status_ok && body.get("error").is_none() && !task_failed)
}

/// Why the agent's reply stream ended before its final event, in words for the user.
fn stream_cut_reason(err: &reqwest::Error, call_timeout_secs: u64) -> String {
    if err.is_timeout() {
        format!(
            "The agent did not finish within the {call_timeout_secs}-second limit, so its reply was cut off."
        )
    } else {
        "The connection to the agent was lost before it finished replying.".to_string()
    }
}

fn extract_failure_message(data: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(data).ok()?;
    let status_update = parsed
        .get("statusUpdate")
        .or_else(|| parsed.get("result").and_then(|r| r.get("statusUpdate")))?;
    let state = status_update.pointer("/status/state")?.as_str()?;
    if state != "TASK_STATE_FAILED" {
        return None;
    }
    let parts = status_update.pointer("/status/message/parts")?.as_array()?;
    let text: String = parts
        .iter()
        .filter_map(|p| p.get("text")?.as_str())
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() { None } else { Some(text) }
}

// `paused_task_id`/`build_pause_question` moved to `oss/types/src/a2a.rs` (shared with
// `oss/orchestrator`'s MAF executor, which cannot depend on this crate) — re-exported here so
// existing call sites in this file and in `oss/server/src/hitl/mod.rs` are unaffected.
pub(crate) use nasiko_types::a2a::{build_pause_question, paused_task_id};

/// Derive a `hitl_requests.kind` from a `Paused`-classified SSE payload — thin wrapper over
/// `nasiko_types::a2a::pause_reason` (moved there so `oss/orchestrator`'s MAF executor can share
/// the same parsing), mapped to this crate's `nasiko_hitl::HitlKind`.
pub(crate) fn pause_kind(data: &str) -> nasiko_hitl::HitlKind {
    match nasiko_types::a2a::pause_reason(data) {
        nasiko_types::a2a::PauseReason::InputRequired => nasiko_hitl::HitlKind::InputRequired,
        nasiko_types::a2a::PauseReason::AuthRequired => nasiko_hitl::HitlKind::AuthRequired,
    }
}

/// Persists a HITL pause from a `Paused`-classified payload for either direct-chat origin —
/// shared by `agent_stream()`'s streaming and non-streaming branches (`origin = DirectChat`)
/// and `agent_proxy.rs`'s streaming and non-streaming branches (`origin = AgentProxy`), which
/// previously reimplemented this inline and had drifted (the `agent_proxy.rs` copy never
/// updated the `flows` row). On a persistence failure, marks the flow `failed` and returns the
/// SSE `error` event the caller must yield — a pause must never look like a silent success (§8:
/// never pretend the stream closed cleanly when the pause itself might be lost).
///
/// On success, marks the flow `paused` and returns the created row — the caller (both pause
/// branches of `agent_stream()`) uses `row.id`/`row.kind`/`row.question` to build the
/// frontend-facing HITL stream event (`build_hitl_stream_event`) from the SAME row the database
/// just durably committed, never from the pre-persistence `pause_data`. This is the ordering fix:
/// the id cannot exist, and therefore cannot be handed to the caller, before `hitl_store.create`
/// has returned `Ok`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_direct_chat_pause(
    hitl_store: &std::sync::Arc<dyn nasiko_hitl::HitlStore>,
    db: &sqlx::PgPool,
    origin: nasiko_hitl::HitlOrigin,
    agent_id_for_hitl: Uuid,
    user_id: Uuid,
    context_id: &str,
    task_id: &str,
    chat_session_id: Option<&str>,
    flow_id: &str,
    pause_data: &str,
) -> Result<nasiko_hitl::HitlRequest, Event> {
    let question = build_pause_question(pause_data);
    let kind = pause_kind(pause_data);
    // The agent's OWN task id, not Nasiko's synthetic per-request `task_id` — see
    // `paused_task_id`'s doc comment. Resume must address the task the agent's own store holds.
    let real_task_id = paused_task_id(pause_data, task_id);

    // This pause may itself be a mirror of a real `mcp_tool` row (see `resolve_display_row`'s doc
    // comment) — the agent stamped `hitl_request_id` into its own pause metadata, and
    // `build_pause_question` forwarded it verbatim into `question`. A tool-call retry re-pauses
    // on a brand-new `task_id` while the underlying `mcp_tool` approval is still the SAME pending
    // one, and nothing before this point knows that — without this check, every retry mints a
    // second, third, ... mirror row for the identical wait, each resolving to the same
    // substituted question and rendered as a separate, duplicate approval card. Retargets the
    // existing mirror onto the new task/context instead of minting another one — not a plain
    // reuse-as-is, since the OLD mirror's `task_id` names a task the retry has already
    // superseded; resolving it unchanged would deliver the human's answer to a dead task while
    // the actually-live retry sits unresolved.
    if let Some(linked_id) = question
        .get("metadata")
        .and_then(|m| m.get("hitl_request_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        let existing_mirror_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM hitl_requests
              WHERE owner_user_id = $1 AND agent_id = $2 AND status = 'pending'
                AND question->'metadata'->>'hitl_request_id' = $3
              LIMIT 1",
        )
        .bind(user_id)
        .bind(agent_id_for_hitl)
        .bind(linked_id.to_string())
        .fetch_optional(db)
        .await
        .unwrap_or(None);
        if let Some(existing_mirror_id) = existing_mirror_id {
            let retargeted = sqlx::query(
                "UPDATE hitl_requests SET task_id = $2, context_id = $3, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(existing_mirror_id)
            .bind(&real_task_id)
            .bind(context_id)
            .execute(db)
            .await
            .is_ok();
            if retargeted {
                // Same flow-status bookkeeping the create path below does — this retry opened
                // its own `flows` row (`flow_id`), which needs marking `paused` too, distinct
                // from whatever flow the mirror's original pause opened.
                let _ = sqlx::query("UPDATE flows SET status = 'paused' WHERE flow_id = $1")
                    .bind(flow_id)
                    .execute(db)
                    .await;
                if let Ok(Some(existing_row)) = hitl_store.get(existing_mirror_id).await {
                    return Ok(existing_row);
                }
            }
        }
    }

    // `hitl_requests.chat_session_id` carries a hard FK to `chat_sessions(session_id)` — unlike
    // `context_id`/`task_id`, which are free-form strings, a bogus value here doesn't just fail
    // to correlate, it fails the ENTIRE insert (confirmed live: `hitl_requests_chat_session_id_fkey`
    // violation). The ordinary web-UI path always creates the `chat_sessions` row via
    // `POST /chat/sessions` before ever sending a turn, so this is normally a no-op existence
    // check — but a pause must never be lost over a stale/foreign session id some other caller
    // supplied, so this degrades to `None` (this function's pre-existing behavior) rather than
    // letting the whole pause fail on a FK violation the caller can't fix.
    let chat_session_id = match chat_session_id {
        Some(sid) => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1)",
            )
            .bind(sid)
            .fetch_one(db)
            .await
            .unwrap_or(false);
            exists.then(|| sid.to_string())
        }
        None => None,
    };
    let new_row = match origin {
        // `.with_chat_session_id` on BOTH arms: the id is computed and FK-validated above for every
        // origin, and applying it to only one silently cost the proxy path two things (found in
        // review) — the row missed `list_for_chat_session`, so a proxy pause never appeared on
        // session load, and `deliver`'s flow registration is guarded on this being `Some`,
        // so every MCP tool-approval retry after the resume resolved to a fresh trace id and
        // re-asked the human to approve the same tool.
        nasiko_hitl::HitlOrigin::AgentProxy => nasiko_hitl::NewHitlRequest::agent_proxy(
            kind,
            agent_id_for_hitl,
            user_id,
            real_task_id,
            context_id.to_string(),
            question,
        )
        .with_chat_session_id(chat_session_id),
        _ => nasiko_hitl::NewHitlRequest::direct_chat(
            kind,
            agent_id_for_hitl,
            user_id,
            real_task_id,
            context_id.to_string(),
            question,
        )
        .with_chat_session_id(chat_session_id),
    };

    match hitl_store.create(new_row).await {
        Ok(row) => {
            let _ = sqlx::query("UPDATE flows SET status = 'paused' WHERE flow_id = $1")
                .bind(flow_id)
                .execute(db)
                .await;
            // Stash the original flow_id so the resume dispatcher can reopen this
            // same flow instead of minting a new one — the agent's OTel auto-
            // instrumentation already carries this trace_id, so reusing it means
            // zero agent-side changes for traceparent propagation on HITL resume.
            let _ = sqlx::query(
                "UPDATE hitl_requests SET resume_state = resume_state || $2 WHERE id = $1",
            )
            .bind(row.id)
            .bind(serde_json::json!({ "flow_id": flow_id }))
            .execute(db)
            .await;
            Ok(row)
        }
        Err(e) => {
            let _ = sqlx::query(
                "UPDATE flows SET status = 'failed', error_message = $2 WHERE flow_id = $1",
            )
            .bind(flow_id)
            .bind(e.to_string())
            .execute(db)
            .await;
            Err(Event::default()
                .event("error")
                .data(json!({ "error": format!("failed to persist HITL pause: {e}") }).to_string()))
        }
    }
}

/// The frontend-facing HITL metadata frame — yielded on the SAME still-open SSE connection
/// immediately after the `hitl_requests` row (and therefore its id) is durably committed. Follows
/// the exact `agent_message`/`data_part`/`status_event` convention already used for the
/// `trace_meta` and `usage_meta` synthetic frames in this file, rather than inventing a new SSE
/// event type: any client already parsing those (via `handleDataParts` in `a2a-stream.js`) sees
/// this the same way. This is a synthetic, Nasiko-originated frame layered onto the agent's own
/// `input-required`/`auth-required` status event (already yielded earlier, unmodified) — it does
/// not replace or alter that event.
///
/// `id`/`kind`/`question` go through `nasiko_hitl::resolve_display_row` first — for the ordinary
/// case (no MCP tool block involved) this is a no-op clone of `row` itself, but when `row` is a
/// mirror of a real `mcp_tool` row (see that function's doc comment), it substitutes the real
/// row's identity so the frontend never shows — or lets a human resolve — the mirror's own
/// generic placeholder. `task_id`/`context_id` in the frame are always the mirror's own
/// (`row.task_id`/`row.context_id`), never the linked row's, since those are what ties this frame
/// to the visible chat task.
///
/// `agent`: `None` for direct chat, which never needs it — there is exactly one agent in that
/// conversation, already known to whoever is looking at the screen. `Some(name)` for the
/// orchestrator, which can delegate to any of several agents in one conversation, so the frontend
/// needs to know which one is actually asking before it can show the question sensibly.
/// The bare SSE `data:` payload for a `"type":"hitl"` frame — same shape `build_hitl_stream_event`
/// wraps into a full `Event` below for a live turn's own stream. Split out (`pub(crate)`) so the
/// HITL resume dispatcher (`hitl/mod.rs`) can push the exact same frame into a resumed execution's
/// continuation buffer when the agent pauses again mid-resume, without duplicating this payload
/// shape — a reconnecting `POST /api/orchestrator/a2a` client and a still-live one see identical
/// framing for "here's the next HITL."
pub(crate) async fn build_hitl_stream_data(
    hitl_store: &std::sync::Arc<dyn nasiko_hitl::HitlStore>,
    task_id: &str,
    context_id: &str,
    row: &nasiko_hitl::HitlRequest,
    agent: Option<&str>,
) -> String {
    let display =
        nasiko_hitl::resolve_display_row(hitl_store.as_ref(), row, row.owner_user_id).await;
    let mut payload = json!({
        "type": "hitl",
        "id": display.id,
        "kind": display.kind.as_str(),
        "task_id": row.task_id,
        "context_id": row.context_id,
        "question": display.question,
    });
    if let Some(agent) = agent
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("agent".to_string(), json!(agent));
    }
    let data = a2a::data_part(payload);
    let msg = a2a::agent_message(context_id, task_id, data);
    a2a::to_sse_data(&a2a::status_event(a2a::working_with_message(
        task_id, context_id, msg,
    )))
}

async fn build_hitl_stream_event(
    hitl_store: &std::sync::Arc<dyn nasiko_hitl::HitlStore>,
    task_id: &str,
    context_id: &str,
    row: &nasiko_hitl::HitlRequest,
    agent: Option<&str>,
) -> Event {
    Event::default().data(build_hitl_stream_data(hitl_store, task_id, context_id, row, agent).await)
}

pub(crate) fn normalize_agent_event(data: &str, task_id: &str, context_id: &str) -> String {
    let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return data.to_string();
    };
    if drop_task_history(&mut parsed) {
        return serde_json::to_string(&parsed).unwrap_or_else(|_| data.to_string());
    }

    // Already in the expected format (no JSON-RPC wrapper)
    if parsed.get("statusUpdate").is_some()
        || parsed.get("artifactUpdate").is_some()
        || parsed.get("task").is_some()
        || parsed.get("message").is_some()
    {
        return data.to_string();
    }

    let Some(result) = parsed.get("result") else {
        return data.to_string();
    };

    let kind = result.get("kind").and_then(|k| k.as_str()).unwrap_or("");

    match kind {
        "artifact-update" => {
            let artifact = result.get("artifact").cloned().unwrap_or(json!({}));
            let append = result
                .get("append")
                .and_then(|a| a.as_bool())
                .unwrap_or(false);
            let last_chunk = result
                .get("final")
                .and_then(|f| f.as_bool())
                .unwrap_or(false);

            let normalized = json!({
                "artifactUpdate": {
                    "taskId": task_id,
                    "contextId": context_id,
                    "artifact": artifact,
                    "append": append,
                    "lastChunk": last_chunk,
                }
            });
            serde_json::to_string(&normalized).unwrap_or_else(|_| data.to_string())
        }
        "status-update" => {
            let status = result.get("status").cloned().unwrap_or(json!({}));
            let normalized = json!({
                "statusUpdate": {
                    "taskId": task_id,
                    "contextId": context_id,
                    "status": status,
                }
            });
            serde_json::to_string(&normalized).unwrap_or_else(|_| data.to_string())
        }
        _ => data.to_string(),
    }
}

/// A direct agent narrates each tool call as a free-form Working-status message of the form
/// `"{tool_name}: {preview}"` (`oss/agents/*/src/main.rs::status_working`), rather than the
/// structured `tool_call` data part the orchestrator emits and the chat renders as a tool row.
/// Split that convention back out — `(tool_name, preview)` — so the dispatch loop can synthesise
/// the structured parts without any change to the agents. `None` for ordinary progress narration,
/// which has no leading `identifier` + `": "` (e.g. "Looking up the exchange rates...").
fn tool_from_status_text(text: &str) -> Option<(&str, &str)> {
    let (name, preview) = text.split_once(": ")?;
    let name = name.trim();
    let looks_like_tool = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    looks_like_tool.then_some((name, preview.trim()))
}

fn drop_task_history(frame: &mut serde_json::Value) -> bool {
    let result_is_task = frame
        .get("result")
        .and_then(|r| r.get("kind"))
        .and_then(|k| k.as_str())
        == Some("task");
    let task = if result_is_task {
        frame.get_mut("result")
    } else if frame.get("task").is_some() {
        frame.get_mut("task")
    } else {
        frame.get_mut("result").and_then(|r| r.get_mut("task"))
    };
    task.and_then(|t| t.as_object_mut())
        .is_some_and(|t| t.remove("history").is_some())
}

// ─── Types & Errors ──────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
#[allow(dead_code)]
struct AgentRow {
    id: Uuid,
    name: String,
    status: String,
    /// Plain column (migration 0032), not a secret — read fresh on every
    /// dispatch so the minimal-code ladder injection below applies
    /// immediately when toggled, with no agent restart needed.
    minimal_code_enabled: bool,
    /// The agent card's skills, classified here rather than in SQL so the dispatch path and
    /// the settings page share one answer — see [`catalog::models::has_coding_skills`].
    skills: sqlx::types::Json<Vec<crate::catalog::models::Skill>>,
    /// Decides whether the history sent to this agent is compressed (IP-4).
    compress_enabled: bool,
    /// The same row as `agent_resolver` reads it, so the forward resolves without a second read.
    #[sqlx(flatten)]
    route: crate::agent_resolver::AgentRow,
}

impl AgentRow {
    /// Whether `coding::proxy::load_agent_target` can find a target for this agent; an ordinary
    /// agent skips that query.
    fn may_drive_code(&self) -> bool {
        matches!(
            self.route.workload_kind.as_str(),
            "acp_coding_agent" | "harness"
        )
    }
}

#[derive(Debug)]
pub enum A2aDispatchError {
    InvalidRequest(String),
    NoAgents,
    AgentNotFound(String),
    Internal(String),
    /// The agent cannot take the call right now. The message comes from the
    /// [`crate::agent_wake::AgentWake`] seam and is already caller-safe.
    Unavailable(String),
    /// The contextId names a harness session that is not the caller's (or does not exist);
    /// the two are indistinguishable on purpose.
    SessionNotFound,
    /// The harness session already has a turn running.
    SessionBusy,
    /// A `reconnect_after_hitl_id` reconnect was for a HITL row that exists but isn't the
    /// caller's — same rule `authorize_hitl_action` enforces for every other `/api/hitl/*` route.
    Forbidden(String),
    /// The agent answered the dispatch with an error status of its own. Separate from
    /// [`Self::Internal`] because the caller, not the operator, is usually the one who can act:
    /// the turn never ran, and the commonest case (`413`) is fixed by sending a smaller
    /// attachment. `message` is already caller-safe — see [`agent_rejection_message`].
    AgentRejected {
        status: StatusCode,
        message: String,
    },
    /// The target agent could not be resolved to somewhere to send the request.
    Resolve(crate::agent_resolver::ResolveError),
    /// The session admission refused the harness turn; answered with its status, and its body as
    /// the JSON-RPC error's `data`, under the request's own `id`.
    SessionRefused {
        refusal: crate::sessions::admission::SessionRefusal,
        id: serde_json::Value,
    },
}

impl IntoResponse for A2aDispatchError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::SessionRefused { refusal, id } => {
                let body = refusal.json_rpc_error(&id);
                return (refusal.status, Json(body)).into_response();
            }
            Self::InvalidRequest(e) => (StatusCode::BAD_REQUEST, -32602, e),
            Self::NoAgents => (
                StatusCode::SERVICE_UNAVAILABLE,
                -32603,
                "no agents available".into(),
            ),
            Self::AgentNotFound(name) => (
                StatusCode::NOT_FOUND,
                -32604,
                format!("agent '{}' not found or not running", name),
            ),
            Self::Forbidden(e) => (StatusCode::FORBIDDEN, -32605, e),
            Self::Unavailable(e) => (StatusCode::SERVICE_UNAVAILABLE, -32603, e),
            // A client error the agent raised is passed back as itself — a `413` the caller can
            // act on must not arrive looking like a server fault. An upstream 5xx is this
            // platform's bad gateway, not the caller's request.
            Self::AgentRejected { status, message } => {
                let status = if status.is_client_error() {
                    status
                } else {
                    StatusCode::BAD_GATEWAY
                };
                (status, -32606, message)
            }
            Self::Resolve(e) => {
                tracing::warn!(error = %e, "a2a dispatch: target resolution failed");
                (e.http_status(), -32609, e.client_message())
            }
            Self::SessionNotFound => (
                StatusCode::NOT_FOUND,
                -32607,
                "session_not_found".to_string(),
            ),
            Self::SessionBusy => (
                StatusCode::CONFLICT,
                -32608,
                "another turn is already running for this session".to_string(),
            ),
            Self::Internal(e) => {
                // `e` may carry raw DB/IO/upstream error text (see call sites) — log
                // it server-side and never echo it back to the client (SRV raw-error
                // leak sweep).
                tracing::error!(error = %e, "a2a dispatch internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    -32603,
                    "internal error".to_string(),
                )
            }
        };

        let body = Json(json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": { "code": code, "message": message }
        }));

        (status, body).into_response()
    }
}

#[cfg(test)]
mod hitl_pause_tests {
    use super::*;

    // `paused_task_id`/`pause_kind`/`build_pause_question`'s own behavior is covered by
    // `oss/types/src/a2a.rs`'s `pause_parsing_tests` now that the logic lives there. This test
    // only covers the thin `pause_kind` wrapper's mapping onto this crate's `HitlKind`.
    const AUTH_REQUIRED_PAYLOAD: &str = r#"{"result": {"statusUpdate": {"taskId": "t1", "contextId": "c1", "status": {"state": "TASK_STATE_AUTH_REQUIRED", "message": {"parts": [{"text": "Please authorize with GitHub"}]}}}}, "id": "1", "jsonrpc": "2.0"}"#;

    #[test]
    fn pause_kind_maps_auth_required_to_the_hitl_crate_enum() {
        assert_eq!(
            pause_kind(AUTH_REQUIRED_PAYLOAD),
            nasiko_hitl::HitlKind::AuthRequired
        );
    }
}

#[cfg(test)]
mod tool_status_text_tests {
    use super::*;

    #[test]
    fn splits_the_seed_agent_tool_convention() {
        // `oss/agents/*/src/main.rs::status_working(Some("{name}: {preview}"))`.
        assert_eq!(
            tool_from_status_text("web_search: set up GitHub Actions for Rust"),
            Some(("web_search", "set up GitHub Actions for Rust"))
        );
        assert_eq!(
            tool_from_status_text("github_repo_info: tokio-rs/tokio"),
            Some(("github_repo_info", "tokio-rs/tokio"))
        );
    }

    #[test]
    fn keeps_a_colon_in_the_preview() {
        assert_eq!(
            tool_from_status_text("web_search: how to: do a thing"),
            Some(("web_search", "how to: do a thing"))
        );
    }

    #[test]
    fn ignores_ordinary_progress_narration() {
        // A tool name is an identifier, never a sentence with spaces before the colon.
        assert_eq!(
            tool_from_status_text("Looking up the exchange rates..."),
            None
        );
        assert_eq!(
            tool_from_status_text("Processing the exchange rates: done"),
            None
        );
        assert_eq!(tool_from_status_text("no colon here"), None);
        assert_eq!(tool_from_status_text(": no name"), None);
    }
}

#[cfg(test)]
mod agent_rejection_tests {
    use super::*;

    #[test]
    fn payload_too_large_with_an_attachment_names_the_remedy() {
        let message = agent_rejection_message(StatusCode::PAYLOAD_TOO_LARGE, "opencode", true);
        assert!(message.contains("opencode"));
        assert!(message.contains("too large"));
        assert!(message.contains("smaller file"));
    }

    #[test]
    fn payload_too_large_without_one_does_not_blame_an_attachment() {
        let message = agent_rejection_message(StatusCode::PAYLOAD_TOO_LARGE, "opencode", false);
        assert!(message.contains("too large"));
        assert!(!message.contains("attachment"));
    }

    #[test]
    fn other_statuses_report_the_code_they_came_back_with() {
        let message = agent_rejection_message(StatusCode::UNAUTHORIZED, "docs", false);
        assert!(message.contains("401"));
    }

    /// The point of the variant: a 413 the caller can fix must not reach them as a 500.
    #[test]
    fn a_client_error_is_passed_through_and_a_server_error_becomes_a_bad_gateway() {
        let client = A2aDispatchError::AgentRejected {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "too large".into(),
        };
        assert_eq!(
            client.into_response().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );

        let server = A2aDispatchError::AgentRejected {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "upstream blew up".into(),
        };
        assert_eq!(server.into_response().status(), StatusCode::BAD_GATEWAY);
    }
}

#[cfg(test)]
mod session_refused_tests {
    use super::*;

    #[tokio::test]
    async fn a_refused_turn_answers_under_the_request_id_with_the_refusal_status() {
        let refused = A2aDispatchError::SessionRefused {
            refusal: crate::sessions::admission::SessionRefusal {
                status: StatusCode::PAYMENT_REQUIRED,
                body: json!({ "error": "at the cap", "code": "plan_agent_limit_reached" }),
            },
            id: json!("7"),
        };
        let response = refused.into_response();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["id"], "7");
        assert_eq!(body["error"]["data"]["code"], "plan_agent_limit_reached");
    }
}

/// `build_preamble`'s only job is precedence ordering — L0 > L1A > L2 — over three independently
/// optional inputs. Pure and DB-free, so every combination is cheap to pin hermetically without
/// standing up `orchestrator_stream`'s full DB/state dependencies.
#[cfg(test)]
mod build_preamble_tests {
    use super::*;

    #[test]
    fn nothing_configured_or_recalled_is_none_not_an_empty_string() {
        // The common case (OSS, or an EE deployment with nothing to add yet) must leave
        // `OrchestratorConfig.preamble` exactly `None`, not `Some(String::new())` — a caller that
        // only checks `.is_some()` would otherwise start behaving as if something was injected.
        assert_eq!(build_preamble(None, None, None), None);
    }

    #[test]
    fn each_layer_alone_surfaces_on_its_own() {
        assert_eq!(
            build_preamble(Some("ORG"), None, None),
            Some("ORG".to_string())
        );
        assert_eq!(
            build_preamble(None, Some("FACTS"), None),
            Some("FACTS".to_string())
        );
        assert_eq!(
            build_preamble(None, None, Some("RECALL")),
            Some("## What you know about this user\nRECALL".to_string())
        );
    }

    #[test]
    fn all_three_land_in_precedence_order_l0_then_l1a_then_l2() {
        let preamble = build_preamble(Some("ORG"), Some("FACTS"), Some("RECALL")).unwrap();
        let org_at = preamble.find("ORG").expect("org policy present");
        let facts_at = preamble.find("FACTS").expect("agent facts present");
        let user_at = preamble
            .find("## What you know about this user")
            .expect("user recall present");
        assert!(
            org_at < facts_at && facts_at < user_at,
            "L0 must precede L1A must precede L2, got: {preamble:?}"
        );
    }

    #[test]
    fn user_recall_is_headed_but_agent_facts_are_not_double_headed() {
        // `agent_facts` already carries its own "## Known facts..." heading from its own call
        // site — `build_preamble` must not add a second one on top of it.
        let preamble =
            build_preamble(None, Some("## Known facts about specific agents\nX"), None).unwrap();
        assert_eq!(
            preamble.matches("##").count(),
            1,
            "agent_facts must not be re-headed: {preamble:?}"
        );
    }
}

#[cfg(test)]
mod reply_outcome_tests {
    use super::*;

    #[test]
    fn a_non_streaming_reply_is_ok_only_without_an_error_or_a_failed_task() {
        let completed = json!({"jsonrpc": "2.0", "id": "1", "result": {"task": {
            "id": "t", "status": {"state": "TASK_STATE_COMPLETED"}}}});
        let flat_message = json!({"jsonrpc": "2.0", "id": "1",
            "result": {"kind": "message", "parts": [{"kind": "text", "text": "ok"}]}});
        let failed = json!({"jsonrpc": "2.0", "id": "1", "result": {"task": {
            "id": "t", "status": {"state": "TASK_STATE_FAILED"}}}});
        let rpc_error = json!({"jsonrpc": "2.0", "id": "1",
            "error": {"code": -32601, "message": "method not found"}});

        assert_eq!(
            non_streaming_reply_outcome(true, &completed),
            RequestOutcome::Ok
        );
        assert_eq!(
            non_streaming_reply_outcome(true, &flat_message),
            RequestOutcome::Ok
        );
        assert_eq!(
            non_streaming_reply_outcome(false, &completed),
            RequestOutcome::Error
        );
        assert_eq!(
            non_streaming_reply_outcome(true, &failed),
            RequestOutcome::Error
        );
        assert_eq!(
            non_streaming_reply_outcome(true, &rpc_error),
            RequestOutcome::Error
        );
    }
}

/// `append_org_policy_part` is `agent_stream`'s half of the marker-splicing fix — the other half
/// (L1A, at the EE injection proxy) is pinned by that hop's own tests. These
/// don't need a live agent or the EE proxy: `append_org_policy_part` operates purely on an
/// in-memory `JsonRpcRequest`, so they run hermetically.
#[cfg(test)]
mod org_policy_part_tests {
    use super::*;

    fn parts_of(req: &JsonRpcRequest) -> &Vec<serde_json::Value> {
        req.params
            .as_ref()
            .unwrap()
            .pointer("/message/parts")
            .unwrap()
            .as_array()
            .unwrap()
    }

    #[test]
    fn no_org_context_appends_nothing() {
        let mut req = nasiko_types::a2a::build_stream_request("hello", Some("ctx-1"));
        append_org_policy_part(&mut req, None);
        let parts = parts_of(&req);
        assert_eq!(
            parts.len(),
            1,
            "a None org_context must be a complete no-op"
        );
        assert_eq!(parts[0]["text"], "hello");
    }

    #[test]
    fn org_context_is_appended_as_its_own_part_after_the_query() {
        let mut req = nasiko_types::a2a::build_stream_request("hello", Some("ctx-1"));
        append_org_policy_part(&mut req, Some("## Organization Policy\n\n- rule: text"));

        let parts = parts_of(&req);
        assert_eq!(
            parts.len(),
            2,
            "the query part plus exactly one appended policy part"
        );
        assert_eq!(
            parts[0]["text"], "hello",
            "the query part must be untouched"
        );
        assert_eq!(parts[1]["text"], "## Organization Policy\n\n- rule: text");
        // `build_stream_request` always uses the proto-style method name ("SendStreamingMessage"),
        // and this platform's real `a2a-lf` `Part` type has no `kind` field at all — an appended
        // part carrying one is rejected outright by a real agent's strict parser (confirmed live).
        assert!(
            parts[1].get("kind").is_none(),
            "a part built for a proto-dialect request must never carry `kind`: {parts:?}"
        );
    }

    #[test]
    fn org_context_is_tagged_when_the_request_uses_the_jsonrpc_dialect() {
        // `append_org_policy_part` always sees a proto-style method in production today (every
        // caller builds via `nasiko_types::a2a::build_request`), but it derives tagging from
        // `req.method` rather than assuming that, so this pins the OTHER half of the mapping
        // stays correct too.
        let mut req = nasiko_types::a2a::build_stream_request("hello", Some("ctx-1"));
        req.method = "message/stream".to_string();
        append_org_policy_part(&mut req, Some("POLICY"));

        let parts = parts_of(&req);
        assert_eq!(parts[1]["kind"], "text");
        assert_eq!(parts[1]["text"], "POLICY");
    }

    #[test]
    fn org_context_never_lands_ahead_of_the_history_glue_marker() {
        // The exact bug this fix closes: a history-glued query ("{history}\n\nCurrent message:
        // {q}") must keep its one marker in its one part — org policy must never appear ahead of
        // it in the same field, or an agent extracting only the text after the marker
        // (`oss/llm-router::latest_user_query`'s convention) silently discards it.
        let glued = "earlier turn\n\nCurrent message: what time is it?";
        let mut req = nasiko_types::a2a::build_stream_request(glued, Some("ctx-1"));
        append_org_policy_part(&mut req, Some("## Organization Policy\n\nbinding"));

        let parts = parts_of(&req);
        assert_eq!(
            parts[0]["text"], glued,
            "the glued query, marker included, is untouched"
        );
        let org_text = parts[1]["text"].as_str().unwrap();
        assert!(
            !org_text.contains("Current message:"),
            "the appended part must not carry the marker: {org_text:?}"
        );
    }
}

#[cfg(test)]
mod attachment_list_tests {
    use super::*;

    fn chat_file(name: &str) -> AttachedFile {
        AttachedFile {
            reference: AttachmentRef::ChatUpload {
                session_id: "s".into(),
                file_id: Uuid::nil(),
            },
            filename: name.into(),
            media_type: "image/png".into(),
            size_bytes: 64,
        }
    }

    fn workspace_file(name: &str, media_type: &str, size_bytes: u64) -> AttachedFile {
        AttachedFile {
            reference: AttachmentRef::Workspace {
                agent_id: Uuid::nil(),
                user_id: Uuid::nil(),
                blob_id: Uuid::nil(),
            },
            filename: name.into(),
            media_type: media_type.into(),
            size_bytes,
        }
    }

    #[test]
    fn a_turn_without_files_keeps_the_query_as_it_is() {
        let listed = with_attachment_list("hello", &[], &[]);
        assert!(matches!(listed, std::borrow::Cow::Borrowed("hello")));
        let text_only = [a2a::text_part("hello")];
        assert_eq!(with_attachment_list("hello", &text_only, &[]), "hello");
    }

    #[test]
    fn every_attached_file_is_listed_once_by_name_type_and_size() {
        let parts = [
            a2a::text_part("ignored"),
            a2a::file_part(
                vec![0; 64],
                Some("fact.png".into()),
                Some("image/png".into()),
            ),
            a2a::file_part(vec![0; 2048], None, None),
        ];
        let files = [
            chat_file("fact.png"),
            workspace_file("q3.pdf", "application/pdf", 3 * 1024 * 1024),
        ];
        assert_eq!(
            with_attachment_list("what is in these?", &parts, &files),
            "what is in these?\n\n[The user attached: fact.png (image/png, 64 B), \
             unnamed (application/octet-stream, 2.0 KB), q3.pdf (application/pdf, 3.0 MB)]"
        );
    }

    #[test]
    fn a_file_name_cannot_break_the_line() {
        let files = [workspace_file("evil\nname.txt", "text/plain", 5)];
        let listed = with_attachment_list("q", &[], &files);
        assert_eq!(
            listed,
            "q\n\n[The user attached: evil name.txt (text/plain, 5 B)]"
        );
    }
}

#[cfg(test)]
mod relayed_task_history_tests {
    use super::*;

    fn echoed_user_message() -> serde_json::Value {
        json!([{
            "messageId": "m1",
            "role": "ROLE_USER",
            "parts": [
                {"text": "describe this"},
                {"raw": "A".repeat(4096), "filename": "big.png", "mediaType": "image/png"}
            ]
        }])
    }

    fn relayed(frame: serde_json::Value) -> serde_json::Value {
        serde_json::from_str(&normalize_agent_event(&frame.to_string(), "t", "c")).unwrap()
    }

    #[test]
    fn a_wrapped_task_frame_loses_the_echoed_history_and_keeps_the_rest() {
        let out = relayed(json!({"jsonrpc": "2.0", "id": "1", "result": {"task": {
            "id": "inner", "contextId": "c", "status": {"state": "TASK_STATE_SUBMITTED"},
            "history": echoed_user_message()
        }}}));
        let task = &out["result"]["task"];
        assert!(task.get("history").is_none());
        assert_eq!(task["id"], "inner");
        assert_eq!(task["status"]["state"], "TASK_STATE_SUBMITTED");
        assert_eq!(out["id"], "1");
    }

    #[test]
    fn a_bare_task_frame_loses_the_echoed_history() {
        let out = relayed(json!({"task": {
            "id": "inner", "status": {"state": "TASK_STATE_WORKING"},
            "artifacts": [{"parts": [{"text": "kept"}]}],
            "history": echoed_user_message()
        }}));
        assert!(out["task"].get("history").is_none());
        assert_eq!(out["task"]["artifacts"][0]["parts"][0]["text"], "kept");
    }

    #[test]
    fn a_v03_kind_task_result_loses_the_echoed_history() {
        let out = relayed(json!({"jsonrpc": "2.0", "id": "1", "result": {
            "kind": "task", "id": "inner", "status": {"state": "submitted"},
            "history": [{"kind": "message", "role": "user", "parts": [
                {"kind": "file", "file": {"bytes": "A".repeat(4096), "name": "big.png"}}
            ]}]
        }}));
        assert!(out["result"].get("history").is_none());
        assert_eq!(out["result"]["kind"], "task");
        assert_eq!(out["result"]["status"]["state"], "submitted");
    }

    #[test]
    fn frames_without_task_history_are_relayed_byte_for_byte() {
        for frame in [
            r#"{"result":{"task":{"id":"inner","status":{"state":"TASK_STATE_SUBMITTED"}}}}"#,
            r#"{"statusUpdate":{"taskId":"t","status":{"state":"TASK_STATE_WORKING"}}}"#,
            r#"{"message":{"role":"ROLE_AGENT","parts":[{"text":"history"}]}}"#,
            "not json",
        ] {
            assert_eq!(normalize_agent_event(frame, "t", "c"), frame);
        }
    }

    #[test]
    fn a_large_echo_shrinks_to_a_small_frame() {
        let mut history = echoed_user_message();
        history[0]["parts"][1]["raw"] = json!("A".repeat(7 * 1024 * 1024));
        let frame = json!({"result": {"task": {"id": "inner", "history": history}}}).to_string();
        assert!(normalize_agent_event(&frame, "t", "c").len() < 200);
    }
}

#[cfg(test)]
mod stream_cut_tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// HD-04: an agent that is still working when the agent-call budget runs out. The relay's
    /// stream read fails with a timeout, and the user must be told so, not shown "completed".
    #[tokio::test]
    async fn a_reply_cut_off_by_the_time_limit_is_reported_as_such() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            // Headers and the first event, then silence: the agent is still working.
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: {}\n\n",
                )
                .await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });

        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{addr}/"))
            .timeout(std::time::Duration::from_millis(300))
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        let err = loop {
            match stream.next().await {
                Some(Ok(_)) => continue,
                Some(Err(e)) => break e,
                None => panic!("the stream ended without the timeout error"),
            }
        };

        let reason = stream_cut_reason(&err, 180);
        assert!(reason.contains("180-second limit"), "{reason}");
    }
}
