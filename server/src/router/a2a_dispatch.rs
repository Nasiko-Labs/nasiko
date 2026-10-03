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

use crate::acl::CpCallGuard;
use crate::auth::Claims;
use crate::orchestrator_policy::TurnKind;
use crate::state::AppState;

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
            let v = if n.eq_ignore_ascii_case("authorization") || n.eq_ignore_ascii_case("cookie") {
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

    // Prefer the explicit metadata.session_id (web UI), fall back to the
    // message contextId — the CLI reuses its CP session id as contextId, so
    // multi-turn chats keep their history either way. An unknown id simply
    // fetches zero rows.
    let history_sid = session_id.as_deref().unwrap_or(&context_id);
    let history_store = state.history_vector_store();
    let history = context_selection::fetch_for_user(
        &state.db,
        user_id,
        history_sid,
        &history_store,
        &text,
        &ContextTiers::from_config(&state.config),
    )
    .await;

    let query = history.with_current_query(&text);

    if is_orchestrator {
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
                file_parts: vec![],
                kind: TurnKind::User,
            },
        )
        .await
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
        // Supplemental context for this one, already-chosen agent, ranked against the real
        // query text (in addition to any pinned content) — a no-op on OSS. Kept in a separate
        // `outbound_query` local, distinct from `query`: `agent_stream` persists/traces `query`
        // verbatim (`flows.title`, `gen_ai.input.messages`) and must never record injected
        // context as if the user had typed it — see `crate::prompt_context` module docs.
        // Same "## Known facts" framing the routed path uses (below) so the agent can tell
        // injected admin knowledge apart from the user's own message by structure, not just by
        // reading closely — consistent provenance cues on both injection paths.
        let outbound_query = match state
            .prompt_context
            .context_for_agent(agent.id, &query)
            .await
        {
            Some(context) => format!("## Known facts about this agent\n{context}\n\n{query}"),
            None => query.clone(),
        };
        agent_stream(
            &state,
            agent,
            &query,
            &outbound_query,
            &task_id,
            &context_id,
            user_id,
            &[],
            session_id,
            history.user_turn_count(),
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
    /// Whether this turn was started by the user or reports on work an earlier
    /// turn already did. The operator's policy may treat the two differently —
    /// see [`TurnKind`].
    pub(crate) kind: TurnKind,
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
        kind,
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

    let mut agents: Vec<AgentInfo> = Vec::new();
    for summary in &agent_summaries {
        let endpoint = match resolve_endpoint(state, &summary.id.to_string(), &summary.name).await {
            Ok(url) => url,
            Err(_) => continue,
        };
        agents.push(AgentInfo {
            id: summary.id.to_string(),
            name: summary.name.clone(),
            description: summary.description.clone(),
            endpoint,
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
    // Known scope limit (v1, deliberate): this only reaches the *planner's own* preamble — a
    // minted sub-leg's actual outbound request to the chosen agent (built in
    // `nasiko-react-agent`'s `AgentTool::call()`, a lower-level OSS crate with no path to this
    // EE-installed context) does NOT itself carry the target agent's L1A. The planner may relay
    // some of what it saw into the sub-task text it writes, but that's LLM-mediated, not
    // guaranteed. Closing this needs either threading the injection seam down into
    // `nasiko-react-agent`, or a proxy that injects on the minted sub-leg directly — real,
    // separate scope, not something this call site can add on its own.
    // From `agents` (the post-endpoint-resolution roster actually rendered below), not
    // `agent_summaries` — looking up context for a candidate that gets dropped for having no
    // endpoint would waste a domain lookup (and possibly an embedding call) on an id the
    // preamble loop below can never reach anyway.
    let agent_ids: Vec<Uuid> = agents
        .iter()
        .filter_map(|a| Uuid::parse_str(&a.id).ok())
        .collect();
    let supplemental_context = state
        .prompt_context
        .context_for_agents(&agent_ids, query)
        .await;
    let preamble = if supplemental_context.is_empty() {
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

    // One read per turn, shared by IP-3 below. See `compression_opt_in` for why this is
    // aggregated over the caller's agents rather than read off a single one.
    let compression_opted_in = nasiko_orchestrator::compression_opt_in(&state.db, user_id).await;

    let config = OrchestratorConfig {
        // `state.config.openai_model` is already loaded via `env_or("OPENAI_MODEL",
        // "gpt-4o-mini")` (oss/config/src/lib.rs) — read that shared, validated
        // default rather than re-reading the env var here with a placeholder
        // fallback ("deepseek-v4-flash") that doesn't exist on a real OpenAI
        // endpoint and silently 404s every orchestrator call when OPENAI_MODEL
        // is unset.
        model: state.config.openai_model.clone(),
        base_url: std::env::var("OPENAI_BASE_URL").ok(),
        api_key: std::env::var("OPENAI_API_KEY").ok(),
        max_turns: 10,
        temperature: Some(0.2),
        policy: policy.clone(),
        preamble,
        // IP-3. Read here rather than defaulted, because `ContextConfig::default()` is
        // deliberately inert — without this the compressor is compiled in but unreachable.
        // Gated by the deployment flag AND the per-agent opt-in, so the UI switch starts and
        // stops this with the rest of the stack instead of leaving one layer running.
        context: nasiko_react_agent::ContextConfig {
            compress: nasiko_compress::Policy {
                enabled: state.config.react_compress_enabled && compression_opted_in,
                min_bytes: state.config.react_compress_min_bytes,
                ..Default::default()
            },
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
    state.flow_guard.init_flow(&flow_ctx, "orchestrator").await;

    // Carry the A2A context_id so the LLM gateway keys its decision cache on the
    // conversation, not this turn's trace id — mirrors the direct-agent proxy
    // (`agent_proxy.rs`). `derive_boundary_signals` reads `metadata->>'context_id'`.
    //
    // Re-opens on conflict for the same reason as the proxy path: a repeat
    // request under one traceparent must not inherit the `completed` status the
    // previous one left, or strict attribution denies the agent's LLM calls.
    let flow_metadata = serde_json::json!({ "context_id": context_id });
    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, metadata)
           VALUES ($1, $2, 'orchestrator', $3, 'running', $4)
           ON CONFLICT (flow_id) DO UPDATE
              SET status = 'running', completed_at = NULL"#,
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(query)
    .bind(&flow_metadata)
    .execute(&state.db)
    .await;

    // `flow_id` doubles as the trace id (see comment below) — index it against
    // the session so `nasiko history`'s Tempo-miss fallback
    // (`PgSessionIdResolver::traces_for_session`) can find it, the same way
    // `agent_proxy.rs` indexes every direct-agent call.
    let _ = sqlx::query(
        "INSERT INTO session_traces (session_id, trace_id, agent_id, agent_name) \
         VALUES ($1, $2, NULL, 'orchestrator') \
         ON CONFLICT (session_id, trace_id) DO NOTHING",
    )
    .bind(context_id)
    .bind(&flow_id)
    .execute(&state.db)
    .await;

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
    );

    // `A2aClient`'s own default is deliberately short — it is shared with agent
    // card / discovery fetches, where a long hang is the wrong behaviour. This
    // client makes real agent turns, including the `message/send` fallback taken
    // by agents that reject `message/stream`, so it carries the agent budget.
    let a2a_client = nasiko_react_agent::A2aClient::new()
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

    let mut rx = {
        let _entered = dispatch_span.enter();
        orchestrator.run_stream(query, file_parts)
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
        let mut pending_usage: Option<Uuid> = None;

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
                            if let Some(usage_id) = pending_usage.take() {
                                let result = sqlx::query(
                                    "UPDATE token_usage SET agent_id = (SELECT id FROM agents WHERE name = $2 AND status = 'running') WHERE id = $1 AND user_id = $3",
                                ).bind(usage_id).bind(&agent).bind(user_id).execute(&db).await;
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

                            pending_usage = match priced.persist(&usage_tracker, user_id, &flow_id_cleanup, None).await {
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
async fn resolve_agent(state: &AppState, target: &str) -> Result<AgentRow, A2aDispatchError> {
    // Excludes `is_internal` agents unconditionally, including for the owning
    // superuser — this is the platform's only generic A2A entry point, and an
    // internal agent (e.g. Weave's dashboard-generator) must be reachable
    // exclusively through its own dedicated route, never here, or the
    // superuser-ACL-bypass would leak it into ordinary chat
    // history/usage tracking.
    sqlx::query_as::<_, AgentRow>(
        "SELECT id, name, status, minimal_code_enabled, skills \
         FROM agents \
         WHERE (id::text = $1 OR name = $1) AND status = 'running' AND NOT is_internal",
    )
    .bind(target)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| A2aDispatchError::Internal(e.to_string()))?
    .ok_or_else(|| A2aDispatchError::AgentNotFound(target.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn agent_stream(
    state: &AppState,
    agent: AgentRow,
    // The user's actual message — persisted (`flows.title`) and traced (`gen_ai.input.messages`)
    // verbatim. Deliberately separate from `outbound_query`: anything server-injected (e.g.
    // enterprise-only supplemental context, see `prompt_context` module docs) must reach the
    // agent without also being recorded as "what the user said" in the flow UI or an
    // observability/audit trail.
    query: &str,
    // What's actually sent to the agent in the A2A request body — `query`, or `query` with
    // injected context prepended. Never read for persistence/tracing.
    outbound_query: &str,
    task_id: &str,
    context_id: &str,
    user_id: Uuid,
    file_parts: &[nasiko_types::a2a::Part],
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
) -> Result<Response, A2aDispatchError> {
    let endpoint = resolve_endpoint(state, &agent.id.to_string(), &agent.name)
        .await
        .map_err(A2aDispatchError::Internal)?;

    // Real root span for the direct-agent exchange — same rationale as the
    // orchestrator path: the forwarded traceparent must name an exported span.
    let dispatch_span = tracing::info_span!(
        "a2a.dispatch",
        otel.kind = "server",
        gen_ai.operation.name = "invoke_agent",
        gen_ai.agent.name = %agent.name,
        session.id = %context_id,
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
    state.flow_guard.init_flow(&flow_ctx, &agent.name).await;

    // Carry the A2A context_id so the LLM gateway keys its decision cache on the
    // conversation, not this turn's trace id — same reason as the orchestrator
    // branch above and `agent_proxy.rs`. Without it `derive_boundary_signals`
    // falls back to the per-turn flow_id, so every turn of one conversation looks
    // like a new conversation and the sticky decision is never reused.
    //
    // Re-opens on conflict — see the orchestrator branch above: a repeat request
    // under one traceparent must not inherit the previous one's `completed`.
    let flow_metadata = serde_json::json!({ "context_id": context_id });
    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, status, metadata)
           VALUES ($1, $2, $3, $4, $5, 'running', $6)
           ON CONFLICT (flow_id) DO UPDATE
              SET status = 'running', completed_at = NULL"#,
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(agent.id)
    .bind(&agent.name)
    .bind(query)
    .bind(&flow_metadata)
    .execute(&state.db)
    .await;
    // Participant record — load-bearing for MCP gateway / LLM router auth
    // (docs/MCP_GATEWAY_AGENT_AUTH.md §2.4); same synchronous pre-forward write
    // as the flows row above.
    crate::flows::record_participant(&state.db, &flow_id, agent.id).await;

    // Index the session ↔ trace mapping, exactly as `orchestrator_stream` and
    // `agent_proxy.rs` already do. Without it this branch — every "chat with
    // this specific agent" request — had no DB record at all, so
    // `/api/observability/session/{id}` could only resolve it through the
    // Tempo `session.id` attribute and returned 404 the moment that lookup
    // missed. `flow_id` doubles as the trace id (see the flow-origin note).
    let _ = sqlx::query(
        "INSERT INTO session_traces (session_id, trace_id, agent_id, agent_name) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (session_id, trace_id) DO NOTHING",
    )
    .bind(context_id)
    .bind(&flow_id)
    .bind(agent.id)
    .bind(&agent.name)
    .execute(&state.db)
    .await;

    // Minimal-code ladder injection: only what's actually FORWARDED to the
    // agent gets the addendum appended — `query` itself stays untouched, so
    // everything above that already used it (the flow title, the span's
    // captured input, session_traces) keeps showing the real conversation,
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
    // Built from `outbound_query`, never from `query`: `outbound_query` is `query` plus any
    // server-injected context (enterprise supplemental knowledge), and it is what the agent is
    // meant to receive. Building from `query` here silently dropped that injection — the
    // context was resolved on every dispatch and then thrown away.
    let effective_query =
        if agent.minimal_code_enabled && crate::catalog::models::has_coding_skills(&agent.skills) {
            let addendum = nasiko_coding_policy::minimal_code_addendum(prior_turn_count);
            tracing::info!(
                agent_id = %agent.id,
                %context_id,
                prior_turn_count,
                "a2a_dispatch: injecting minimal-code ladder"
            );
            format!("{outbound_query}\n{addendum}")
        } else {
            outbound_query.to_string()
        };

    // Streaming first (`message/stream`): agents that stream (all the Rust
    // seed agents, and python a2a-sdk servers) deliver live tokens and tool
    // activity. The SSE loop below terminates itself on the first terminal
    // status event because the python a2a-sdk SSE producer never closes the
    // stream on its own (upstream bug). Agents that answer with plain JSON
    // (or reject `message/stream`) fall through to the non-streaming branch,
    // which retries with `message/send`.
    let req_body = if file_parts.is_empty() {
        nasiko_types::a2a::build_stream_request(&effective_query, Some(context_id))
    } else {
        nasiko_types::a2a::build_stream_request_with_parts(
            &effective_query,
            Some(context_id),
            file_parts,
        )
    };

    // No per-request MCP credential: the agent authenticates to /api/mcp with
    // its own deploy-time MCP_GATEWAY_TOKEN; the forwarded traceparent + the
    // flow_participants record written above carry the user binding.
    let build_agent_req = || {
        state
            .http_client
            .post(&endpoint)
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .header("traceparent", crate::telemetry::traceparent_for(&flow_ctx))
            // Agent turns can legitimately run past the shared client's short
            // default (long tool calls, multi-step orchestration); override
            // per-request instead of raising the global default for every caller
            // of `state.http_client`.
            .timeout(std::time::Duration::from_secs(
                state.config.agent_call_timeout_secs,
            ))
    };

    let response = build_agent_req()
        .json(&req_body)
        .send()
        .await
        .map_err(|e| A2aDispatchError::Internal(format!("agent request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(A2aDispatchError::Internal(format!(
            "agent HTTP {}: {}",
            status, body
        )));
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

    if content_type.contains("text/event-stream") {
        let byte_stream = response.bytes_stream();
        let flow_id_cleanup = flow_id;

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
                            Err(_) => { agent_done = true; continue; }
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
                                let normalized = normalize_agent_event(data, &task_id, &context_id);
                                yield Ok(Event::default().data(normalized));
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

                if let Some(err) = agent_error {
                    yield Ok(to_sse(a2a::status_event(a2a::failed(&task_id, &context_id, &err))));
                } else {
                    yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));
                }

                let _ = sqlx::query(
                    r#"UPDATE flows SET status = 'completed',
                       duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
                       completed_at = now()
                       WHERE flow_id = $1"#,
                ).bind(&flow_id_cleanup).execute(&db).await;
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

        // The agent rejected `message/stream` (e.g. method not found) —
        // retry once with plain `message/send`.
        if resp_body.get("error").is_some() {
            let retry_body =
                nasiko_types::a2a::build_send_request(&effective_query, Some(&context_id));
            let retry = build_agent_req()
                .json(&retry_body)
                .send()
                .await
                .map_err(|e| A2aDispatchError::Internal(format!("agent request failed: {e}")))?;
            resp_body = retry
                .json()
                .await
                .map_err(|e| A2aDispatchError::Internal(format!("invalid agent JSON: {e}")))?;
        }

        // A non-streaming reply can pause too (an agent that only implements
        // `message/send`, or whose `message/stream` fell back here) — §8 applies here just as
        // much as it does to the streaming branch; without this check the pause was silently
        // flattened into `extract_text`'s ordinary-reply fallback and lost entirely.
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

            yield Ok(to_sse(a2a::status_event(a2a::completed(&task_id, &context_id))));

            let _ = sqlx::query(
                r#"UPDATE flows SET status = 'completed',
                   duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
                   completed_at = now()
                   WHERE flow_id = $1"#,
            ).bind(&flow_id_cleanup).execute(&db).await;
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
            kind: TurnKind::User,
        },
    )
    .await
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn to_sse(event: StreamResponse) -> Event {
    Event::default().data(a2a::to_sse_data(&event))
}

/// Upsert a `chat_sessions` row and persist the user's message for an
/// orchestrator-routed chat, mirroring what `agent_proxy.rs`'s
/// `ensure_chat_session` already does for direct agent chat. `agent_id` is
/// left `NULL` (unlike agent_proxy's fixed target) since the orchestrator
/// can route to a different agent on every turn of the same session.
/// Fire-and-forget: session bookkeeping here is a nice-to-have for
/// `sessions`/`history`/resume, not a security boundary like agent_proxy's
/// version (which also authorizes access to an existing session).
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

pub(crate) async fn resolve_endpoint(
    state: &AppState,
    agent_id: &str,
    agent_name: &str,
) -> Result<String, String> {
    // Containers are UUID-keyed (see `build_agent_spec`), so the runtime lookup
    // below needs `agent_id`, not `agent_name` — a name-keyed lookup always
    // misses. `agent_name` is kept only for the DB fallback query and error
    // messages, which are keyed by name for readability.
    let agent_id: Uuid = agent_id
        .parse()
        .map_err(|e| format!("invalid agent id: {e}"))?;
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT transport_path, url FROM agents WHERE id = $1 AND status = 'running'",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("db lookup: {e}"))?;

    let Some((transport_path, stored_url)) = row else {
        return Err(format!("no running agent named '{agent_name}'"));
    };

    // The A2A spec fixes no path — it must come from the agent's card, never
    // be assumed. The a2a-server-lf crate (used by the example agents) mounts
    // its JSON-RPC handler at the container root, not `/jsonrpc` — a row with
    // no captured transport_path must default to root, not guess a path the
    // agent doesn't actually serve.
    let path = match transport_path.as_deref() {
        None | Some("/") | Some("") => "",
        Some(p) => p,
    };

    // Prefer live runtime endpoint (Docker port mapping can change on restart).
    let container_id = nasiko_runtime::ContainerId::from_uuid(agent_id);
    match state.runtime.endpoint(&container_id).await {
        Ok(endpoint) => {
            let base = endpoint.trim_end_matches('/');
            return Ok(format!("{base}{path}"));
        }
        Err(_) => {
            // Container not reachable via runtime — check if it's actually stopped.
            if let Ok(status) = state.runtime.status(&container_id).await
                && status.state != nasiko_runtime::RuntimeState::Running
            {
                // Mark as stopped so future routing skips it.
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'stopped' WHERE id = $1 AND status = 'running'",
                )
                .bind(agent_id)
                .execute(&state.db)
                .await;
                return Err(format!("agent '{agent_name}' is not running"));
            }
        }
    }

    // Fall back to stored URL (e.g. external agents, K8s with stable DNS).
    if let Some(ref url) = stored_url
        && !url.is_empty()
    {
        let u = url.trim_end_matches('/');
        return Ok(format!("{u}{path}"));
    }

    Err(format!("no endpoint found for agent '{agent_name}'"))
}

/// Normalize a Python a2a-sdk JSONRPC event to CP native StreamResponse format.
/// Extract the error message if this SSE event represents a TASK_STATE_FAILED status update.
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
        // session load, and `deliver`'s `session_traces` insert is guarded on this being `Some`,
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
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return data.to_string();
    };

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
}

#[derive(Debug)]
pub enum A2aDispatchError {
    InvalidRequest(String),
    NoAgents,
    AgentNotFound(String),
    Internal(String),
    /// A `reconnect_after_hitl_id` reconnect was for a HITL row that exists but isn't the
    /// caller's — same rule `authorize_hitl_action` enforces for every other `/api/hitl/*` route.
    Forbidden(String),
}

impl IntoResponse for A2aDispatchError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
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
