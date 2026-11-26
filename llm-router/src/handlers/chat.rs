//! `POST /v1/chat/completions` — verify JWT → resolve config → call provider →
//! render OpenAI-shaped response (non-streaming JSON or streaming SSE), with a
//! fire-and-forget usage row.
//!
//! The Axum entry point is a thin wrapper that builds a `PgRegistry` from the shared
//! pool; the real work lives in [`chat_core`], which takes a `&dyn RegistryStore` so
//! the whole path is testable without a database.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use futures::StreamExt;
use serde_json::Value;
use tracing::Instrument;

use futures::stream::BoxStream;

use nasiko_session_token::SessionClaims;

use super::auth_layer::rejected_body;
use crate::LlmRouterCtx;
use crate::auth::{caller_credential, verify_agent_token, verify_session_credential};
use crate::error::GatewayError;
use crate::inbound::{ChatStreamRenderer, InboundFormat, inbound_for};
use crate::ir::{ChatChunk, Usage};
use crate::payer;
use crate::providers::{ProviderError, fallback};
use crate::resolver::{ConfigSubject, PgRegistry, RegistryStore, RequestHint, resolve_for};
use crate::routing::boundary::{TRACEPARENT_HEADER, parse_flow_id};
use crate::routing::{self, BoundarySignals, RouteInputs};
use crate::turns::{self, StreamedReply, TurnRecorder, TurnStart};
use crate::usage::{self, UsageRecord};

#[derive(Clone)]
pub(crate) struct RoutedRequest {
    pub agent_id: String,
    pub owner_id: String,
    pub resolved: crate::resolver::ResolvedConfig,
    pub flow_id: Option<String>,
    pub attribution_source: Option<routing::attribution::AttributionSource>,
    /// What payload compression did to this request, for its usage rows. Set by the Responses
    /// surface after routing; `chat_core` passes its own stats straight to its rows instead.
    pub compression: crate::compress::Ledger,
    /// Pre-serialized `metadata.routing` block for this request's calls
    /// (`RouteDecision::to_metadata`): which precedence level chose the model, which tier it drew,
    /// and whether that overrode the resolver's model (NAS-1263). Every call of the request —
    /// success, failed attempt, streamed — carries the same block, since the model was chosen once.
    pub routing_metadata: Option<serde_json::Value>,
    pub sticky_conv_id: Option<String>,
}

/// Prompt-derived signals `resolve_routed_request` needs beyond the resolved config,
/// gathered once per format-specific handler since each wire format shapes its transcript
/// differently (chat.rs's IR `Message` list vs. responses.rs's Responses-API `input` array).
pub(crate) struct RequestSignals {
    /// Latest user turn's text — the classifier's `query` input (Level 3) for every agent,
    /// and (for a coding-agent integration) also the `conv_id` anchor for *this* turn.
    pub query: Option<String>,
    /// Count of top-level user turns so far. Combined with `query`, anchors a coding-agent's
    /// `conv_id` to the current turn rather than the whole session — see
    /// `BoundarySignals::for_coding_agent`'s doc comment for why that distinction matters.
    /// Only used when the resolved agent is a coding-agent integration.
    pub turn_ordinal: usize,
    /// Whether the transcript's last turn is a tool result — keeps a coding-agent's
    /// in-flight tool loop sticky. Only used when the resolved agent is a coding-agent
    /// integration.
    pub is_tool_continuation: bool,
}

/// Record a call's four token classes on its `gen_ai` span.
///
/// Normalizes first, so the span carries the same disjoint counts the
/// `token_usage` row does. Recording `prompt_tokens` untouched — which is what
/// this used to do — publishes OpenAI's *inclusive* prompt with no cache
/// attributes beside it, and a reader has no way to tell that from a call that
/// missed cache entirely. It then prices the cached portion at the full input
/// rate, so the same call costs more on the FinOps dashboard than in the
/// metering table it was billed from.
/// Record how many tools the model asked for in one reply (`nasiko.response.tool_calls`), which
/// the dashboard's trace sync counts for agents that emit no tool spans of their own.
fn record_tool_calls(span: &tracing::Span, count: usize) {
    if count > 0 {
        span.record("nasiko.response.tool_calls", count as i64);
    }
}

fn record_span_usage(span: &tracing::Span, usage: Option<&Usage>) {
    let Some(usage) = usage else {
        return;
    };
    let mut usage = usage.clone();
    usage.normalize_openai_details();

    if let Some(input) = usage.prompt_tokens {
        span.record("gen_ai.usage.input_tokens", input);
    }
    if let Some(output) = usage.completion_tokens {
        span.record("gen_ai.usage.output_tokens", output);
    }
    if let Some(cache_read) = usage.cache_read_input_tokens {
        span.record("gen_ai.usage.cache_read_input_tokens", cache_read);
    }
    if let Some(cache_creation) = usage.cache_creation_input_tokens {
        span.record("gen_ai.usage.cache_creation_input_tokens", cache_creation);
    }
    // The provider's own total, not a re-derived one: it is what settles the
    // prompt convention for a reader, and re-deriving it here would just echo
    // our own normalization back.
    if let Some(total) = usage.total_tokens {
        span.record("gen_ai.usage.total_tokens", total);
    }
}

/// Who is calling the router.
#[derive(Clone)]
pub enum AuthedCaller {
    /// The deploy-time agent-identity token: `{agent_id, owner_id}`. Identity for
    /// the actual turn is still inferred from the request's `traceparent`.
    Agent { agent_id: String, owner_id: String },
    /// A coding-agent session token (NAS-839): signed
    /// `{agent_id, user_id, session_id, flow_id}`, so the user and flow are read
    /// straight off the claims with no `traceparent` and no freshness window.
    Session(SessionClaims),
}

/// Authenticate the caller, preferring a coding-agent session token and falling
/// back to the agent-identity token.
///
/// A valid, non-revoked session token wins; anything that isn't one of our
/// session tokens falls through to [`verify_agent_token`], so A2A agents are
/// unaffected. Expiry and revocation are hard failures (fail closed).
pub(crate) async fn authenticate_caller(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    headers: &HeaderMap,
) -> Result<AuthedCaller, GatewayError> {
    let authz = caller_credential(headers);
    if let Some(claims) = verify_session_credential(authz, &ctx.cfg, &ctx.db).await? {
        return Ok(AuthedCaller::Session(claims));
    }

    let (agent_id, owner_id) = verify_agent_identity(ctx, store, authz).await?;
    Ok(AuthedCaller::Agent { agent_id, owner_id })
}

/// Verify an agent-identity token and check its `owner_id` claim is still the
/// agent's owner.
///
/// The token is minted at deploy time and lives in the container until the
/// next deploy. Ownership can change in between (`PUT /api/agents/{id}/owner`),
/// and the owner decides whose provider keys pay for the call. Trusting the
/// claim let the new owner's agent spend the previous owner's keys; switching
/// silently to the new owner would let anyone still holding the old token
/// spend the new owner's. So a stale token is refused until a redeploy mints a
/// fresh one. A deleted agent is refused too. A token with no owner claim
/// already takes the platform-key path and is left as it is.
pub(crate) async fn verify_agent_identity(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    authorization: Option<&str>,
) -> Result<(String, String), GatewayError> {
    let token = verify_agent_token(authorization, &ctx.cfg)?;
    let (agent_id, owner_id) = (token.agent_id, token.owner_id);
    let id = uuid::Uuid::parse_str(&agent_id)
        .map_err(|_| GatewayError::Forbidden("token agent_id is not a UUID".into()))?;
    let identity = store
        .fetch_agent_identity(id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "llm-router: agent owner lookup failed — failing closed");
            GatewayError::Forbidden("identity store unavailable".into())
        })?
        .ok_or_else(|| GatewayError::Forbidden("agent not found for token".into()))?;
    if is_revoked(token.issued_at, identity.llm_tokens_revoked_at) {
        tracing::info!(%agent_id, "llm-router: agent token predates its LLM token revocation");
        return Err(GatewayError::TokenRevoked);
    }
    let current_owner = identity.owner_id.to_string();
    if !owner_id.is_empty() && owner_id != current_owner {
        tracing::warn!(%agent_id, token_owner = %owner_id, %current_owner, "llm-router: agent token predates an ownership change");
        return Err(GatewayError::Forbidden(
            "the agent's owner changed since it was deployed; redeploy it to refresh its credentials"
                .into(),
        ));
    }
    Ok((agent_id, owner_id))
}

/// True when a token issued at `issued_at` (Unix seconds) falls before the agent's last LLM token
/// revocation. Compared at whole-second precision, the precision of `iat`: a token issued in the
/// same second as the revocation is accepted, so a reconnect straight after a disconnect works. A
/// token without `iat` cannot prove it came after a revocation, so it is refused once one exists.
fn is_revoked(issued_at: Option<i64>, revoked_at: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    let Some(revoked_at) = revoked_at else {
        return false;
    };
    issued_at.is_none_or(|issued| issued < revoked_at.timestamp())
}

/// The owner of the agent a session token names — needed for config/secret
/// resolution, which is keyed on the agent owner, not the (possibly different)
/// session user the token bills.
pub(crate) async fn lookup_agent_owner(
    db: &sqlx::PgPool,
    agent_id: &str,
) -> Result<String, GatewayError> {
    let id = uuid::Uuid::parse_str(agent_id)
        .map_err(|_| GatewayError::Forbidden("token agent_id is not a UUID".into()))?;
    let owner: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT owner_id FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "llm-router: agent owner lookup failed — failing closed");
        GatewayError::Forbidden("identity store unavailable".into())
    })?;
    owner
        .map(|u| u.to_string())
        .ok_or_else(|| GatewayError::Forbidden("agent not found for token".into()))
}

/// Axum handler for the OpenAI surface (`POST /v1/chat/completions`).
pub async fn chat_completions(
    State(ctx): State<LlmRouterCtx>,
    Extension(caller): Extension<AuthedCaller>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, GatewayError> {
    let body = match body {
        Ok(Json(body)) => body,
        Err(rejection) => {
            return Ok(rejected_body(
                rejection,
                ctx.cfg.max_body_bytes,
                IntoResponse::into_response,
            ));
        }
    };
    let store = PgRegistry::new(ctx.db.clone());
    chat_core_as(
        &ctx,
        &store,
        caller,
        &headers,
        body,
        InboundFormat::OpenAi,
        None,
    )
    .await
}

/// Axum handler for the Anthropic surface (`POST /v1/messages`). Same core; the inbound
/// format selects the Anthropic parser/renderer (P2.3).
pub async fn messages(
    State(ctx): State<LlmRouterCtx>,
    Extension(caller): Extension<AuthedCaller>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, GatewayError> {
    let body = match body {
        Ok(Json(body)) => body,
        Err(rejection) => {
            return Ok(rejected_body(
                rejection,
                ctx.cfg.max_body_bytes,
                IntoResponse::into_response,
            ));
        }
    };
    let store = PgRegistry::new(ctx.db.clone());
    chat_core_as(
        &ctx,
        &store,
        caller,
        &headers,
        body,
        InboundFormat::Anthropic,
        None,
    )
    .await
}

pub async fn count_tokens(
    State(ctx): State<LlmRouterCtx>,
    Extension(caller): Extension<AuthedCaller>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, GatewayError> {
    let body = match body {
        Ok(Json(body)) => body,
        Err(rejection) => {
            return Ok(rejected_body(
                rejection,
                ctx.cfg.max_body_bytes,
                IntoResponse::into_response,
            ));
        }
    };
    let store = PgRegistry::new(ctx.db.clone());
    count_tokens_as(&ctx, &store, caller, body).await
}

async fn count_tokens_as(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    caller: AuthedCaller,
    mut body: Value,
) -> Result<Response, GatewayError> {
    let (agent_id, owner_id) = match caller {
        AuthedCaller::Agent { agent_id, owner_id } => (agent_id, owner_id),
        AuthedCaller::Session(claims) => {
            let owner_id = lookup_agent_owner(&ctx.db, &claims.agent_id).await?;
            (claims.agent_id, owner_id)
        }
    };
    let hint = RequestHint {
        provider: Some(InboundFormat::Anthropic.provider_label()),
        model: body["model"].as_str(),
    };
    let resolved =
        crate::resolver::resolve(store, &ctx.cache, &ctx.cfg, &agent_id, &owner_id, hint).await?;
    if resolved.provider != "anthropic" {
        return Ok((
            axum::http::StatusCode::NOT_IMPLEMENTED,
            crate::error::error_body(
                axum::http::StatusCode::NOT_IMPLEMENTED,
                &format!(
                    "count_tokens is available only when the agent's model is served by Anthropic (this agent uses {})",
                    resolved.provider
                ),
            ),
        )
            .into_response());
    }
    body["model"] = Value::String(resolved.model);
    let counted = crate::providers::anthropic::count_tokens(
        &ctx.http,
        &ctx.cfg.anthropic_api_base,
        &resolved.api_key,
        &body,
    )
    .await?;
    Ok(Json(counted).into_response())
}

/// Axum handler for the Gemini surface (`POST /v1beta/models/{model}:{method}`). Gemini
/// signals streaming by the endpoint method (`:streamGenerateContent`), not a body field,
/// so we force the stream flag from the path (P2.4).
pub async fn gemini_generate(
    State(ctx): State<LlmRouterCtx>,
    Extension(caller): Extension<AuthedCaller>,
    Path(model_method): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, GatewayError> {
    let body = match body {
        Ok(Json(body)) => body,
        Err(rejection) => {
            return Ok(rejected_body(
                rejection,
                ctx.cfg.max_body_bytes,
                IntoResponse::into_response,
            ));
        }
    };
    let stream = model_method
        .rsplit(':')
        .next()
        .map(|m| m.eq_ignore_ascii_case("streamGenerateContent"))
        .unwrap_or(false);
    let store = PgRegistry::new(ctx.db.clone());
    chat_core_as(
        &ctx,
        &store,
        caller,
        &headers,
        body,
        InboundFormat::Gemini,
        Some(stream),
    )
    .await
}

/// Storage-agnostic core of the chat handler. `format` selects the inbound parser/
/// renderer; `force_stream` overrides the request's `stream` flag when the wire protocol
/// signals streaming out-of-band (Gemini's endpoint method). The canonical IR, resolver,
/// providers, and fallbacks are format-agnostic.
#[cfg(test)]
async fn chat_core(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    headers: &HeaderMap,
    body: Value,
    format: InboundFormat,
    force_stream: Option<bool>,
) -> Result<Response, GatewayError> {
    let caller = authenticate_caller(ctx, store, headers).await?;
    chat_core_as(ctx, store, caller, headers, body, format, force_stream).await
}

async fn chat_core_as(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    caller: AuthedCaller,
    headers: &HeaderMap,
    body: Value,
    format: InboundFormat,
    force_stream: Option<bool>,
) -> Result<Response, GatewayError> {
    // A coding-agent session token carries the user + flow in signed claims; the
    // agent-identity token does not, and still relies on the request traceparent.
    let (agent_id, owner_id, session) = match caller {
        AuthedCaller::Agent { agent_id, owner_id } => (agent_id, owner_id, None),
        AuthedCaller::Session(claims) => {
            // Config/secrets resolve on the agent owner, which the token doesn't
            // carry — look it up; the session user is only the billing subject.
            let owner_id = lookup_agent_owner(&ctx.db, &claims.agent_id).await?;
            (claims.agent_id.clone(), owner_id, Some(claims))
        }
    };
    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id, %owner_id, ?format,
        session_bound = session.is_some(),
        has_traceparent = headers.get(TRACEPARENT_HEADER).is_some(),
        "chat_core: request received (credential verified)"
    );

    let inbound = inbound_for(format);
    let mut req = inbound.parse_chat(body)?;
    if let Some(stream) = force_stream {
        req.stream = Some(stream);
    }
    tracing::debug!(
        target: "nasiko::llm_router::chat",
        %agent_id,
        message_count = req.messages.len(),
        streaming = req.is_streaming(),
        requested_model = ?req.model,
        "chat_core: parsed inbound request (NOTE: requested_model is authoritative only when the agent has no llm_config — see resolver)"
    );
    // No-llm_config agents are routed to what the request itself asked for: the provider
    // implied by the inbound SDK surface + the request body's model (defaults are the
    // last-resort safety net). A configured agent ignores this hint.
    let hint = RequestHint {
        provider: Some(format.provider_label()),
        model: req.model.as_deref(),
    };
    let signals = RequestSignals {
        query: routing::latest_user_query(&req.messages).map(crate::attachments::strip_markers),
        turn_ordinal: routing::user_turn_ordinal(&req.messages),
        is_tool_continuation: routing::is_tool_continuation(&req.messages),
    };
    // Capture the session id before `session` is moved into routing below; used to
    // stamp `session.id` on the gen_ai span so a coding agent's orphan LLM trace is
    // discoverable by session (see the span record further down).
    let session_id_for_span: Option<String> = session.as_ref().map(|s| s.session_id.clone());
    let routed = resolve_routed_request(
        ctx, store, headers, agent_id, owner_id, hint, signals, session,
    )
    .await?;
    let RoutedRequest {
        agent_id,
        owner_id,
        resolved,
        flow_id,
        attribution_source,
        compression: _,
        routing_metadata,
        sticky_conv_id,
    } = routed;
    // A discovered agent's call is also a turn of its conversation (see `crate::turns`). Read
    // the user message before compression can rewrite the request.
    // In a tool loop the latest user message is the question every call of the loop repeats,
    // so a call answering tool results is recorded as that instead.
    let turn = resolved.is_discovered_agent.then(|| TurnStart {
        user_text: if routing::is_tool_continuation(&req.messages) {
            turns::TOOL_RESULTS.to_string()
        } else {
            routing::latest_user_query(&req.messages).unwrap_or_default()
        },
        started_at: chrono::Utc::now(),
        span: tracing::Span::current(),
    });

    crate::attachments::expand::attach_marked_files(ctx, &mut req, flow_id.as_deref(), &agent_id)
        .await?;

    // ── compression seam ──────────────────────────────────────────────────────────────────
    // After `resolve_routed_request`, not before it: `RequestSignals` (built at :159 from
    // `req.messages`) feeds the classifier, the salience gate and the `conv_id` that keys the
    // Redis decision cache, so compressing first could flip the selected model mid-conversation.
    // Resolving first also puts `resolved` in scope, which is what makes the policy per-agent.
    //
    // Keep this to one statement — `hint` holds a shared borrow of `req` that ends at :165, and
    // any later read of it would turn this `&mut req` into E0502.
    let compress_policy = crate::compress::policy_for(&ctx.cfg, &resolved);
    tracing::info!(
        target: "nasiko::llm_router::compress",
        %agent_id,
        compress_enabled = resolved.compress_enabled,
        kill_switch = ctx.cfg.compress_kill_switch,
        policy_enabled = compress_policy.enabled,
        "compress: policy for this request"
    );

    let recovery = crate::compress::Recovery::for_request(
        ctx.cfg.compress_recovery_enabled,
        ctx.cfg.compress_recovery_min_bytes,
        flow_id.as_deref(),
        &req,
    );

    let compression = crate::compress::apply(&mut req, &compress_policy, recovery);
    if compression.messages_touched > 0 {
        tracing::debug!(
            target: "nasiko::llm_router::compress",
            %agent_id,
            applied = compression.applied,
            dry_run = compression.dry_run,
            level = compression.level,
            bytes_in = compression.bytes_in,
            bytes_out = compression.bytes_out,
            messages_touched = compression.messages_touched,
            elapsed_us = compression.elapsed_us,
            "compress: tool results reduced"
        );
    }

    // The markers naming these handles are already in `req`; the row has to exist before the
    // request carrying them goes out (see `recovery`'s module docs). Only a flow-scoped request
    // can be recovered, so one without a flow id stores nothing.
    if !compression.originals.is_empty() {
        match (flow_id.as_deref(), owner_id.parse::<uuid::Uuid>()) {
            (Some(flow), Ok(owner)) => {
                crate::recovery::persist(
                    &ctx.db,
                    &compression.originals,
                    flow,
                    owner,
                    agent_id.parse().ok(),
                )
                .await
            }
            _ => tracing::debug!(
                target: "nasiko::llm_router::recovery",
                %agent_id,
                count = compression.originals.len(),
                "recovery: request is not flow-scoped; originals not stored"
            ),
        }
    }

    // ── brevity seam (IP-2) ───────────────────────────────────────────────────────────────
    // After compression, so the size floor is judged on the bytes actually being sent, and so a
    // compressed tool result cannot push a turn over the floor it would otherwise miss.
    let brevity = crate::brevity::apply(&mut req, &ctx.cfg, &resolved, flow_id.as_deref());
    let brevity_metadata = Some(crate::brevity::to_metadata(
        &brevity,
        crate::brevity::DIRECTIVE.len(),
    ));
    tracing::debug!(
        target: "nasiko::llm_router::brevity",
        %agent_id,
        applied = brevity.is_ok(),
        skipped = ?brevity.err(),
        "brevity: directive decision"
    );

    // ── compact tool schemas (opt-in, `TOKEN_COMPACT_TOOLS`) ──────────────────────────────
    // After brevity and before `sent_bytes`, so the ledger measures the payload actually sent.
    // Off (the default) or skipped, `req` is untouched and the reply is not inspected.
    let compact = crate::compact_tools::apply(&mut req, &ctx.cfg, &resolved);
    tracing::debug!(
        target: "nasiko::llm_router::compact_tools",
        %agent_id,
        applied = compact.is_ok(),
        skipped = compact.as_ref().err().map(|s| s.as_label()),
        "compact_tools: decision"
    );
    // The usage row's `metadata.compact_tools`, one block per provider call: what it did with
    // compact schemas and the tool-definition bytes it sent, against what native would have.
    let native_tool_bytes = match &compact {
        Ok(compacted) => compacted.bytes.0,
        Err(_) => crate::compact_tools::native_tool_bytes(&req, &ctx.cfg, &resolved),
    };
    let compact_tools_metadata = |outcome, tool_bytes_sent| {
        Some(crate::compact_tools::to_metadata(
            outcome,
            tool_bytes_sent,
            native_tool_bytes,
        ))
    };
    let mut compact_outcome = match &compact {
        Ok(compacted) => (crate::compact_tools::Outcome::Applied, compacted.bytes.1),
        Err(reason) => (
            crate::compact_tools::Outcome::Skipped(*reason),
            native_tool_bytes,
        ),
    };

    // ── savings ledger inputs ─────────────────────────────────────────────────────────────
    // Measured here, after both seams, because this is the payload the provider will actually
    // bill for — which is what makes `sent_bytes / reported_input_tokens` a calibration rather
    // than a guess (savings.rs). Only a reduction that really happened is credited: `applied` is
    // already false for a dry run and for a pass that found nothing to shrink.
    let sent_bytes = crate::brevity::estimated_bytes(&req);
    let compress_bytes = compression
        .applied
        .then_some((compression.bytes_in, compression.bytes_out));

    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id,
        litellm_model = %resolved.litellm_model,
        provider = %resolved.provider,
        fallback_models = ?resolved.fallback_models,
        streaming = req.is_streaming(),
        "chat_core: final model selected — dispatching to provider"
    );

    // Server-side gen_ai span — records the *actual* provider and resolved model so
    // traces show the truth even when the agent-side OTel instrumentation labels the
    // span by the SDK name (e.g. "openai") instead of the real upstream.
    let llm_span = tracing::info_span!(
        "gen_ai.chat",
        otel.kind = "client",
        gen_ai.operation.name = "chat",
        gen_ai.request.model = %resolved.model,
        gen_ai.provider.name = %resolved.provider,
        gen_ai.agent.id = %agent_id,
        // Set for a session caller (coding agent) below, so this span's trace is
        // discoverable by `session.id` like a normal agent's own spans are.
        session.id = tracing::field::Empty,
        gen_ai.response.model = tracing::field::Empty,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        nasiko.compress.bytes_in = tracing::field::Empty,
        nasiko.compress.bytes_out = tracing::field::Empty,
        nasiko.compress.elapsed_us = tracing::field::Empty,
        nasiko.brevity.applied = brevity.is_ok(),
        // The cache classes are recorded too, or the trace-derived cost of a
        // cached call is wrong in a way nothing downstream can detect: an
        // absent cache attribute is indistinguishable from a cache miss, so the
        // whole prompt gets billed at the full input rate.
        gen_ai.usage.cache_read_input_tokens = tracing::field::Empty,
        gen_ai.usage.cache_creation_input_tokens = tracing::field::Empty,
        gen_ai.usage.total_tokens = tracing::field::Empty,
        // How many tools the model asked for in this reply: an agent that runs its own tools
        // emits no tool spans, so this is the only record of them the dashboard can count.
        nasiko.response.tool_calls = tracing::field::Empty,
        nasiko.usage.prompt_convention = "exclusive",
    );
    if compression.messages_touched > 0 {
        llm_span.record("nasiko.compress.bytes_in", compression.bytes_in);
        llm_span.record("nasiko.compress.bytes_out", compression.bytes_out);
        llm_span.record("nasiko.compress.elapsed_us", compression.elapsed_us);
    }
    // A coding agent (session-token caller) emits no OTel spans of its own and
    // doesn't propagate the turn's traceparent, so this router span is the only
    // record of the call and would otherwise sit in an orphan trace. Stamping the
    // session id links it to the conversation, so the session/FinOps views (which
    // discover traces by `session.id`) find and attribute it.
    if let Some(sid) = session_id_for_span.as_deref() {
        llm_span.record("session.id", sid);
    }

    let started = Instant::now();
    let platform_paid = resolved.platform_paid;
    // Every attempt that reached a provider and failed is billed as its own call, whether or
    // not a fallback then succeeded (the success row is written below, unchanged).
    let failed_scope = usage::CallScope {
        owner_id: owner_id.clone(),
        agent_id: agent_id.clone(),
        operation_type: "direct_llm",
        streaming: req.is_streaming(),
        flow_id: flow_id.clone(),
        attribution_source,
        request_bytes: Some(sent_bytes),
        routing_metadata: routing_metadata.clone(),
    };
    let mut attempts = crate::providers::attempts::AttemptLog::default();

    if req.is_streaming() {
        let established =
            fallback::execute_chat_stream(&ctx.http, &ctx.cfg, &resolved, &req, &mut attempts)
                .instrument(llm_span.clone())
                .await;
        usage::spawn_log_failed(
            &ctx.db,
            &ctx.pricing,
            &ctx.ledger,
            &failed_scope,
            attempts.take_failed(),
        );
        let (stream, (provider, model)) = established?;
        forget_route_that_did_not_answer(
            ctx,
            &agent_id,
            sticky_conv_id.as_deref(),
            &resolved,
            (&provider, &model),
        )
        .await;
        llm_span.record("gen_ai.response.model", model.as_str());
        let renderer = inbound.chat_stream_renderer();
        return stream_chat(StreamChatArgs {
            ctx,
            renderer,
            provider_stream: stream,
            provider,
            model,
            agent_id,
            owner_id,
            started,
            flow_id,
            attribution_source,
            platform_paid,
            compress_metadata: compression.to_metadata(),
            brevity_metadata: brevity_metadata.clone(),
            compact_tools_metadata: compact_tools_metadata(compact_outcome.0, compact_outcome.1),
            routing_metadata,
            compress_bytes,
            request_bytes: Some(sent_bytes),
            span: llm_span.clone(),
            turn,
        });
    }

    // Non-streaming: run with ordered fallbacks; usage records the effective provider/model.
    // One deadline for the whole request, compact-tools re-send included.
    let deadline = fallback::provider_deadline(&ctx.cfg);
    let answered = fallback::execute_chat_by(
        &ctx.http,
        &ctx.cfg,
        &resolved,
        &req,
        deadline,
        &mut attempts,
    )
    .instrument(llm_span.clone())
    .await;
    usage::spawn_log_failed(
        &ctx.db,
        &ctx.pricing,
        &ctx.ledger,
        &failed_scope,
        attempts.take_failed(),
    );
    let (mut resp, (mut provider, mut model)) = answered?;
    forget_route_that_did_not_answer(
        ctx,
        &agent_id,
        sticky_conv_id.as_deref(),
        &resolved,
        (&provider, &model),
    )
    .await;

    // Credited only if the compact reply is the one returned; a native re-send below clears it.
    let mut compact_tools_bytes = compact.as_ref().ok().map(|c| c.bytes);

    // Compact calls → native `tool_calls`. A reply that does not decode is never guessed at:
    // the failed attempt is billed as its own row and the original native request is re-sent.
    if let Ok(compacted) = &compact
        && let Err(e) = crate::compact_tools::decode_response(&mut resp, &compacted.tools)
    {
        tracing::warn!(
            target: "nasiko::llm_router::compact_tools",
            %agent_id,
            error = e,
            "compact_tools: reply did not decode; retrying with native tools"
        );
        usage::spawn_log(
            ctx.db.clone(),
            ctx.pricing.clone(),
            ctx.ledger.clone(),
            UsageRecord {
                owner_id: owner_id.clone(),
                agent_id: agent_id.clone(),
                operation_type: "direct_llm",
                provider: provider.clone(),
                model: model.clone(),
                usage: resp.usage.clone(),
                cached_tokens: None,
                reasoning_tokens: None,
                latency_ms: started.elapsed().as_millis() as i64,
                streaming: false,
                finish_reason: Some("compact_decode_error".into()),
                flow_id: flow_id.clone(),
                attribution_source,
                platform_paid,
                compress_metadata: compression.to_metadata(),
                brevity_metadata: brevity_metadata.clone(),
                compact_tools_metadata: compact_tools_metadata(
                    crate::compact_tools::Outcome::Retried,
                    compacted.bytes.1,
                ),
                routing_metadata: routing_metadata.clone(),
                compress_bytes,
                // The schemas this call saved are re-sent in full below, so nothing was saved.
                compact_tools_bytes: None,
                request_bytes: Some(sent_bytes),
                // The provider answered; only the router could not decode the reply.
                failure: None,
            },
        );
        compact_tools_bytes = None;
        compact_outcome = (
            crate::compact_tools::Outcome::NativeResend,
            native_tool_bytes,
        );
        let resent = fallback::execute_chat_by(
            &ctx.http,
            &ctx.cfg,
            &resolved,
            &compacted.native,
            deadline,
            &mut attempts,
        )
        .instrument(llm_span.clone())
        .await;
        usage::spawn_log_failed(
            &ctx.db,
            &ctx.pricing,
            &ctx.ledger,
            &failed_scope,
            attempts.take_failed(),
        );
        (resp, (provider, model)) = resent?;
    }
    let latency_ms = started.elapsed().as_millis() as i64;

    // Record effective model and token usage on the server-side gen_ai span.
    llm_span.record("gen_ai.response.model", model.as_str());
    record_span_usage(&llm_span, resp.usage.as_ref());
    record_tool_calls(
        &llm_span,
        resp.choices
            .first()
            .and_then(|c| c.message.tool_calls.as_ref())
            .map_or(0, Vec::len),
    );

    if let Some(turn) = turn {
        ctx.turn_recorder.record(turn.finish(
            (agent_id.clone(), owner_id.clone()),
            turns::reply_text(&resp),
            model.clone(),
            resp.usage.as_ref(),
            latency_ms,
        ));
    }
    usage::spawn_log(
        ctx.db.clone(),
        ctx.pricing.clone(),
        ctx.ledger.clone(),
        UsageRecord {
            owner_id,
            agent_id,
            operation_type: "direct_llm",
            provider,
            model,
            usage: resp.usage.clone(),
            cached_tokens: None,
            reasoning_tokens: None,
            latency_ms,
            streaming: false,
            finish_reason: resp.choices.first().and_then(|c| c.finish_reason.clone()),
            flow_id,
            attribution_source,
            platform_paid,
            compress_metadata: compression.to_metadata(),
            brevity_metadata: brevity_metadata.clone(),
            compact_tools_metadata: compact_tools_metadata(compact_outcome.0, compact_outcome.1),
            routing_metadata,
            compress_bytes,
            compact_tools_bytes,
            request_bytes: Some(sent_bytes),
            failure: None,
        },
    );

    Ok(Json(inbound.render_chat_response(resp)).into_response())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_routed_request(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    headers: &HeaderMap,
    agent_id: String,
    owner_id: String,
    hint: RequestHint<'_>,
    signals: RequestSignals,
    session: Option<SessionClaims>,
) -> Result<RoutedRequest, GatewayError> {
    // Attribution runs before config resolution: the attributed user decides whose config and
    // keys pay (a shared agent runs on its caller's own, or on what the payer says,
    // `payer::subject_for_call`).
    //
    // A coding-agent CLI (Claude Code, Codex, OpenCode, Cursor) is never dispatched through
    // the orchestrator, so it never has a `flows` row — derive_boundary_signals's
    // traceparent lookup is a permanent dead end for it (see that fn's doc comment), which
    // otherwise pins every request to Level 4 (the agent's configured `llm_config`) and
    // makes the prompt classifier (Level 3) unreachable. Derive signals from the transcript
    // itself instead for these agents. An A2A agent the CLI discovered on a user's machine is
    // in the same position (Nasiko never calls it, so no flow exists), and its calls are
    // likewise billed to its owner rather than refused for a missing traceparent.
    let (boundary, flow_id, billed_user_id, attribution_source, query, acting_user) =
        if let Some(s) = session.as_ref() {
            // Session-bound coding agent: the signed token names the user, so bill that
            // user. The token's flow is the first turn's; the flow is resolved from the
            // session (or a traceparent, if one was sent). No freshness window (a long
            // turn is never rejected for age).
            let trace_flow = headers
                .get(TRACEPARENT_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_flow_id);
            let attributed = store.attribute_session_flow(trace_flow.as_deref(), s).await;
            (
                BoundarySignals::for_coding_agent(
                    &agent_id,
                    signals.turn_ordinal,
                    signals.query.as_deref(),
                    signals.is_tool_continuation,
                ),
                Some(attributed.flow_id),
                s.user_id.clone(),
                Some(attributed.source.into()),
                signals.query,
                // A session token's agent is the user's own: their coding-agent integration or
                // their harness agent (a session opened before users had one names the platform's
                // harness agent). None is shared through access settings, so it keeps resolving
                // on the agent owner, which for the user's own agent is the user.
                None,
            )
        } else if runs_outside_flows(store, &agent_id).await? {
            (
                BoundarySignals::for_coding_agent(
                    &agent_id,
                    signals.turn_ordinal,
                    signals.query.as_deref(),
                    signals.is_tool_continuation,
                ),
                None,
                owner_id.clone(),
                None,
                signals.query,
                None,
            )
        } else {
            let raw_traceparent = headers
                .get(TRACEPARENT_HEADER)
                .and_then(|value| value.to_str().ok());
            let trace_flow = raw_traceparent.and_then(parse_flow_id);
            let attribution = routing::attribution::resolve(
                store,
                &agent_id,
                trace_flow,
                ctx.cfg.attribution_window_secs as i64,
            )
            .await
            .map_err(|denied| {
                GatewayError::Forbidden(format!(
                    "{denied} (received traceparent: {})",
                    raw_traceparent.unwrap_or("<none>")
                ))
            })?;
            let billed_user_id = attribution
                .user_id
                .map(|user_id| user_id.to_string())
                .unwrap_or_else(|| owner_id.clone());
            (
                boundary_signals_for(&attribution),
                Some(attribution.flow_id.clone()),
                billed_user_id,
                Some(attribution.source),
                routing_query(attribution.user_message, signals.query),
                attribution.user_id,
            )
        };
    let (subject, retry_after) =
        payer::subject_for_call(store, ctx.payer.as_ref(), &agent_id, &owner_id, acting_user)
            .await?;
    let mut resolved =
        match resolve_for(store, &ctx.cache, &ctx.cfg, &agent_id, subject, hint).await {
            Err(no_key @ GatewayError::NoApiKey { .. })
                if matches!(subject, ConfigSubject::Owner(_)) =>
            {
                match payer::cover_owner_call(ctx.payer.as_ref(), &owner_id, acting_user).await? {
                    Some(covered) => {
                        resolve_for(store, &ctx.cache, &ctx.cfg, &agent_id, covered, hint).await?
                    }
                    None => return Err(no_key),
                }
            }
            resolved => resolved.map_err(|e| payer::allowance_error(e, retry_after))?,
        };
    // Model routing: the resolver fixed provider/key/params; routing may override only model.
    let decision = routing::route_model_with(
        ctx.classifier.as_ref(),
        ctx.router_cache.as_ref(),
        ctx.tier_registry.as_ref(),
        ctx.cell_store.as_ref(),
        ctx.salience_gate.as_ref(),
        &RouteInputs {
            agent_id: &agent_id,
            provider: &resolved.provider,
            fallback_model: &resolved.model,
            has_llm_config: resolved.has_llm_config,
            pinned_model: resolved.pinned_model.as_deref(),
            tier1_model: resolved.tier1_model.as_deref(),
            tier2_model: resolved.tier2_model.as_deref(),
            tier3_model: resolved.tier3_model.as_deref(),
            signals: &boundary,
            query: query.as_deref(),
            tier_seed: ctx.cfg.router_tier_seed,
        },
    )
    .await;
    let model_overridden = decision.model != resolved.model;
    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id,
        source = ?decision.source, tier = ?decision.tier,
        provider = %resolved.provider,
        resolved_model = %resolved.model,
        decision_model = %decision.model,
        model_overridden,
        "chat_core: model routing decision"
    );
    // Built before the override below consumes `decision.model`, so the block records the decision
    // as it was made rather than what survived it.
    let routing_metadata = Some(decision.to_metadata(model_overridden));
    let mut sticky_conv_id = None;
    if model_overridden {
        tracing::info!(
            target: "nasiko::llm_router::chat",
            %agent_id,
            from = %resolved.model, to = %decision.model,
            "chat_core: router overrode the resolved model"
        );
        keep_configured_model_as_fallback(&mut resolved);
        if matches!(
            decision.source,
            routing::RouteSource::CacheHit | routing::RouteSource::Classified
        ) {
            sticky_conv_id = boundary.conv_id;
        }
        resolved.litellm_model = format!("{}/{}", resolved.provider, decision.model);
        resolved.model = decision.model;
    }
    if resolved.pinned_model.is_some() {
        // Compliance lock (Level 1): a pinned agent never re-routes. Disable fallbacks so an
        // unavailable pinned model surfaces its error instead of silently switching models.
        tracing::info!(
            target: "nasiko::llm_router::chat",
            %agent_id, pinned_model = ?resolved.pinned_model,
            "chat_core: agent is pinned — fallbacks disabled (compliance lock)"
        );
        resolved.fallback_models.clear();
    }
    Ok(RoutedRequest {
        agent_id,
        owner_id: billed_user_id,
        resolved,
        flow_id,
        attribution_source,
        compression: crate::compress::Ledger::default(),
        routing_metadata,
        sticky_conv_id,
    })
}

fn keep_configured_model_as_fallback(resolved: &mut crate::resolver::ResolvedConfig) {
    let configured = format!("{}/{}", resolved.provider, resolved.model);
    let listed = resolved
        .fallback_models
        .iter()
        .any(|entry| *entry == configured || *entry == resolved.model);
    if !listed {
        resolved.fallback_models.insert(0, configured);
    }
}

pub(crate) async fn forget_route_that_did_not_answer(
    ctx: &LlmRouterCtx,
    agent_id: &str,
    sticky_conv_id: Option<&str>,
    routed: &crate::resolver::ResolvedConfig,
    answered: (&str, &str),
) {
    let Some(conv_id) = sticky_conv_id else {
        return;
    };
    if answered == (routed.provider.as_str(), routed.model.as_str()) {
        return;
    }
    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id, routed_model = %routed.model, answered_model = %answered.1,
        "chat_core: routed model did not answer; dropping its sticky decision"
    );
    ctx.router_cache.forget(conv_id, agent_id).await;
}

/// Whether the agent runs outside any flow: a coding-agent CLI integration, or an A2A agent the
/// CLI discovered on a user's machine. Read before its config is resolved (the config depends on
/// who the call is attributed to, and neither kind is ever attributed). A missing agent row is
/// `NoRegistryEntry`, as the resolver reports it.
async fn runs_outside_flows(
    store: &dyn RegistryStore,
    agent_id: &str,
) -> Result<bool, GatewayError> {
    let no_entry = || GatewayError::NoRegistryEntry(agent_id.to_string());
    let agent_uuid = uuid::Uuid::parse_str(agent_id).map_err(|_| no_entry())?;
    let flags = store
        .fetch_agent_flags(agent_uuid)
        .await
        .map_err(|e| GatewayError::Internal(format!("registry read failed: {e}")))?
        .ok_or_else(no_entry)?;
    Ok(flags.is_coding_agent || flags.is_discovered_agent)
}

/// The text the salience gate and classifier score for an in-flow call: the end user's
/// message as the platform received it, when the flow recorded one, else the latest user
/// turn parsed from the LLM request.
///
/// The recorded message is preferred because the request body is whatever the agent's
/// framework built — a system prompt glued onto the user turn, retrieved context, packed
/// history — and scoring that judges the framing, not what the user asked.
fn routing_query(flow_message: Option<String>, transcript_query: Option<String>) -> Option<String> {
    let flow_message = flow_message.filter(|m| !m.trim().is_empty());
    tracing::info!(
        target: "nasiko::llm_router::chat",
        source = if flow_message.is_some() { "flow_user_message" } else { "llm_request" },
        "chat_core: routing query source"
    );
    flow_message.or(transcript_query)
}

/// Derive the model-routing [`BoundarySignals`] from the attributed flow.
///
/// The flow lookup already happened in [`attribution::resolve`] — the signals
/// here come from that same trusted flow state. Attribution is strict, so an
/// unattributable call was rejected before this point; every served call is
/// in-flow.
fn boundary_signals_for(a: &routing::attribution::FlowAttribution) -> BoundarySignals {
    // Key the decision cache on the conversation's stable context_id, not
    // the flow_id (= this turn's trace id, which the CLI re-mints every
    // turn). Turn 1 writes the sticky decision under it and turn 2+ hit
    // it. Fall back to flow_id for flows that never set context_id.
    let conv_id = a
        .context_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| a.flow_id.clone());
    let signals = BoundarySignals::in_flow(conv_id.clone(), a.mode);
    tracing::info!(
        target: "nasiko::llm_router::boundary",
        flow_id = %a.flow_id, %conv_id, mode = ?a.mode,
        source = a.source.as_label(), phase = ?signals.phase,
        is_fireable_boundary = signals.is_fireable_boundary(),
        "boundary signals: known flow → IN-FLOW (router may re-select the model at this boundary)"
    );
    signals
}

/// Everything [`stream_chat`] needs; bundled so the argument list stays readable.
struct StreamChatArgs<'a> {
    ctx: &'a LlmRouterCtx,
    renderer: Box<dyn ChatStreamRenderer>,
    provider_stream: BoxStream<'static, Result<ChatChunk, ProviderError>>,
    provider: String,
    model: String,
    agent_id: String,
    owner_id: String,
    started: Instant,
    flow_id: Option<String>,
    attribution_source: Option<routing::attribution::AttributionSource>,
    platform_paid: bool,
    compress_metadata: Option<serde_json::Value>,
    /// The call's `gen_ai` span, kept alive for the stream's lifetime so the
    /// usage that only arrives in a terminal chunk can still be recorded on it.
    /// Without this a streamed call produced a span with no token attributes at
    /// all, so every trace-derived figure counted it as free.
    span: tracing::Span,
    brevity_metadata: Option<serde_json::Value>,
    compact_tools_metadata: Option<serde_json::Value>,
    /// How the model was chosen (`RoutedRequest::routing_metadata`), carried to the row the stream
    /// writes when it ends.
    routing_metadata: Option<serde_json::Value>,
    /// Savings-ledger inputs, threaded through to the `Drop` write for the same reason
    /// `compress_metadata` is: on this path the `UsageRecord` is only built once the stream ends.
    compress_bytes: Option<(usize, usize)>,
    request_bytes: Option<usize>,
    /// Set for a discovered agent: its reply is gathered as it streams and recorded as a turn
    /// when the stream ends.
    turn: Option<TurnStart>,
}

/// Stream provider chunks back as OpenAI SSE: `data: <chunk>\n\n` … `data: [DONE]\n\n`.
/// Usage is captured as chunks flow and written when the stream ends — including on
/// client disconnect — via a `Drop` guard. `provider`/`model` are the effective
/// (possibly fallback) values chosen by the executor.
fn stream_chat(args: StreamChatArgs<'_>) -> Result<Response, GatewayError> {
    let StreamChatArgs {
        ctx,
        renderer,
        provider_stream,
        provider,
        model,
        agent_id,
        owner_id,
        started,
        flow_id,
        attribution_source,
        platform_paid,
        compress_metadata,
        span,
        brevity_metadata,
        compact_tools_metadata,
        routing_metadata,
        compress_bytes,
        request_bytes,
        turn,
    } = args;
    let state = Arc::new(Mutex::new(StreamState {
        reply: turn.is_some().then(StreamedReply::default),
        ..StreamState::default()
    }));
    let guard = UsageGuard {
        db: ctx.db.clone(),
        pricing: ctx.pricing.clone(),
        ledger: ctx.ledger.clone(),
        span,
        owner_id,
        agent_id,
        provider,
        model: model.clone(),
        started,
        state: state.clone(),
        flow_id,
        attribution_source,
        platform_paid,
        compress_metadata,
        brevity_metadata,
        compact_tools_metadata,
        routing_metadata,
        compress_bytes,
        request_bytes,
        turn: turn.map(|turn| (ctx.turn_recorder.clone(), turn)),
    };

    let body_stream = async_stream::stream! {
        // Moved in so it drops (→ writes usage) when the stream ends or the client
        // disconnects. `state` is read by the guard at drop time. `renderer` moves in
        // too — it owns the agent-facing SSE framing (flat `data:` for OpenAI, stateful
        // `event:` sequences for Anthropic) and its terminal events.
        let _guard = guard;
        let mut renderer = renderer;
        futures::pin_mut!(provider_stream);
        while let Some(item) = provider_stream.next().await {
            match item {
                Ok(mut chunk) => {
                    chunk.model = model.clone();
                    {
                        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                        if chunk.usage.is_some() {
                            st.usage = chunk.usage.clone();
                        }
                        if let Some(fr) = chunk.choices.first().and_then(|c| c.finish_reason.clone()) {
                            st.finish_reason = Some(fr);
                        }
                        // A streamed tool call arrives in fragments that share its index.
                        if let Some(calls) =
                            chunk.choices.first().and_then(|c| c.delta.tool_calls.as_ref())
                        {
                            st.tool_call_indexes.extend(calls.iter().map(|c| c.index));
                        }
                        if let (Some(reply), Some(choice)) = (st.reply.as_mut(), chunk.choices.first()) {
                            reply.push(choice.delta.content.as_deref(), choice.delta.tool_calls.as_deref());
                        }
                    }
                    for frame in renderer.render(chunk) {
                        yield Ok::<String, std::io::Error>(frame);
                    }
                }
                Err(e) => {
                    // Mid-stream provider failure: log and end the stream cleanly. The call
                    // is billed as failed, with whatever usage the provider reported first.
                    tracing::error!(error = %e, "provider stream error");
                    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).failure =
                        Some(crate::providers::attempts::mid_stream_failure(&e));
                    break;
                }
            }
        }
        for frame in renderer.finish() {
            yield Ok(frame);
        }
    };

    Response::builder()
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(body_stream))
        .map_err(|e| GatewayError::Internal(format!("failed to build sse response: {e}")))
}

#[derive(Default)]
struct StreamState {
    usage: Option<Usage>,
    finish_reason: Option<String>,
    /// Distinct tool calls the model asked for in this reply.
    tool_call_indexes: std::collections::BTreeSet<i64>,
    /// Set when the provider's stream broke off with an error.
    failure: Option<usage::CallFailure>,
    /// The reply so far, when the call is recorded as a turn.
    reply: Option<StreamedReply>,
}

/// Writes the streaming usage row when dropped (stream completion or client disconnect).
struct UsageGuard {
    db: sqlx::PgPool,
    pricing: Arc<nasiko_pricing::PricingEngine>,
    ledger: Arc<dyn nasiko_pricing::UsageLedger>,
    span: tracing::Span,
    owner_id: String,
    agent_id: String,
    provider: String,
    model: String,
    started: Instant,
    state: Arc<Mutex<StreamState>>,
    flow_id: Option<String>,
    attribution_source: Option<routing::attribution::AttributionSource>,
    platform_paid: bool,
    /// Taken in `drop`, which runs exactly once.
    compress_metadata: Option<serde_json::Value>,
    brevity_metadata: Option<serde_json::Value>,
    compact_tools_metadata: Option<serde_json::Value>,
    routing_metadata: Option<serde_json::Value>,
    /// `Copy`, so unlike the four above these are read rather than taken.
    compress_bytes: Option<(usize, usize)>,
    request_bytes: Option<usize>,
    /// Taken in `drop`: the recorder and start of a discovered agent's turn.
    turn: Option<(Arc<dyn TurnRecorder>, TurnStart)>,
}

impl UsageGuard {
    /// The row for the stream as it ended. Takes the metadata blocks, so it runs once.
    fn usage_record(&mut self) -> UsageRecord {
        let compress_metadata = self.compress_metadata.take();
        let brevity_metadata = self.brevity_metadata.take();
        let compact_tools_metadata = self.compact_tools_metadata.take();
        let routing_metadata = self.routing_metadata.take();
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        record_span_usage(&self.span, st.usage.as_ref());
        record_tool_calls(&self.span, st.tool_call_indexes.len());
        if let (Some((recorder, turn)), Some(reply)) = (self.turn.take(), st.reply.as_ref()) {
            recorder.record(turn.finish(
                (self.agent_id.clone(), self.owner_id.clone()),
                reply.text(),
                self.model.clone(),
                st.usage.as_ref(),
                self.started.elapsed().as_millis() as i64,
            ));
        }
        UsageRecord {
            owner_id: self.owner_id.clone(),
            agent_id: self.agent_id.clone(),
            operation_type: "direct_llm",
            provider: self.provider.clone(),
            model: self.model.clone(),
            usage: st.usage.clone(),
            cached_tokens: None,
            reasoning_tokens: None,
            latency_ms: self.started.elapsed().as_millis() as i64,
            streaming: true,
            finish_reason: st.finish_reason.clone(),
            flow_id: self.flow_id.clone(),
            attribution_source: self.attribution_source,
            platform_paid: self.platform_paid,
            compress_metadata,
            brevity_metadata,
            compact_tools_metadata,
            routing_metadata,
            compress_bytes: self.compress_bytes,
            // Compact tools skips streaming requests.
            compact_tools_bytes: None,
            request_bytes: self.request_bytes,
            failure: st.failure,
        }
    }
}

impl Drop for UsageGuard {
    fn drop(&mut self) {
        let record = self.usage_record();
        usage::spawn_log(
            self.db.clone(),
            self.pricing.clone(),
            self.ledger.clone(),
            record,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;
    use crate::payer::Payer;
    use crate::resolver::{AgentConfigResult, ConfigCache, LLMConfig};
    use async_trait::async_trait;
    use axum::http::StatusCode;
    use axum::http::header::AUTHORIZATION;
    use jsonwebtoken::Algorithm;
    use nasiko_secrets::SecretsCrypto;
    use serde_json::json;
    use sqlx::PgPool;
    use std::time::Duration;
    use uuid::Uuid;

    #[derive(Clone, Default)]
    struct CapturedUsage(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, i64>>>);

    impl tracing::field::Visit for CapturedUsage {
        fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
            self.0.lock().unwrap().insert(field.name().into(), value);
        }

        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedUsage {
        fn on_record(
            &self,
            _: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            values.record(&mut self.clone());
        }
    }

    #[test]
    fn gateway_spans_keep_the_normalized_cache_split_and_provider_total() {
        use tracing_subscriber::prelude::*;
        let cases = [
            (
                json!({"prompt_tokens": 4732, "completion_tokens": 110, "total_tokens": 4842,
                "prompt_tokens_details": {"cached_tokens": 3968}}),
                [764, 110, 3968, 0],
            ),
            (
                json!({"prompt_tokens": 1000, "completion_tokens": 50,
                "cache_read_input_tokens": 200, "cache_creation_input_tokens": 300}),
                [1000, 50, 200, 300],
            ),
        ];
        for (raw, expected) in cases {
            let usage: Usage = serde_json::from_value(raw).unwrap();
            let captured = CapturedUsage::default();
            let subscriber = tracing_subscriber::registry().with(captured.clone());
            tracing::subscriber::with_default(subscriber, || {
                let span = tracing::info_span!(
                    "test_usage",
                    gen_ai.usage.input_tokens = tracing::field::Empty,
                    gen_ai.usage.output_tokens = tracing::field::Empty,
                    gen_ai.usage.cache_read_input_tokens = tracing::field::Empty,
                    gen_ai.usage.cache_creation_input_tokens = tracing::field::Empty,
                    gen_ai.usage.total_tokens = tracing::field::Empty,
                );
                record_span_usage(&span, Some(&usage));
            });
            let fields = captured.0.lock().unwrap();
            for (key, value) in [
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(
                    fields
                        .get(&format!("gen_ai.usage.{key}"))
                        .copied()
                        .unwrap_or(0),
                    value
                );
            }
            assert_eq!(
                fields.get("gen_ai.usage.total_tokens").copied(),
                usage.total_tokens
            );
            // Recording must not mutate the response sent back to the client.
            if usage.prompt_tokens_details.is_some() {
                assert_eq!(usage.prompt_tokens, Some(4732));
            }
        }
    }

    #[tokio::test]
    async fn dropping_a_stream_records_its_last_reported_usage() {
        use tracing_subscriber::prelude::*;
        let captured = CapturedUsage::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        tracing::subscriber::with_default(subscriber, || {
            let ctx = ctx_with("http://unused.invalid".into());
            let span = tracing::info_span!(
                "stream_usage",
                gen_ai.usage.input_tokens = tracing::field::Empty,
                gen_ai.usage.output_tokens = tracing::field::Empty,
                gen_ai.usage.cache_read_input_tokens = tracing::field::Empty,
                gen_ai.usage.cache_creation_input_tokens = tracing::field::Empty,
                gen_ai.usage.total_tokens = tracing::field::Empty,
            );
            let guard = UsageGuard {
                db: ctx.db,
                pricing: Arc::new(nasiko_pricing::PricingEngine::offline()),
                ledger: Arc::new(nasiko_pricing::NoUsageLedger),
                span,
                // An invalid owner skips the asynchronous database write in this
                // span-lifetime test; persistence is covered by integration tests.
                owner_id: "no-database-write".into(),
                agent_id: AGENT.into(),
                provider: "openai".into(),
                model: "gpt-4o".into(),
                started: Instant::now(),
                state: Arc::new(Mutex::new(StreamState {
                    usage: Some(
                        serde_json::from_value(json!({
                            "prompt_tokens": 1000, "completion_tokens": 50,
                            "total_tokens": 1050,
                            "prompt_tokens_details": {"cached_tokens": 200}
                        }))
                        .unwrap(),
                    ),
                    finish_reason: None,
                    tool_call_indexes: Default::default(),
                    failure: None,
                    reply: None,
                })),
                flow_id: None,
                turn: None,
                // This test is about the span's lifetime, not the savings ledger: no compression
                // ran, so there is nothing for the guard to credit.
                compress_bytes: None,
                request_bytes: None,
                attribution_source: None,
                platform_paid: true,
                // This test covers span lifetime, not compression.
                compress_metadata: None,
                brevity_metadata: None,
                compact_tools_metadata: None,
                routing_metadata: None,
            };
            drop(guard);
        });
        let fields = captured.0.lock().unwrap();
        assert_eq!(fields["gen_ai.usage.input_tokens"], 800);
        assert_eq!(fields["gen_ai.usage.output_tokens"], 50);
        assert_eq!(fields["gen_ai.usage.cache_read_input_tokens"], 200);
    }

    /// BUG-12: a streamed reply asking for two tools records 2 on the call's span.
    #[tokio::test]
    async fn dropping_a_stream_records_the_tool_calls_it_asked_for() {
        use tracing_subscriber::prelude::*;
        let captured = CapturedUsage::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        tracing::subscriber::with_default(subscriber, || {
            let ctx = ctx_with("http://unused.invalid".into());
            let span = tracing::info_span!(
                "stream_tool_calls",
                nasiko.response.tool_calls = tracing::field::Empty,
            );
            let guard = UsageGuard {
                db: ctx.db,
                pricing: Arc::new(nasiko_pricing::PricingEngine::offline()),
                ledger: Arc::new(nasiko_pricing::NoUsageLedger),
                span,
                owner_id: "no-database-write".into(),
                agent_id: AGENT.into(),
                provider: "openai".into(),
                model: "gpt-4o".into(),
                started: Instant::now(),
                state: Arc::new(Mutex::new(StreamState {
                    usage: None,
                    finish_reason: Some("tool_calls".into()),
                    tool_call_indexes: [0, 1].into_iter().collect(),
                    failure: None,
                    reply: None,
                })),
                flow_id: None,
                compress_bytes: None,
                request_bytes: None,
                attribution_source: None,
                platform_paid: true,
                compress_metadata: None,
                brevity_metadata: None,
                compact_tools_metadata: None,
                routing_metadata: None,
                turn: None,
            };
            drop(guard);
        });
        assert_eq!(captured.0.lock().unwrap()["nasiko.response.tool_calls"], 2);
    }

    /// NAS-1018: a stream that broke off mid-reply is billed as a failed call, with the usage
    /// the provider reported before it broke (so nothing is estimated).
    #[tokio::test]
    async fn a_stream_that_broke_off_is_billed_as_failed_with_its_reported_usage() {
        let ctx = ctx_with("http://unused.invalid".into());
        let mut guard = UsageGuard {
            db: ctx.db,
            pricing: Arc::new(nasiko_pricing::PricingEngine::offline()),
            ledger: Arc::new(nasiko_pricing::NoUsageLedger),
            span: tracing::Span::none(),
            owner_id: "no-database-write".into(),
            agent_id: AGENT.into(),
            provider: "openai".into(),
            model: "gpt-4o".into(),
            started: Instant::now(),
            state: Arc::new(Mutex::new(StreamState {
                usage: Some(
                    serde_json::from_value(
                        json!({ "prompt_tokens": 900, "completion_tokens": 12 }),
                    )
                    .unwrap(),
                ),
                failure: Some(crate::providers::attempts::mid_stream_failure(
                    &ProviderError::Transport("connection reset".into()),
                )),
                ..Default::default()
            })),
            flow_id: None,
            compress_bytes: None,
            request_bytes: Some(4000),
            attribution_source: None,
            platform_paid: true,
            compress_metadata: None,
            brevity_metadata: None,
            compact_tools_metadata: None,
            routing_metadata: None,
            turn: None,
        };
        let record = guard.usage_record();
        assert_eq!(
            record.failure,
            Some(usage::CallFailure::new("stream", None))
        );
        assert_eq!(record.usage.and_then(|u| u.prompt_tokens), Some(900));
        assert!(record.streaming);
    }

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SECRET: &str = "gateway-secret";

    struct Store {
        config: Option<LLMConfig>,
        is_coding_agent: bool,
        is_discovered_agent: bool,
        compress_enabled: bool,
    }
    #[async_trait]
    impl RegistryStore for Store {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(Some(AgentConfigResult {
                config: self.config.clone(),
                agent_pinned_model: None,
                is_coding_agent: self.is_coding_agent,
                is_discovered_agent: self.is_discovered_agent,
                compress_enabled: self.compress_enabled,
            }))
        }
        async fn fetch_agent_owner(&self, _: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            Ok(Uuid::parse_str(OWNER).ok())
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<routing::attribution::LiveFlow>, sqlx::Error> {
            // Every strict-attribution branch is unit-tested in
            // routing/attribution.rs; here the flow always resolves so the
            // format-translation paths under test are reachable.
            Ok(Some(routing::attribution::LiveFlow {
                user_id: None,
                context_id: Some("ses_test".into()),
                mode: None,
                user_message: None,
                agent_is_participant: true,
            }))
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
            _: Option<Uuid>,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<crate::resolver::OrgEmbeddingDefault>, sqlx::Error> {
            Ok(None)
        }
    }

    /// [`Store`], plus an LLM token revocation time.
    struct RevokedStore {
        inner: Store,
        revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    }
    #[async_trait]
    impl RegistryStore for RevokedStore {
        async fn fetch_llm_config(
            &self,
            id: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            self.inner.fetch_llm_config(id).await
        }
        async fn fetch_agent_owner(&self, id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            self.inner.fetch_agent_owner(id).await
        }
        async fn fetch_agent_identity(
            &self,
            id: Uuid,
        ) -> Result<Option<crate::resolver::AgentIdentity>, sqlx::Error> {
            Ok(self.inner.fetch_agent_owner(id).await?.map(|owner_id| {
                crate::resolver::AgentIdentity {
                    owner_id,
                    llm_tokens_revoked_at: self.revoked_at,
                }
            }))
        }
        async fn fetch_user_secret(
            &self,
            owner: Uuid,
            name: &str,
        ) -> Result<Option<String>, sqlx::Error> {
            self.inner.fetch_user_secret(owner, name).await
        }
        async fn fetch_live_flow(
            &self,
            trace_id: &str,
            agent: Uuid,
            window: i64,
        ) -> Result<Option<routing::attribution::LiveFlow>, sqlx::Error> {
            self.inner.fetch_live_flow(trace_id, agent, window).await
        }
        async fn fetch_custom_provider(
            &self,
            name: &str,
            user: Option<Uuid>,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            self.inner.fetch_custom_provider(name, user).await
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<crate::resolver::OrgEmbeddingDefault>, sqlx::Error> {
            self.inner.fetch_org_embedding_default().await
        }
    }

    fn revoked_store(revoked_at: Option<chrono::DateTime<chrono::Utc>>) -> RevokedStore {
        RevokedStore {
            inner: Store {
                config: None,
                is_coding_agent: true,
                is_discovered_agent: false,
                compress_enabled: false,
            },
            revoked_at,
        }
    }

    fn fresh_agent_token() -> String {
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 600, Algorithm::HS256).unwrap();
        format!("Bearer {token}")
    }

    #[tokio::test]
    async fn a_token_issued_before_the_revocation_is_refused() {
        let ctx = ctx_with("http://unused".into());
        let store = revoked_store(Some(chrono::Utc::now() + chrono::Duration::seconds(60)));
        let err = verify_agent_identity(&ctx, &store, Some(&fresh_agent_token()))
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::TokenRevoked), "{err:?}");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        assert!(err.to_string().contains("disconnected"), "{err}");
    }

    #[tokio::test]
    async fn a_token_issued_after_the_revocation_is_accepted() {
        let ctx = ctx_with("http://unused".into());
        let store = revoked_store(Some(chrono::Utc::now() - chrono::Duration::seconds(60)));
        let (agent_id, owner_id) = verify_agent_identity(&ctx, &store, Some(&fresh_agent_token()))
            .await
            .unwrap();
        assert_eq!(agent_id, AGENT);
        assert_eq!(owner_id, OWNER);
    }

    #[tokio::test]
    async fn an_agent_never_revoked_is_accepted() {
        let ctx = ctx_with("http://unused".into());
        let store = revoked_store(None);
        assert!(
            verify_agent_identity(&ctx, &store, Some(&fresh_agent_token()))
                .await
                .is_ok()
        );
    }

    #[test]
    fn revocation_compares_at_whole_second_precision() {
        let revoked = chrono::DateTime::from_timestamp(1_000, 700_000_000).unwrap();
        assert!(is_revoked(Some(999), Some(revoked)));
        assert!(
            !is_revoked(Some(1_000), Some(revoked)),
            "same second is a reconnect"
        );
        assert!(!is_revoked(Some(1_001), Some(revoked)));
        assert!(!is_revoked(Some(999), None));
        assert!(
            is_revoked(None, Some(revoked)),
            "no iat cannot prove it is newer"
        );
        assert!(!is_revoked(None, None));
    }

    /// No-op tier registry: attribution now always resolves in these tests
    /// (strict enforcement), which makes every call a fireable boundary — a
    /// registry with tier mappings would then override the request model and
    /// break the passthrough behaviour under test. No mapping ⇒ the resolved
    /// model always passes through.
    struct NoTiers;
    #[async_trait]
    impl routing::registry::TierRegistry for NoTiers {
        async fn model_for(&self, _: &str, _: routing::classifier::Tier) -> Option<String> {
            None
        }
    }

    /// ctx whose DB never connects — fire-and-forget usage writes fail silently,
    /// also exercising "usage-logging failure does not break the request".
    fn ctx_with(base: String) -> LlmRouterCtx {
        let cfg = GatewayConfig {
            agent_jwt_secret: SECRET.into(),
            openai_api_base: base,
            platform_openai_api_key: "sk-platform".into(),
            default_provider: "openai".into(),
            default_model: "gpt-4o-mini".into(),
            ..Default::default()
        };
        LlmRouterCtx {
            db: PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            http: reqwest::Client::new(),
            cfg: Arc::new(cfg),
            cache: Arc::new(ConfigCache::new(Duration::from_secs(30))),
            router_cache: Arc::new(crate::routing::NoopCache),
            tier_registry: Arc::new(NoTiers),
            cell_store: Arc::new(crate::routing::InMemoryCellStore::new()),
            salience_gate: Arc::new(crate::routing::AllowAllGate),
            classifier: Arc::new(crate::routing::RegexClassifier),
            pricing: Arc::new(nasiko_pricing::PricingEngine::new(
                PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            )),
            body_permits: crate::handlers::auth_layer::body_permit_pool(256 * 1024 * 1024),
            attachment_store: Arc::new(crate::attachments::NoAttachmentStore),
            attachment_cache: Arc::new(crate::attachments::AttachmentCache::new(
                1024 * 1024,
                crate::attachments::ATTACHMENT_CACHE_TTL,
            )),
            marker_signer: None,
            ledger: Arc::new(nasiko_pricing::NoUsageLedger),
            payer: Arc::new(crate::payer::CallerPays),
            turn_recorder: Arc::new(crate::turns::NoTurnRecorder),
        }
    }

    const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    fn auth_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        // Strict attribution: every served call must carry trace context.
        h.insert(TRACEPARENT_HEADER, TRACEPARENT.parse().unwrap());
        h
    }

    fn token() -> String {
        crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap()
    }

    /// An llm_config pinning the destination to OpenAI `gpt-4o-mini` — used by the format-
    /// translation tests so a non-OpenAI inbound surface still routes to the mocked OpenAI
    /// endpoint (isolating inbound/outbound translation from the no-config passthrough path).
    fn openai_config() -> LLMConfig {
        LLMConfig {
            provider: "openai".into(),
            model: Some("gpt-4o-mini".into()),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            api_key_secret_name: None,
            pinned: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            embedding_provider: None,
            embedding_model: None,
            embedding_api_key_secret_name: None,
            embedding_model_override: false,
        }
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // ── compression: proves the per-agent toggle reaches the wire ─────────────────────────
    //
    // `compress::tests` covers the transform. These cover the wiring the toggle depends on:
    // that `policy_for` reads the agent's own flag, and that what the provider receives is what
    // compression produced — neither of which the unit tests can see.

    /// A tool result big and repetitive enough to be worth compressing.
    fn noisy_tool_result() -> String {
        (0..300)
            .map(|i| format!("2026-01-01T00:00:00Z INFO handled request {i}\n"))
            .collect()
    }

    fn tool_transcript(result: &str) -> serde_json::Value {
        json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "user", "content": "why did the deploy fail?" },
                { "role": "assistant", "content": null, "tool_calls": [
                    { "id": "call_1", "type": "function",
                      "function": { "name": "read_logs", "arguments": "{}" } }
                ]},
                { "role": "tool", "tool_call_id": "call_1", "content": result },
            ]
        })
    }

    /// Mocks the provider, captures the body it actually received, and runs one request.
    ///
    /// The capture happens in `with_body_from_request` — the only hook mockito gives onto the
    /// real outbound body, which is the whole point: asserting on `req` in-process would prove
    /// nothing about what the provider is sent.
    async fn provider_saw(compress_enabled: bool) -> String {
        let seen = Arc::new(Mutex::new(String::new()));
        let capture = Arc::clone(&seen);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let body = request.body().map(Vec::as_slice).unwrap_or_default();
                *capture.lock().unwrap_or_else(|e| e.into_inner()) =
                    String::from_utf8_lossy(body).into_owned();
                json!({
                    "id": "chatcmpl-x", "object": "chat.completion", "model": "gpt-4o",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
                })
                .to_string()
                .into_bytes()
            })
            .create_async()
            .await;

        let ctx = ctx_with(server.url());
        let store = Store {
            config: None,
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled,
        };
        chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            tool_transcript(&noisy_tool_result()),
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();

        mock.assert_async().await;
        let body = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(!body.is_empty(), "provider was never called");
        body
    }

    #[tokio::test]
    async fn toggle_off_sends_the_tool_result_verbatim() {
        let sent = provider_saw(false).await;
        assert!(
            sent.contains("handled request 150"),
            "an agent with the toggle off must reach the provider unchanged"
        );
        assert!(!sent.contains("lines elided"));
    }

    #[tokio::test]
    async fn toggle_on_compresses_the_tool_result_the_provider_receives() {
        let sent = provider_saw(true).await;
        assert!(
            sent.contains("lines elided"),
            "toggle on, but the provider received no elision marker: {}",
            &sent[..sent.len().min(400)]
        );
        assert!(
            !sent.contains("handled request 150"),
            "middle noise survived"
        );
    }

    #[tokio::test]
    async fn the_toggle_decides_and_one_agent_does_not_affect_another() {
        let off = provider_saw(false).await;
        let on = provider_saw(true).await;
        assert!(
            on.len() < off.len(),
            "compressed payload ({}) is not smaller than uncompressed ({})",
            on.len(),
            off.len()
        );
        println!(
            "wire bytes: off={} on={} ({:.1}%)",
            off.len(),
            on.len(),
            (on.len() as f64 - off.len() as f64) / off.len() as f64 * 100.0
        );
    }

    #[tokio::test]
    async fn compression_never_disturbs_tool_call_threading() {
        let sent = provider_saw(true).await;
        assert!(sent.contains("call_1"), "tool_call_id lost");
        assert!(sent.contains("read_logs"), "tool call name lost");
        assert!(
            sent.contains("why did the deploy fail?"),
            "the user's own question was altered"
        );
    }

    #[tokio::test]
    async fn end_to_end_honors_request_model_when_no_config_and_returns_openai_shape() {
        // No llm_config ⇒ the request's own model ("gpt-4o") is honored (passthrough),
        // NOT the platform default ("gpt-4o-mini"). Provider stays openai (the OpenAI SDK
        // surface). The response `model` is normalized to that resolved model.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "model": "gpt-4o" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-x", "object": "chat.completion", "model": "gpt-4o-2024",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let ctx = ctx_with(server.url());
        let store = Store {
            config: None,
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        let body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] });
        let resp = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            body,
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();

        let v: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["model"], "gpt-4o");
        assert_eq!(v["choices"][0]["message"]["content"], "hello");
        assert_eq!(v["usage"]["total_tokens"], 7);
    }

    #[tokio::test]
    async fn anthropic_inbound_parses_and_renders_anthropic_shape() {
        // An Anthropic-SDK agent POSTs an Anthropic request; its llm_config pins the
        // destination to the OpenAI provider, and we must return an Anthropic-shaped response.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            // System extracted from top-level → an OpenAI system message reaches the provider.
            .match_body(mockito::Matcher::PartialJson(json!({ "model": "gpt-4o-mini" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-y", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "नमस्ते" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let ctx = ctx_with(server.url());
        let store = Store {
            config: Some(openai_config()),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        // Anthropic Messages request shape: top-level system + max_tokens.
        let body = json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "system": "You are helpful.",
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let resp = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            body,
            InboundFormat::Anthropic,
            None,
        )
        .await
        .unwrap();

        let v: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        // Anthropic-shaped response, not OpenAI.
        assert_eq!(v["type"], "message");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "नमस्ते");
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["usage"]["input_tokens"], 9);
        assert_eq!(v["usage"]["output_tokens"], 3);
    }

    #[tokio::test]
    async fn gemini_inbound_parses_and_renders_gemini_shape() {
        // A Gemini-SDK agent POSTs a Gemini request; its llm_config pins the destination to
        // the OpenAI provider, and we must return a Gemini `GenerateContentResponse` shape.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "model": "gpt-4o-mini" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-z", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let ctx = ctx_with(server.url());
        let store = Store {
            config: Some(openai_config()),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        // Gemini Messages request shape: systemInstruction + contents.
        let body = json!({
            "systemInstruction": { "parts": [{ "text": "sys" }] },
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }]
        });
        let resp = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            body,
            InboundFormat::Gemini,
            Some(false),
        )
        .await
        .unwrap();

        let v: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["candidates"][0]["content"]["role"], "model");
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "ok");
        assert_eq!(v["candidates"][0]["finishReason"], "STOP");
        assert_eq!(v["usageMetadata"]["totalTokenCount"], 4);
    }

    #[tokio::test]
    async fn streaming_returns_sse_with_done_terminator() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        );
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "stream": true })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let ctx = ctx_with(server.url());
        let store = Store {
            config: None,
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        let body = json!({ "model": "gpt-4o", "stream": true, "messages": [{ "role": "user", "content": "hi" }] });
        let resp = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            body,
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            resp.headers().get(CONTENT_TYPE).unwrap(),
            "text/event-stream"
        );

        let body = body_string(resp).await;
        assert!(body.contains("\"content\":\"hi\""));
        // No config ⇒ the request model is honored; chunks are normalized to that resolved id.
        assert!(body.contains("\"model\":\"gpt-4o\""));
        assert!(body.trim_end().ends_with("data: [DONE]"));
    }

    /// Keeps every turn it is given, for tests to read back.
    #[derive(Clone, Default)]
    struct CapturedTurns(Arc<std::sync::Mutex<Vec<crate::turns::RecordedTurn>>>);

    impl crate::turns::TurnRecorder for CapturedTurns {
        fn record(&self, turn: crate::turns::RecordedTurn) {
            self.0.lock().unwrap().push(turn);
        }
    }

    fn store_for(is_discovered_agent: bool) -> Store {
        Store {
            config: None,
            is_coding_agent: false,
            is_discovered_agent,
            compress_enabled: false,
        }
    }

    /// A conversation whose latest user message is the one a turn is recorded under.
    fn conversation(stream: bool) -> Value {
        json!({
            "model": "gpt-4o", "stream": stream,
            "messages": [
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "earlier question" },
                { "role": "assistant", "content": "earlier answer" },
                { "role": "user", "content": "plan my day" }
            ]
        })
    }

    #[tokio::test]
    async fn a_discovered_agents_call_is_recorded_as_a_turn() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "r", "object": "chat.completion", "model": "gpt-4o",
                    "choices": [{ "index": 0, "finish_reason": "stop",
                                  "message": { "role": "assistant", "content": "start with email" } }],
                    "usage": { "prompt_tokens": 30, "completion_tokens": 4, "total_tokens": 34 }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let turns = CapturedTurns::default();
        let mut ctx = ctx_with(server.url());
        ctx.turn_recorder = Arc::new(turns.clone());

        chat_core(
            &ctx,
            &store_for(true),
            &auth_headers(&token()),
            conversation(false),
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();

        let recorded = turns.0.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].agent_id, AGENT);
        assert_eq!(recorded[0].user_text, "plan my day");
        assert_eq!(recorded[0].assistant_text, "start with email");
        assert_eq!(recorded[0].model, "gpt-4o");
        assert_eq!(
            (recorded[0].input_tokens, recorded[0].output_tokens),
            (Some(30), Some(4))
        );
    }

    #[tokio::test]
    async fn a_discovered_agents_streamed_reply_is_recorded_once_the_stream_ends() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"start \"}}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"with email\"}}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":30,\"completion_tokens\":4,\"total_tokens\":34}}\n\n",
            "data: [DONE]\n\n",
        );
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;
        let turns = CapturedTurns::default();
        let mut ctx = ctx_with(server.url());
        ctx.turn_recorder = Arc::new(turns.clone());

        let resp = chat_core(
            &ctx,
            &store_for(true),
            &auth_headers(&token()),
            conversation(true),
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();
        // Nothing is recorded while the reply streams; the agent's response is not held for it.
        assert!(turns.0.lock().unwrap().is_empty());
        let body = body_string(resp).await;
        assert!(body.contains("with email"));

        let recorded = turns.0.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].user_text, "plan my day");
        assert_eq!(recorded[0].assistant_text, "start with email");
        assert_eq!(recorded[0].output_tokens, Some(4));
    }

    #[tokio::test]
    async fn a_call_answering_tool_results_is_recorded_as_such() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "r", "object": "chat.completion", "model": "gpt-4o",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "done" } }]
                })
                .to_string(),
            )
            .create_async()
            .await;
        let turns = CapturedTurns::default();
        let mut ctx = ctx_with(server.url());
        ctx.turn_recorder = Arc::new(turns.clone());
        let body = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "user", "content": "plan my day" },
                { "role": "assistant", "content": null, "tool_calls": [
                    { "id": "c1", "type": "function", "function": { "name": "calendar", "arguments": "{}" } }
                ] },
                { "role": "tool", "tool_call_id": "c1", "content": "3 meetings" }
            ]
        });

        chat_core(
            &ctx,
            &store_for(true),
            &auth_headers(&token()),
            body,
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();

        let recorded = turns.0.lock().unwrap().clone();
        assert_eq!(recorded[0].user_text, crate::turns::TOOL_RESULTS);
        assert_eq!(recorded[0].assistant_text, "done");
    }

    #[tokio::test]
    async fn only_discovered_agents_calls_are_recorded() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "r", "object": "chat.completion", "model": "gpt-4o",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" } }]
                })
                .to_string(),
            )
            .create_async()
            .await;
        let turns = CapturedTurns::default();
        let mut ctx = ctx_with(server.url());
        ctx.turn_recorder = Arc::new(turns.clone());

        chat_core(
            &ctx,
            &store_for(false),
            &auth_headers(&token()),
            conversation(false),
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();

        assert!(turns.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_auth_is_401_before_any_provider_call() {
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: None,
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        let body = json!({ "model": "gpt-4o", "messages": [] });
        let err = chat_core(
            &ctx,
            &store,
            &HeaderMap::new(),
            body,
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::MissingAuthHeader));
    }

    fn gemini_sdk_headers(key: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-goog-api-key", key.parse().unwrap());
        h.insert(TRACEPARENT_HEADER, TRACEPARENT.parse().unwrap());
        h
    }

    fn gemini_hi() -> Value {
        json!({ "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }] })
    }

    fn store_pinned_to_openai() -> Store {
        Store {
            config: Some(openai_config()),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        }
    }

    #[tokio::test]
    async fn the_gemini_sdks_x_goog_api_key_header_authenticates_and_is_not_forwarded() {
        let mut server = mockito::Server::new_async().await;
        let upstream = server
            .mock("POST", "/chat/completions")
            .match_header("x-goog-api-key", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-g", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let resp = chat_core(
            &ctx_with(server.url()),
            &store_pinned_to_openai(),
            &gemini_sdk_headers(&token()),
            gemini_hi(),
            InboundFormat::Gemini,
            Some(false),
        )
        .await
        .unwrap();

        upstream.assert_async().await;
        let v: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "ok");
    }

    #[tokio::test]
    async fn an_empty_gemini_turn_reaches_a_provider_that_accepts_one() {
        let mut server = mockito::Server::new_async().await;
        let upstream = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "messages": [{ "role": "user", "content": "" }]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-e", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let resp = chat_core(
            &ctx_with(server.url()),
            &store_pinned_to_openai(),
            &gemini_sdk_headers(&token()),
            json!({ "contents": [{ "role": "user", "parts": [] }] }),
            InboundFormat::Gemini,
            Some(false),
        )
        .await
        .unwrap();

        upstream.assert_async().await;
        let v: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "ok");
    }

    #[tokio::test]
    async fn a_bad_token_in_x_goog_api_key_is_refused_before_any_provider_call() {
        let err = chat_core(
            &ctx_with("http://unused".into()),
            &store_pinned_to_openai(),
            &gemini_sdk_headers("not-a-token"),
            gemini_hi(),
            InboundFormat::Gemini,
            Some(false),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, GatewayError::InvalidToken(_)), "{err:?}");
    }

    #[tokio::test]
    async fn authorization_wins_over_x_goog_api_key() {
        let mut headers = gemini_sdk_headers(&token());
        headers.insert(AUTHORIZATION, "Bearer not-a-token".parse().unwrap());

        let err = chat_core(
            &ctx_with("http://unused".into()),
            &store_pinned_to_openai(),
            &headers,
            gemini_hi(),
            InboundFormat::Gemini,
            Some(false),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, GatewayError::InvalidToken(_)), "{err:?}");
    }

    #[test]
    fn routing_query_prefers_the_flows_user_message() {
        let wrapped = Some("You are a deployed AI agent...\n\nUser request:\nHi".to_string());
        assert_eq!(
            routing_query(Some("Hi".into()), wrapped.clone()).as_deref(),
            Some("Hi")
        );
        // No recorded message (or a blank one) falls back to the request's own text.
        assert_eq!(routing_query(None, wrapped.clone()), wrapped);
        assert_eq!(routing_query(Some("  ".into()), wrapped.clone()), wrapped);
    }

    #[test]
    fn boundary_signals_in_flow_when_attributed() {
        let attribution = routing::attribution::FlowAttribution {
            flow_id: "f1".into(),
            user_id: None,
            context_id: Some("ses_1".into()),
            mode: routing::Mode::FreeFlowing,
            source: routing::attribution::AttributionSource::Traceparent,
            user_message: None,
        };
        // The stable context_id keys the decision cache, not the per-turn flow id.
        let signals = boundary_signals_for(&attribution);
        assert_eq!(signals.conv_id.as_deref(), Some("ses_1"));
        assert!(signals.is_fireable_boundary());
    }

    #[tokio::test]
    async fn missing_traceparent_is_403_before_any_provider_call() {
        // Strict enforcement: a valid agent JWT with no trace context is
        // refused with 403 (not 401 — the credential itself is fine).
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: None,
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", token()).parse().unwrap(),
        );
        let body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] });
        let err = chat_core(&ctx, &store, &headers, body, InboundFormat::OpenAi, None)
            .await
            .unwrap_err();
        match err {
            GatewayError::Forbidden(msg) => {
                assert!(msg.contains("traceparent"), "descriptive body: {msg}")
            }
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    /// A store whose flow was started by a user other than the agent's owner — the agent is
    /// shared with them — and who has no LLM config of their own.
    struct SharedCallerStore(Store);
    #[async_trait]
    impl RegistryStore for SharedCallerStore {
        async fn fetch_llm_config(
            &self,
            id: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            self.0.fetch_llm_config(id).await
        }
        async fn fetch_agent_owner(&self, id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            self.0.fetch_agent_owner(id).await
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            panic!("no one's key may be read for a shared caller without a config")
        }
        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<routing::attribution::LiveFlow>, sqlx::Error> {
            Ok(Some(routing::attribution::LiveFlow {
                user_id: Uuid::parse_str("33333333-3333-3333-3333-333333333333").ok(),
                context_id: Some("ses_shared".into()),
                mode: None,
                user_message: None,
                agent_is_participant: true,
            }))
        }
        async fn fetch_custom_provider(
            &self,
            label: &str,
            user: Option<Uuid>,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            self.0.fetch_custom_provider(label, user).await
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<crate::resolver::OrgEmbeddingDefault>, sqlx::Error> {
            self.0.fetch_org_embedding_default().await
        }
    }

    #[tokio::test]
    async fn a_shared_caller_without_a_config_is_told_to_connect_one_before_any_provider_call() {
        // The owner has a config with a key; the shared caller must never run on it.
        let ctx = ctx_with("http://unused".into());
        let store = SharedCallerStore(Store {
            config: Some(openai_config()),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        });
        let body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] });
        let err = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            body,
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::NoSharedUserConfig), "{err:?}");
    }

    const SHARED: &str = "33333333-3333-3333-3333-333333333333";

    /// An agent `OWNER` owns, attached to `owner_config` (`owner-model` unless a test clears it),
    /// called by `flow_user`. Every secret decrypts to `key-of-<holder>`, so the key a provider
    /// receives names whose key it is.
    struct PayerStore {
        flow_user: Uuid,
        owner_config: Option<LLMConfig>,
        chosen: Option<LLMConfig>,
        default: Option<LLMConfig>,
        is_coding_agent: bool,
        choice_read_fails: bool,
    }

    impl PayerStore {
        fn new(chosen: Option<&str>, default: Option<&str>) -> Self {
            let keyed = |model: &str| LLMConfig {
                model: Some(model.into()),
                api_key_secret_name: Some("KEY".into()),
                ..openai_config()
            };
            Self {
                flow_user: Uuid::parse_str(SHARED).unwrap(),
                owner_config: Some(keyed("owner-model")),
                chosen: chosen.map(keyed),
                default: default.map(keyed),
                is_coding_agent: false,
                choice_read_fails: false,
            }
        }
    }

    #[async_trait]
    impl RegistryStore for PayerStore {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(Some(AgentConfigResult {
                config: self.owner_config.clone(),
                agent_pinned_model: None,
                is_coding_agent: self.is_coding_agent,
                is_discovered_agent: false,
                compress_enabled: false,
            }))
        }
        async fn fetch_agent_owner(&self, _: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            Ok(Uuid::parse_str(OWNER).ok())
        }
        async fn has_user_agent_config(&self, user: Uuid, _: Uuid) -> Result<bool, sqlx::Error> {
            assert_eq!(user, self.flow_user, "only the caller's own choice");
            if self.choice_read_fails {
                return Err(sqlx::Error::PoolTimedOut);
            }
            Ok(self.chosen.is_some())
        }
        async fn fetch_user_agent_config(
            &self,
            user: Uuid,
            _: Uuid,
        ) -> Result<Option<LLMConfig>, sqlx::Error> {
            assert_eq!(user, self.flow_user, "only the caller's own config");
            Ok(self.chosen.clone().or_else(|| self.default.clone()))
        }
        async fn fetch_user_secret(
            &self,
            holder: Uuid,
            _: &str,
        ) -> Result<Option<String>, sqlx::Error> {
            let key = format!("key-of-{holder}");
            Ok(Some(SecretsCrypto::for_user(holder).encrypt(&key)))
        }
        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<routing::attribution::LiveFlow>, sqlx::Error> {
            Ok(Some(routing::attribution::LiveFlow {
                user_id: Some(self.flow_user),
                context_id: Some("ses_payer".into()),
                mode: None,
                user_message: None,
                agent_is_participant: true,
            }))
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
            _: Option<Uuid>,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<crate::resolver::OrgEmbeddingDefault>, sqlx::Error> {
            Ok(None)
        }
    }

    fn install_master_key() {
        use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
        // SAFETY: every caller is #[serial]; no other test reads the env concurrently.
        unsafe {
            std::env::set_var("SECRETS_ENCRYPTION_KEY", BASE64.encode([7u8; 32]));
        }
    }

    fn hello() -> Value {
        json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] })
    }

    type Payers = Arc<crate::payer::RecordingPayer>;

    async fn chat_paid_by(
        payer: &Payers,
        base: String,
        store: &PayerStore,
    ) -> Result<Response, GatewayError> {
        let ctx = LlmRouterCtx {
            payer: payer.clone(),
            ..ctx_with(base)
        };
        let headers = auth_headers(&token());
        chat_core(&ctx, store, &headers, hello(), InboundFormat::OpenAi, None).await
    }

    fn asked(payer: &Payers) -> Vec<(Uuid, Uuid)> {
        payer.calls.lock().unwrap().clone()
    }

    /// One chat call, answered only when the provider receives `model` with `key-of-<key_holder>`.
    async fn paid_chat(store: &PayerStore, payer: &Payers, model: &str, key_holder: &str) {
        install_master_key();
        let mut server = mockito::Server::new_async().await;
        let mock = completion_mock(&mut server, model, &format!("key-of-{key_holder}")).await;
        chat_paid_by(payer, server.url(), store).await.unwrap();
        mock.assert_async().await;
    }

    /// A completion answered only for `model` with `Bearer <key>`, exactly once.
    async fn completion_mock(
        server: &mut mockito::ServerGuard,
        model: &str,
        key: &str,
    ) -> mockito::Mock {
        server
            .mock("POST", "/chat/completions")
            .match_header("authorization", format!("Bearer {key}").as_str())
            .match_body(mockito::Matcher::PartialJson(json!({ "model": model })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-p", "object": "chat.completion", "model": model,
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await
    }

    /// What the payer is asked about: the agent's owner, then the calling user.
    fn owner_and_caller() -> Vec<(Uuid, Uuid)> {
        vec![(
            Uuid::parse_str(OWNER).unwrap(),
            Uuid::parse_str(SHARED).unwrap(),
        )]
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_shared_caller_with_a_choice_runs_on_it_and_the_payer_is_never_asked() {
        // The payer would put them on the owner, so a call that reaches it fails the stub.
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Owner));
        let store = PayerStore::new(Some("chosen-model"), Some("default-model"));
        paid_chat(&store, &payer, "chosen-model", SHARED).await;
        assert!(asked(&payer).is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn the_owner_pays_for_a_shared_caller_without_a_choice() {
        // The caller has a default, which the owner's payment must not use.
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Owner));
        let store = PayerStore::new(None, Some("default-model"));
        paid_chat(&store, &payer, "owner-model", OWNER).await;
        assert_eq!(asked(&payer), owner_and_caller());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_shared_caller_the_payer_leaves_to_themselves_runs_on_their_default() {
        // Left to themselves outright, or after a spent allowance: the same config and key.
        let spent = Payer::CallerAfterAllowance {
            retry_after_secs: 120,
        };
        for left in [Payer::Caller, spent] {
            let payer = crate::payer::RecordingPayer::answering(Some(left));
            let store = PayerStore::new(None, Some("default-model"));
            paid_chat(&store, &payer, "default-model", SHARED).await;
            assert_eq!(asked(&payer), owner_and_caller());
        }
    }

    #[tokio::test]
    async fn a_spent_allowance_without_a_config_is_a_429_with_retry_after() {
        let spent = Payer::CallerAfterAllowance {
            retry_after_secs: 120,
        };
        let payer = crate::payer::RecordingPayer::answering(Some(spent));
        let store = PayerStore::new(None, None);
        let err = chat_paid_by(&payer, "http://unused".into(), &store)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                GatewayError::OwnerAllowanceSpent {
                    retry_after_secs: 120
                }
            ),
            "{err:?}"
        );
        let response = err.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "120");
        let body: Value = serde_json::from_str(&body_string(response).await).unwrap();
        assert_eq!(
            body["detail"],
            "Today's free model calls are used up. Connect your own model in LLM router to keep \
             going, or try again after 00:00 UTC."
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn the_owner_a_session_token_and_a_coding_agent_never_ask_the_payer() {
        install_master_key();
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Caller));
        // The owner calling their own agent.
        let owner_store = PayerStore {
            flow_user: Uuid::parse_str(OWNER).unwrap(),
            ..PayerStore::new(None, None)
        };
        paid_chat(&owner_store, &payer, "owner-model", OWNER).await;
        // A coding agent and a session token are never attributed to a flow user, so they
        // resolve on the agent owner whoever the token names.
        let coding_store = PayerStore {
            is_coding_agent: true,
            ..PayerStore::new(None, None)
        };
        let session = SessionClaims {
            agent_id: AGENT.into(),
            user_id: SHARED.into(),
            session_id: "ses_1".into(),
            flow_id: "flow".into(),
            jti: "jti".into(),
            aud: String::new(),
            iat: 0,
            exp: 0,
        };
        let ctx = LlmRouterCtx {
            payer: payer.clone(),
            ..ctx_with("http://unused".into())
        };
        for session in [None, Some(session)] {
            let routed = resolve_routed_request(
                &ctx,
                &coding_store,
                &HeaderMap::new(),
                AGENT.into(),
                OWNER.into(),
                RequestHint {
                    provider: Some("openai"),
                    model: None,
                },
                RequestSignals {
                    query: Some("hi".into()),
                    turn_ordinal: 1,
                    is_tool_continuation: false,
                },
                session,
            )
            .await
            .unwrap();
            assert_eq!(routed.resolved.model, "owner-model");
            assert_eq!(routed.resolved.api_key, format!("key-of-{OWNER}"));
        }
        assert!(asked(&payer).is_empty());
    }

    #[tokio::test]
    async fn a_payer_failure_is_internal_and_calls_no_provider() {
        let payer = crate::payer::RecordingPayer::answering(None);
        let store = PayerStore::new(None, Some("default-model"));
        let err = chat_paid_by(&payer, "http://unused".into(), &store)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, GatewayError::Internal(m) if m == "counter store down"),
            "{err:?}"
        );
        assert_eq!(asked(&payer), owner_and_caller());
    }

    #[tokio::test]
    async fn a_failed_choice_read_is_internal_and_the_payer_is_not_asked() {
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Owner));
        let store = PayerStore {
            choice_read_fails: true,
            ..PayerStore::new(Some("chosen-model"), None)
        };
        let err = chat_paid_by(&payer, "http://unused".into(), &store)
            .await
            .unwrap_err();
        let read_failed = "shared user choice read failed";
        assert!(
            matches!(&err, GatewayError::Internal(m) if m.starts_with(read_failed)),
            "{err:?}"
        );
        assert!(asked(&payer).is_empty());
    }

    /// Agents get no platform key; the platform keeps its own.
    fn no_agent_key_ctx(base: String, payer: &Payers) -> LlmRouterCtx {
        let ctx = ctx_with(base);
        let cfg = GatewayConfig {
            generic_key_fallback: false,
            platform_openai_api_key: String::new(),
            service_openai_api_key: "sk-service".into(),
            ..(*ctx.cfg).clone()
        };
        LlmRouterCtx {
            cfg: Arc::new(cfg),
            payer: payer.clone(),
            ..ctx
        }
    }

    /// The owner calling their own agent, which resolves to no config.
    fn unconfigured_owner() -> PayerStore {
        PayerStore {
            flow_user: Uuid::parse_str(OWNER).unwrap(),
            owner_config: None,
            ..PayerStore::new(None, None)
        }
    }

    fn owner_asked_about_themselves() -> Vec<(Uuid, Uuid)> {
        let owner = Uuid::parse_str(OWNER).unwrap();
        vec![(owner, owner)]
    }

    #[tokio::test]
    async fn an_owner_without_a_key_is_covered_on_the_platform_defaults() {
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Platform));
        let mut server = mockito::Server::new_async().await;
        // The request asks for `gpt-4o`; a covered call runs the platform default instead.
        let mock = completion_mock(&mut server, "gpt-4o-mini", "sk-service").await;
        let ctx = no_agent_key_ctx(server.url(), &payer);
        let headers = auth_headers(&token());
        let store = unconfigured_owner();
        chat_core(&ctx, &store, &headers, hello(), InboundFormat::OpenAi, None)
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(asked(&payer), owner_asked_about_themselves());
    }

    #[tokio::test]
    async fn an_owner_whose_allowance_is_spent_gets_the_429() {
        let spent = Payer::CallerAfterAllowance {
            retry_after_secs: 120,
        };
        let payer = crate::payer::RecordingPayer::answering(Some(spent));
        let ctx = no_agent_key_ctx("http://unused".into(), &payer);
        let headers = auth_headers(&token());
        let store = unconfigured_owner();
        let err = chat_core(&ctx, &store, &headers, hello(), InboundFormat::OpenAi, None)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                GatewayError::OwnerAllowanceSpent {
                    retry_after_secs: 120
                }
            ),
            "{err:?}"
        );
        let response = err.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "120");
        let body: Value = serde_json::from_str(&body_string(response).await).unwrap();
        assert_eq!(
            body["detail"],
            "Today's free model calls are used up. Connect your own model in LLM router to keep \
             going, or try again after 00:00 UTC."
        );
    }

    #[tokio::test]
    async fn an_owner_the_payer_leaves_alone_keeps_the_no_api_key_error() {
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Caller));
        let ctx = no_agent_key_ctx("http://unused".into(), &payer);
        let headers = auth_headers(&token());
        let store = unconfigured_owner();
        let err = chat_core(&ctx, &store, &headers, hello(), InboundFormat::OpenAi, None)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::NoApiKey { .. }), "{err:?}");
        assert_eq!(asked(&payer), owner_asked_about_themselves());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn an_owner_with_a_working_config_is_never_counted_on_the_no_agent_key_ctx() {
        install_master_key();
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Platform));
        let mut server = mockito::Server::new_async().await;
        let mock = completion_mock(&mut server, "owner-model", &format!("key-of-{OWNER}")).await;
        let ctx = no_agent_key_ctx(server.url(), &payer);
        let headers = auth_headers(&token());
        let store = PayerStore {
            flow_user: Uuid::parse_str(OWNER).unwrap(),
            ..PayerStore::new(None, None)
        };
        chat_core(&ctx, &store, &headers, hello(), InboundFormat::OpenAi, None)
            .await
            .unwrap();
        mock.assert_async().await;
        assert!(asked(&payer).is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_shared_caller_is_never_put_on_the_platform() {
        // The platform covers only an owner's own call: for anyone else the answer is their own config.
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Platform));
        let store = PayerStore::new(None, Some("default-model"));
        paid_chat(&store, &payer, "default-model", SHARED).await;
        assert_eq!(asked(&payer), owner_and_caller());
    }

    #[tokio::test]
    async fn an_unattributed_call_that_finds_no_key_never_asks_the_payer() {
        let payer = crate::payer::RecordingPayer::answering(Some(Payer::Platform));
        let ctx = no_agent_key_ctx("http://unused".into(), &payer);
        // A coding agent and a session token are never attributed to a flow user.
        let coding_store = PayerStore {
            is_coding_agent: true,
            owner_config: None,
            ..PayerStore::new(None, None)
        };
        let session = SessionClaims {
            agent_id: AGENT.into(),
            user_id: OWNER.into(),
            session_id: "ses_1".into(),
            flow_id: "flow".into(),
            jti: "jti".into(),
            aud: String::new(),
            iat: 0,
            exp: 0,
        };
        for session in [None, Some(session)] {
            let routed = resolve_routed_request(
                &ctx,
                &coding_store,
                &HeaderMap::new(),
                AGENT.into(),
                OWNER.into(),
                RequestHint {
                    provider: Some("openai"),
                    model: None,
                },
                RequestSignals {
                    query: Some("hi".into()),
                    turn_ordinal: 1,
                    is_tool_continuation: false,
                },
                session,
            )
            .await;
            let Err(err) = routed else {
                panic!("an unattributed call was covered");
            };
            assert!(matches!(err, GatewayError::NoApiKey { .. }), "{err:?}");
        }
        assert!(asked(&payer).is_empty());
    }

    /// A store whose session-flow lookup answers a fixed later-turn flow.
    struct SessionStore(Store);
    #[async_trait]
    impl RegistryStore for SessionStore {
        async fn fetch_llm_config(
            &self,
            id: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            self.0.fetch_llm_config(id).await
        }
        async fn fetch_agent_owner(&self, id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            self.0.fetch_agent_owner(id).await
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_live_flow(
            &self,
            flow_id: &str,
            agent_id: Uuid,
            window_secs: i64,
        ) -> Result<Option<routing::attribution::LiveFlow>, sqlx::Error> {
            self.0.fetch_live_flow(flow_id, agent_id, window_secs).await
        }
        async fn attribute_session_flow(
            &self,
            _: Option<&str>,
            _: &SessionClaims,
        ) -> nasiko_session_token::attribution::AttributedFlow {
            nasiko_session_token::attribution::AttributedFlow {
                flow_id: "later-turn-flow".into(),
                source: nasiko_session_token::attribution::FlowSource::Session,
            }
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
            _: Option<Uuid>,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<crate::resolver::OrgEmbeddingDefault>, sqlx::Error> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn session_token_call_is_attributed_to_the_flow_the_store_resolves() {
        let ctx = ctx_with("http://unused".into());
        let store = SessionStore(Store {
            config: None,
            is_coding_agent: true,
            is_discovered_agent: false,
            compress_enabled: false,
        });
        let claims = SessionClaims {
            agent_id: AGENT.into(),
            user_id: OWNER.into(),
            session_id: "ses_1".into(),
            flow_id: "first-turn-flow".into(),
            jti: "jti".into(),
            aud: String::new(),
            iat: 0,
            exp: 0,
        };
        let routed = resolve_routed_request(
            &ctx,
            &store,
            &HeaderMap::new(),
            AGENT.into(),
            OWNER.into(),
            RequestHint {
                provider: Some("openai"),
                model: None,
            },
            RequestSignals {
                query: Some("hello".into()),
                turn_ordinal: 2,
                is_tool_continuation: false,
            },
            Some(claims),
        )
        .await
        .unwrap();
        assert_eq!(routed.flow_id.as_deref(), Some("later-turn-flow"));
        assert_eq!(
            routed.attribution_source,
            Some(routing::attribution::AttributionSource::Session)
        );
    }

    #[tokio::test]
    async fn coding_agent_with_no_traceparent_still_gets_classified_not_pinned_to_config() {
        // The bug this fixes: a coding-agent CLI (Claude Code, Codex, ...) never has a
        // traceparent tied to a `flows` row, so `derive_boundary_signals` alone always goes
        // inert for it — which pins every request to Level 4 (the attached llm_config) and
        // makes the prompt classifier (Level 3) unreachable. `is_coding_agent: true` must
        // make `resolve_routed_request` derive signals from the transcript instead, so the
        // classifier actually gets to run.
        let mut ctx = ctx_with("http://unused".into());
        ctx.tier_registry = Arc::new(crate::routing::registry::test_support::StubRegistry);
        let store = Store {
            compress_enabled: false,
            // A tiered config (tier routing is opt-in by tier models) whose model is NOT one
            // of the tier models — if the classifier never fires, the resolved model will be
            // exactly this. If it does fire, it will be the config's tier-1 model or one of
            // openai's seeded registry models (gpt-5.4 / gpt-4o-mini) for the unset tiers.
            config: Some(LLMConfig {
                provider: "openai".into(),
                model: Some("static-configured-model".into()),
                fallback_models: vec![],
                temperature: None,
                max_tokens: None,
                api_key_secret_name: None,
                pinned: false,
                pinned_model: None,
                tier1_model: Some("gpt-5.5".into()),
                tier2_model: None,
                tier3_model: None,
                embedding_provider: None,
                embedding_model: None,
                embedding_api_key_secret_name: None,
                embedding_model_override: false,
            }),
            is_coding_agent: true,
            is_discovered_agent: false,
        };
        let routed = resolve_routed_request(
            &ctx,
            &store,
            &HeaderMap::new(), // no traceparent — a coding-agent CLI never sends one
            AGENT.into(),
            OWNER.into(),
            RequestHint {
                provider: Some("openai"),
                model: None,
            },
            RequestSignals {
                query: Some("write a function that reverses a string".into()),
                turn_ordinal: 1,
                is_tool_continuation: false,
            },
            None,
        )
        .await
        .unwrap();
        assert!(
            ["gpt-5.5", "gpt-5.4", "gpt-4o-mini"].contains(&routed.resolved.model.as_str()),
            "expected a classifier-selected tier model, got {}",
            routed.resolved.model
        );
    }

    use crate::routing::cache::test_support::ForgetLog;

    const STUB_OPENAI_TIERS: [&str; 3] = ["gpt-5.5", "gpt-5.4", "gpt-4o-mini"];

    fn tiered_store(fallback_models: Vec<String>, pinned_model: Option<&str>) -> Store {
        let mut config = openai_config();
        config.model = Some("static-configured-model".into());
        config.tier1_model = Some("gpt-5.5".into());
        config.fallback_models = fallback_models;
        config.pinned = pinned_model.is_some();
        config.pinned_model = pinned_model.map(str::to_string);
        Store {
            config: Some(config),
            is_coding_agent: true,
            is_discovered_agent: false,
            compress_enabled: false,
        }
    }

    fn tiered_ctx(base: String) -> (LlmRouterCtx, Arc<ForgetLog>) {
        let mut ctx = ctx_with(base);
        ctx.tier_registry = Arc::new(crate::routing::registry::test_support::StubRegistry);
        let log = Arc::new(ForgetLog::default());
        ctx.router_cache = log.clone();
        (ctx, log)
    }

    async fn route_tiered(ctx: &LlmRouterCtx, store: &Store) -> RoutedRequest {
        resolve_routed_request(
            ctx,
            store,
            &HeaderMap::new(),
            AGENT.into(),
            OWNER.into(),
            RequestHint {
                provider: Some("openai"),
                model: None,
            },
            RequestSignals {
                query: Some("write a function that reverses a string".into()),
                turn_ordinal: 1,
                is_tool_continuation: false,
            },
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_tier_model_keeps_the_configured_model_as_its_first_fallback() {
        let (ctx, _) = tiered_ctx("http://unused".into());
        let routed = route_tiered(
            &ctx,
            &tiered_store(vec!["anthropic/claude-haiku-4-5".into()], None),
        )
        .await;
        assert!(STUB_OPENAI_TIERS.contains(&routed.resolved.model.as_str()));
        assert_eq!(
            routed.resolved.fallback_models,
            vec![
                "openai/static-configured-model".to_string(),
                "anthropic/claude-haiku-4-5".to_string(),
            ]
        );
        assert!(routed.sticky_conv_id.is_some());
    }

    #[tokio::test]
    async fn a_configured_model_already_in_the_fallbacks_is_not_added_twice() {
        for listed in ["static-configured-model", "openai/static-configured-model"] {
            let (ctx, _) = tiered_ctx("http://unused".into());
            let routed = route_tiered(&ctx, &tiered_store(vec![listed.into()], None)).await;
            assert_eq!(routed.resolved.fallback_models, vec![listed.to_string()]);
        }
    }

    #[tokio::test]
    async fn a_pinned_agent_still_gets_no_fallbacks() {
        let (ctx, _) = tiered_ctx("http://unused".into());
        let routed = route_tiered(
            &ctx,
            &tiered_store(vec!["openai/gpt-4o".into()], Some("gpt-4.1")),
        )
        .await;
        assert_eq!(routed.resolved.model, "gpt-4.1");
        assert!(routed.resolved.fallback_models.is_empty());
        assert!(routed.sticky_conv_id.is_none());
    }

    async fn tier_models_answer(server: &mut mockito::ServerGuard, status: usize, body: &str) {
        for model in STUB_OPENAI_TIERS {
            server
                .mock("POST", "/chat/completions")
                .match_body(mockito::Matcher::PartialJson(json!({ "model": model })))
                .with_status(status)
                .with_header("content-type", "application/json")
                .with_body(body)
                .create_async()
                .await;
        }
    }

    const MODEL_GONE: &str =
        r#"{"error":{"message":"The model does not exist","code":"model_not_found"}}"#;

    fn completion(model: &str) -> String {
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": model,
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
        .to_string()
    }

    fn reverse_question() -> Value {
        json!({ "messages": [{ "role": "user", "content": "write a function that reverses a string" }] })
    }

    fn agent_only_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            format!("Bearer {}", token()).parse().unwrap(),
        );
        h
    }

    #[tokio::test]
    async fn a_missing_tier_model_falls_back_to_the_configured_model_and_drops_the_sticky_choice() {
        for stream in [false, true] {
            let mut server = mockito::Server::new_async().await;
            tier_models_answer(&mut server, 404, MODEL_GONE).await;
            let configured = server
                .mock("POST", "/chat/completions")
                .match_body(mockito::Matcher::PartialJson(
                    json!({ "model": "static-configured-model" }),
                ))
                .with_status(200)
                .with_header(
                    "content-type",
                    if stream {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                )
                .with_body(if stream {
                    "data: [DONE]\n\n".to_string()
                } else {
                    completion("static-configured-model")
                })
                .expect(1)
                .create_async()
                .await;
            let (ctx, log) = tiered_ctx(server.url());
            let resp = chat_core(
                &ctx,
                &tiered_store(vec![], None),
                &agent_only_headers(),
                reverse_question(),
                InboundFormat::OpenAi,
                Some(stream),
            )
            .await
            .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "stream={stream}");
            configured.assert_async().await;
            let forgotten = log.forgotten.lock().unwrap().clone();
            assert_eq!(forgotten.len(), 1, "stream={stream}");
            assert_eq!(forgotten[0].1, AGENT);
        }
    }

    #[tokio::test]
    async fn a_tier_model_that_answers_keeps_its_sticky_choice() {
        let mut server = mockito::Server::new_async().await;
        tier_models_answer(&mut server, 200, &completion("gpt-5.5")).await;
        let (ctx, log) = tiered_ctx(server.url());
        let resp = chat_core(
            &ctx,
            &tiered_store(vec![], None),
            &agent_only_headers(),
            reverse_question(),
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(log.forgotten.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn non_coding_agent_with_no_traceparent_is_rejected() {
        // Ordinary agents remain subject to development's strict flow attribution.
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: Some(LLMConfig {
                provider: "openai".into(),
                model: Some("static-configured-model".into()),
                fallback_models: vec![],
                temperature: None,
                max_tokens: None,
                api_key_secret_name: None,
                pinned: false,
                pinned_model: None,
                tier1_model: None,
                tier2_model: None,
                tier3_model: None,
                embedding_provider: None,
                embedding_model: None,
                embedding_api_key_secret_name: None,
                embedding_model_override: false,
            }),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        let result = resolve_routed_request(
            &ctx,
            &store,
            &HeaderMap::new(),
            AGENT.into(),
            OWNER.into(),
            RequestHint {
                provider: Some("openai"),
                model: None,
            },
            RequestSignals {
                query: Some("write a function that reverses a string".into()),
                turn_ordinal: 1,
                is_tool_continuation: false,
            },
            None,
        )
        .await;
        let Err(error) = result else {
            panic!("expected strict attribution to reject the request");
        };
        assert!(matches!(error, GatewayError::Forbidden(_)));
    }

    #[tokio::test]
    async fn discovered_agent_with_no_traceparent_is_billed_to_its_owner() {
        // An A2A agent the CLI discovered runs on the user's machine and is never called
        // through Nasiko, so it never has a traceparent. Its calls are accepted and billed to
        // its owner, while it stays an ordinary agent for everything coding agents skip.
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: Some(LLMConfig {
                provider: "openai".into(),
                model: Some("static-configured-model".into()),
                fallback_models: vec![],
                temperature: None,
                max_tokens: None,
                api_key_secret_name: None,
                pinned: false,
                pinned_model: None,
                tier1_model: None,
                tier2_model: None,
                tier3_model: None,
                embedding_provider: None,
                embedding_model: None,
                embedding_api_key_secret_name: None,
                embedding_model_override: false,
            }),
            is_coding_agent: false,
            is_discovered_agent: true,
            compress_enabled: false,
        };
        let routed = resolve_routed_request(
            &ctx,
            &store,
            &HeaderMap::new(),
            AGENT.into(),
            OWNER.into(),
            RequestHint {
                provider: Some("openai"),
                model: None,
            },
            RequestSignals {
                query: Some("summarise the latest research".into()),
                turn_ordinal: 1,
                is_tool_continuation: false,
            },
            None,
        )
        .await
        .expect("a discovered agent's call is accepted");
        assert_eq!(routed.owner_id, OWNER);
        assert_eq!(routed.flow_id, None);
        assert!(routed.resolved.is_discovered_agent);
        assert!(!routed.resolved.is_coding_agent);
    }

    #[tokio::test]
    async fn unregistered_provider_is_bad_request() {
        // A non-built-in provider with no active custom_providers row is a client
        // error (400), resolved before any provider client is built — it must not
        // fall through to the OpenAI key/base URL, nor surface as an opaque 500.
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: Some(LLMConfig {
                provider: "cohere".into(),
                model: Some("command-r".into()),
                fallback_models: vec![],
                temperature: None,
                max_tokens: None,
                api_key_secret_name: None,
                pinned: false,
                pinned_model: None,
                tier1_model: None,
                tier2_model: None,
                tier3_model: None,
                embedding_provider: None,
                embedding_model: None,
                embedding_api_key_secret_name: None,
                embedding_model_override: false,
            }),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        let body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] });
        let err = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            body,
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
    }

    // ── compact tool schemas: the opt-in flag reaches the wire, and off means untouched ────

    fn weather_params() -> Value {
        json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]})
    }

    fn weather_request() -> Value {
        json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "Weather in Pune?" }],
            "tools": [{ "type": "function", "function": {
                "name": "get_weather", "description": "Weather for a city.", "parameters": weather_params()
            }}]
        })
    }

    /// One request against a mock provider that answers with `replies` in order. Returns every
    /// body the provider received and the client's response.
    async fn compact_round(
        flag: bool,
        replies: Vec<Value>,
        format: InboundFormat,
        body: Value,
    ) -> (Vec<Value>, Value) {
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let capture = Arc::clone(&seen);
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let mut seen = capture.lock().unwrap_or_else(|e| e.into_inner());
                let body = request.body().map(Vec::as_slice).unwrap_or_default();
                seen.push(serde_json::from_slice(body).unwrap_or(Value::Null));
                let reply = &replies[(seen.len() - 1).min(replies.len() - 1)];
                json!({
                    "id": "chatcmpl-c", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": reply, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
                })
                .to_string()
                .into_bytes()
            })
            .create_async()
            .await;

        let mut ctx = ctx_with(server.url());
        ctx.cfg = Arc::new(GatewayConfig {
            compact_tools_enabled: flag,
            ..(*ctx.cfg).clone()
        });
        let store = Store {
            config: Some(openai_config()),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: true,
        };
        let resp = chat_core(&ctx, &store, &auth_headers(&token()), body, format, None)
            .await
            .unwrap();
        let out: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        let bodies = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        (bodies, out)
    }

    const COMPACT_CALL: &str = r#"<<call get_weather {"city":"Pune"}>>"#;

    #[tokio::test]
    async fn compact_tools_off_sends_native_tools_and_leaves_the_reply_alone() {
        let reply = json!({ "role": "assistant", "content": COMPACT_CALL });
        let (bodies, out) =
            compact_round(false, vec![reply], InboundFormat::OpenAi, weather_request()).await;
        assert_eq!(bodies.len(), 1);
        assert_eq!(
            bodies[0]["tools"],
            weather_request()["tools"],
            "native tools changed"
        );
        assert!(
            !bodies[0].to_string().contains("<<call"),
            "compact instructions leaked"
        );
        // Off, the reply is not inspected: compact-looking text reaches the client verbatim.
        assert_eq!(out["choices"][0]["message"]["content"], COMPACT_CALL);
        assert!(out["choices"][0]["message"]["tool_calls"].is_null());
    }

    #[tokio::test]
    async fn compact_tools_on_sends_signatures_and_returns_native_tool_calls() {
        let reply = json!({ "role": "assistant", "content": COMPACT_CALL });
        let (bodies, out) =
            compact_round(true, vec![reply], InboundFormat::OpenAi, weather_request()).await;
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].get("tools").is_none(), "native tools still sent");
        let messages = bodies[0]["messages"].as_array().unwrap();
        let system = messages.last().unwrap();
        assert_eq!(system["role"], "system");
        assert!(
            system["content"].as_str().unwrap().starts_with(
                "get_weather(city:str) - Weather for a city.
"
            ),
            "{system}"
        );
        let choice = &out["choices"][0];
        assert_eq!(choice["finish_reason"], "tool_calls");
        let call = &choice["message"]["tool_calls"][0];
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "get_weather");
        assert_eq!(call["function"]["arguments"], r#"{"city":"Pune"}"#);
        assert!(call["id"].as_str().unwrap().starts_with("call_"));
    }

    #[tokio::test]
    async fn compact_reply_that_does_not_decode_is_retried_with_native_tools() {
        let bad =
            json!({ "role": "assistant", "content": r#"<<call get_weather {"town":"Pune"}>>"# });
        let native = json!({ "role": "assistant", "content": null, "tool_calls": [{
            "id": "call_native", "type": "function",
            "function": { "name": "get_weather", "arguments": "{\"city\":\"Pune\"}" }
        }]});
        let (bodies, out) = compact_round(
            true,
            vec![bad, native],
            InboundFormat::OpenAi,
            weather_request(),
        )
        .await;
        assert_eq!(
            bodies.len(),
            2,
            "expected one compact attempt and one native retry"
        );
        assert!(bodies[0].get("tools").is_none());
        assert_eq!(bodies[1]["tools"], weather_request()["tools"]);
        assert_eq!(
            out["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_native"
        );
    }

    #[tokio::test]
    async fn anthropic_client_gets_tool_use_blocks_from_compact_calls() {
        let body = json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Weather in Pune?" }],
            "tools": [{ "name": "get_weather", "description": "Weather for a city.", "input_schema": weather_params() }]
        });
        let reply = json!({ "role": "assistant", "content": COMPACT_CALL });
        let (bodies, out) = compact_round(true, vec![reply], InboundFormat::Anthropic, body).await;
        assert!(bodies[0].get("tools").is_none());
        let block = out["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["type"] == "tool_use")
            .unwrap_or_else(|| panic!("no tool_use block: {out}"));
        assert_eq!(block["name"], "get_weather");
        assert_eq!(block["input"], json!({"city": "Pune"}));
        assert_eq!(out["stop_reason"], "tool_use");
    }

    // ── cache markers: an agent's own markers reach the wire ──────────────────────────────
    //
    // The adapters' unit tests cover each translation. These cover what sits between an
    // Anthropic-SDK agent and the provider: resolution, compression, brevity and the fallback
    // chain, any of which could drop or misplace a marker.

    /// [`Store`] plus one registered custom provider, for a destination such as Bedrock.
    struct WithCustomProvider {
        inner: Store,
        custom: crate::resolver::CustomProvider,
    }
    #[async_trait]
    impl RegistryStore for WithCustomProvider {
        async fn fetch_llm_config(
            &self,
            agent: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            self.inner.fetch_llm_config(agent).await
        }
        async fn fetch_agent_owner(&self, agent: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
            self.inner.fetch_agent_owner(agent).await
        }
        async fn fetch_user_secret(
            &self,
            owner: Uuid,
            name: &str,
        ) -> Result<Option<String>, sqlx::Error> {
            self.inner.fetch_user_secret(owner, name).await
        }
        async fn fetch_live_flow(
            &self,
            trace: &str,
            agent: Uuid,
            window: i64,
        ) -> Result<Option<routing::attribution::LiveFlow>, sqlx::Error> {
            self.inner.fetch_live_flow(trace, agent, window).await
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
            _: Option<Uuid>,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(Some(self.custom.clone()))
        }
        async fn fetch_org_embedding_default(
            &self,
        ) -> Result<Option<crate::resolver::OrgEmbeddingDefault>, sqlx::Error> {
            self.inner.fetch_org_embedding_default().await
        }
    }

    /// Mocks one provider route answering `status` + `reply`, recording every body it is sent.
    async fn capturing_mock(
        server: &mut mockito::ServerGuard,
        path: impl Into<mockito::Matcher>,
        status: usize,
        reply: Value,
    ) -> (mockito::Mock, Arc<Mutex<Vec<Value>>>) {
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let capture = Arc::clone(&seen);
        let reply = reply.to_string();
        let mock = server
            .mock("POST", path)
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let body = request.body().map(Vec::as_slice).unwrap_or_default();
                capture
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(serde_json::from_slice(body).unwrap_or(Value::Null));
                reply.clone().into_bytes()
            })
            .create_async()
            .await;
        (mock, seen)
    }

    fn sent(seen: &Arc<Mutex<Vec<Value>>>) -> Value {
        let bodies = seen.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(bodies.len(), 1, "expected exactly one provider call");
        bodies[0].clone()
    }

    fn anthropic_reply() -> Value {
        json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-haiku-5-5",
            "content": [{ "type": "text", "text": "ok" }], "stop_reason": "end_turn",
            "usage": { "input_tokens": 5, "output_tokens": 2 }
        })
    }

    fn ephemeral() -> Value {
        json!({ "type": "ephemeral" })
    }

    /// An Anthropic-SDK agent mid tool loop, marked on its system prompt, its tool and the
    /// latest tool result: where agents such as Claude Code put their markers.
    fn marked_tool_loop(tool_result: &str) -> Value {
        json!({
            "model": "claude-haiku-5-5",
            "max_tokens": 256,
            "system": [
                { "type": "text", "text": "You are a coding agent." },
                { "type": "text", "text": "Repository map.", "cache_control": ephemeral() }
            ],
            "tools": [{ "name": "read_logs", "input_schema": { "type": "object" }, "cache_control": ephemeral() }],
            "messages": [
                { "role": "user", "content": "why did the deploy fail?" },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_1", "name": "read_logs", "input": {} }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": tool_result, "cache_control": ephemeral() }
                ]}
            ]
        })
    }

    /// `ctx_with` pointed at a mocked Anthropic API, with a platform key for it.
    fn anthropic_ctx(base: String) -> LlmRouterCtx {
        let mut ctx = ctx_with(base.clone());
        ctx.cfg = Arc::new(GatewayConfig {
            anthropic_api_base: base,
            platform_anthropic_api_key: "sk-ant-platform".into(),
            ..(*ctx.cfg).clone()
        });
        ctx
    }

    fn anthropic_store(fallback_models: Vec<String>) -> Store {
        Store {
            config: Some(LLMConfig {
                provider: "anthropic".into(),
                model: Some("claude-haiku-5-5".into()),
                fallback_models,
                ..openai_config()
            }),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: true,
        }
    }

    #[tokio::test]
    async fn an_anthropic_agents_markers_reach_anthropic_with_compression_on() {
        let mut server = mockito::Server::new_async().await;
        let (_mock, seen) = capturing_mock(&mut server, "/messages", 200, anthropic_reply()).await;

        let ctx = anthropic_ctx(server.url());
        chat_core(
            &ctx,
            &anthropic_store(vec![]),
            &auth_headers(&token()),
            marked_tool_loop(&noisy_tool_result()),
            InboundFormat::Anthropic,
            None,
        )
        .await
        .unwrap();

        let body = sent(&seen);
        assert_eq!(
            body["system"],
            json!([
                { "type": "text", "text": "You are a coding agent." },
                { "type": "text", "text": "Repository map.", "cache_control": ephemeral() }
            ])
        );
        assert_eq!(body["tools"][0]["cache_control"], ephemeral());
        let result = &body["messages"][2]["content"][0];
        assert_eq!(result["cache_control"], ephemeral());
        assert!(
            result["content"].as_str().unwrap().contains("lines elided"),
            "the marked tool result was not compressed: {result}"
        );
    }

    fn agent_caller() -> AuthedCaller {
        AuthedCaller::Agent {
            agent_id: AGENT.into(),
            owner_id: OWNER.into(),
        }
    }

    fn count_request(model: &str) -> Value {
        json!({
            "model": model,
            "system": "be brief",
            "messages": [{"role": "user", "content": "how many tokens is this?"}]
        })
    }

    async fn response_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn count_tokens_reaches_anthropic_with_the_agents_model_and_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/messages/count_tokens")
            .match_header("x-api-key", "sk-ant-platform")
            .match_header("anthropic-version", "2023-06-01")
            .match_body(mockito::Matcher::Json(count_request("claude-haiku-5-5")))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"input_tokens":42}"#)
            .expect(1)
            .create_async()
            .await;

        let response = count_tokens_as(
            &anthropic_ctx(server.url()),
            &anthropic_store(vec![]),
            agent_caller(),
            count_request("claude-opus-4-1"),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, json!({"input_tokens": 42}));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn count_tokens_keeps_an_anthropic_refusal_status() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/messages/count_tokens")
            .with_status(400)
            .with_body(r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: field required"}}"#)
            .create_async()
            .await;

        let err = count_tokens_as(
            &anthropic_ctx(server.url()),
            &anthropic_store(vec![]),
            agent_caller(),
            json!({"model": "x"}),
        )
        .await
        .unwrap_err();

        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let detail = response_json(response).await["detail"].to_string();
        assert!(detail.contains("messages: field required"), "{detail}");
    }

    #[tokio::test]
    async fn count_tokens_for_an_agent_not_on_anthropic_is_501_and_calls_nothing() {
        let server = mockito::Server::new_async().await;
        let store = Store {
            config: Some(openai_config()),
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };

        let response = count_tokens_as(
            &anthropic_ctx(server.url()),
            &store,
            agent_caller(),
            count_request("claude-haiku-5-5"),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            response_json(response).await["detail"],
            "count_tokens is available only when the agent's model is served by Anthropic (this agent uses openai)"
        );
    }

    #[tokio::test]
    async fn unknown_router_paths_answer_a_json_404_and_known_ones_are_untouched() {
        let send = |method: &str, path: &str| {
            let request = axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();
            tower::ServiceExt::oneshot(crate::router(ctx_with("http://unused".into())), request)
        };

        for (method, path) in [
            ("GET", "/v1/nope"),
            ("POST", "/v1/messages/batches"),
            ("POST", "/v1beta/nope"),
            ("GET", "/v1beta/models/gemini:generateContent/extra"),
        ] {
            let response = send(method, path).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
            assert_eq!(
                response_json(response).await,
                json!({
                    "detail": format!("no such router endpoint: {method} {path}"),
                    "error": {
                        "message": format!("no such router endpoint: {method} {path}"),
                        "type": "not_found_error"
                    }
                })
            );
        }

        let wrong_method = send("GET", "/v1/chat/completions").await.unwrap();
        assert_eq!(wrong_method.status(), StatusCode::UNAUTHORIZED);
        let unauthenticated = send("POST", "/v1/messages/count_tokens").await.unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
        let health = send("GET", "/v1/health").await.unwrap();
        assert_eq!(health.status(), StatusCode::OK);
    }

    struct OneFile(&'static [u8]);

    #[async_trait]
    impl crate::attachments::AttachmentStore for OneFile {
        async fn fetch(
            &self,
            _: &crate::attachments::AttachmentRef,
        ) -> Result<bytes::Bytes, crate::attachments::AttachmentFetchError> {
            Ok(bytes::Bytes::from_static(self.0))
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png";

    fn png_marker_line() -> String {
        crate::attachments::MarkerSigner::from_secret(SECRET)
            .unwrap()
            .marker_line(&crate::attachments::MarkerClaims {
                reference: crate::attachments::AttachmentRef::ChatUpload {
                    session_id: "sess".into(),
                    file_id: Uuid::from_u128(1),
                },
                filename: "fact.png".into(),
                media_type: "image/png".into(),
                size_bytes: PNG.len() as u64,
                flow_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
                expires_at: chrono::Utc::now().timestamp() + 600,
            })
    }

    fn anthropic_turn_with_marker() -> Value {
        json!({
            "model": "claude-haiku-5-5",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": format!("What is in this?\n{}", png_marker_line()) }]
        })
    }

    #[tokio::test]
    async fn a_marked_chat_file_reaches_anthropic_as_an_image_block() {
        use base64::Engine as _;
        let mut server = mockito::Server::new_async().await;
        let (_mock, seen) = capturing_mock(&mut server, "/messages", 200, anthropic_reply()).await;
        let mut ctx = anthropic_ctx(server.url());
        ctx.marker_signer = crate::attachments::MarkerSigner::from_secret(SECRET).map(Arc::new);
        ctx.attachment_store = Arc::new(OneFile(PNG));

        chat_core(
            &ctx,
            &anthropic_store(vec![]),
            &auth_headers(&token()),
            anthropic_turn_with_marker(),
            InboundFormat::Anthropic,
            None,
        )
        .await
        .unwrap();

        let content = &sent(&seen)["messages"][0]["content"];
        assert_eq!(
            content[0],
            json!({ "type": "text", "text": "What is in this?\n" })
        );
        assert_eq!(content[1]["type"], "image");
        assert_eq!(
            content[1]["source"],
            json!({
                "type": "base64",
                "media_type": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(PNG)
            })
        );
        assert!(
            !content
                .to_string()
                .contains(crate::attachments::TOKEN_PREFIX)
        );
    }

    #[tokio::test]
    async fn without_a_signing_key_a_marker_stays_plain_text() {
        let mut server = mockito::Server::new_async().await;
        let (_mock, seen) = capturing_mock(&mut server, "/messages", 200, anthropic_reply()).await;
        let mut ctx = anthropic_ctx(server.url());
        ctx.attachment_store = Arc::new(OneFile(PNG));

        let turn = anthropic_turn_with_marker();
        chat_core(
            &ctx,
            &anthropic_store(vec![]),
            &auth_headers(&token()),
            turn.clone(),
            InboundFormat::Anthropic,
            None,
        )
        .await
        .unwrap();

        let content = &sent(&seen)["messages"][0]["content"];
        let text = content
            .as_str()
            .or_else(|| content[0]["text"].as_str())
            .unwrap();
        assert_eq!(text, turn["messages"][0]["content"].as_str().unwrap());
    }

    #[tokio::test]
    async fn brevity_lands_after_the_agents_last_marker() {
        let mut server = mockito::Server::new_async().await;
        let (_mock, seen) = capturing_mock(&mut server, "/messages", 200, anthropic_reply()).await;

        let mut ctx = anthropic_ctx(server.url());
        ctx.cfg = Arc::new(GatewayConfig {
            brevity_enabled: true,
            brevity_min_bytes: 0,
            brevity_holdout_pct: 0,
            ..(*ctx.cfg).clone()
        });
        chat_core(
            &ctx,
            &anthropic_store(vec![]),
            &auth_headers(&token()),
            json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 256,
                "system": [{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral() }],
                "messages": [{ "role": "user", "content": [
                    { "type": "text", "text": "Summarise the repository.", "cache_control": ephemeral() }
                ]}]
            }),
            InboundFormat::Anthropic,
            None,
        )
        .await
        .unwrap();

        // In `system` it would precede the user turn's marker, and every turn that skips brevity
        // would miss the cache from there on.
        let body = sent(&seen);
        assert_eq!(
            body["system"],
            json!([{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral() }])
        );
        assert_eq!(
            body["messages"][0]["content"],
            json!([
                { "type": "text", "text": "Summarise the repository.", "cache_control": ephemeral() },
                { "type": "text", "text": crate::brevity::DIRECTIVE }
            ])
        );
    }

    #[tokio::test]
    async fn an_anthropic_agents_markers_become_cache_points_on_bedrock() {
        let mut server = mockito::Server::new_async().await;
        let reply = json!({
            "output": { "message": { "role": "assistant", "content": [{ "text": "ok" }] } },
            "stopReason": "end_turn",
            "usage": { "inputTokens": 5, "outputTokens": 2, "totalTokens": 7 }
        });
        let path = mockito::Matcher::Regex(r"^/model/[^/]+/converse$".into());
        let (_mock, seen) = capturing_mock(&mut server, path, 200, reply).await;

        let ctx = ctx_with(server.url());
        let store = WithCustomProvider {
            inner: Store {
                config: Some(LLMConfig {
                    provider: "aws-bedrock".into(),
                    model: Some("us.anthropic.claude-haiku-5-5".into()),
                    ..openai_config()
                }),
                is_coding_agent: false,
                is_discovered_agent: false,
                compress_enabled: true,
            },
            custom: crate::resolver::CustomProvider {
                base_url: server.url(),
                dialect: crate::providers::ProviderDialect::BedrockConverse,
                api_key: "bedrock-key".into(),
                default_model: None,
                public_only: false,
            },
        };
        chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            marked_tool_loop("fn main() {}"),
            InboundFormat::Anthropic,
            None,
        )
        .await
        .unwrap();

        let body = sent(&seen);
        let cache_point = json!({ "cachePoint": { "type": "default" } });
        assert_eq!(
            body["system"],
            json!([{ "text": "You are a coding agent." }, { "text": "Repository map." }, cache_point])
        );
        assert_eq!(body["toolConfig"]["tools"][1], cache_point);
        assert_eq!(body["messages"][2]["content"][1], cache_point);
        assert!(!body.to_string().contains("cache_control"));
    }

    #[tokio::test]
    async fn a_fallback_from_anthropic_to_openai_sends_openai_no_markers() {
        let mut server = mockito::Server::new_async().await;
        let overloaded = json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } });
        let (_anthropic, to_anthropic) =
            capturing_mock(&mut server, "/messages", 529, overloaded).await;
        let openai_reply = json!({
            "id": "chatcmpl-f", "object": "chat.completion", "model": "gpt-4o-mini",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
        });
        let (_openai, to_openai) =
            capturing_mock(&mut server, "/chat/completions", 200, openai_reply).await;

        let ctx = anthropic_ctx(server.url());
        chat_core(
            &ctx,
            &anthropic_store(vec!["openai/gpt-4o-mini".into()]),
            &auth_headers(&token()),
            marked_tool_loop("fn main() {}"),
            InboundFormat::Anthropic,
            None,
        )
        .await
        .unwrap();

        assert!(sent(&to_anthropic).to_string().contains("cache_control"));
        let fallback = sent(&to_openai).to_string();
        assert!(!fallback.contains("cache_control"), "{fallback}");
        assert!(fallback.contains("Repository map."));
    }
}
