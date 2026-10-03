//! `POST /v1/chat/completions` — verify JWT → resolve config → call provider →
//! render OpenAI-shaped response (non-streaming JSON or streaming SSE), with a
//! fire-and-forget usage row.
//!
//! The Axum entry point is a thin wrapper that builds a `PgRegistry` from the shared
//! pool; the real work lives in [`chat_core`], which takes a `&dyn RegistryStore` so
//! the whole path is testable without a database.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::Value;
use tracing::Instrument;

use futures::stream::BoxStream;

use crate::LlmRouterCtx;
use crate::auth::verify_agent_jwt;
use crate::error::GatewayError;
use crate::inbound::{ChatStreamRenderer, InboundFormat, inbound_for};
use crate::ir::{ChatChunk, Usage};
use crate::providers::{ProviderError, fallback};
use crate::resolver::{PgRegistry, RegistryStore, RequestHint, resolve};
use crate::routing::boundary::{TRACEPARENT_HEADER, parse_flow_id};
use crate::routing::{self, BoundarySignals, RouteInputs};
use crate::usage::{self, UsageRecord};

#[derive(Clone)]
pub(crate) struct RoutedRequest {
    pub agent_id: String,
    pub owner_id: String,
    pub resolved: crate::resolver::ResolvedConfig,
    pub flow_id: Option<String>,
    pub attribution_source: Option<routing::attribution::AttributionSource>,
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

pub(crate) fn authenticate_request(
    headers: &HeaderMap,
    cfg: &crate::config::GatewayConfig,
) -> Result<(String, String), GatewayError> {
    let authz = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    verify_agent_jwt(authz, cfg)
}

/// Axum handler for the OpenAI surface (`POST /v1/chat/completions`).
pub async fn chat_completions(
    State(ctx): State<LlmRouterCtx>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    let store = PgRegistry::new(ctx.db.clone());
    chat_core(&ctx, &store, &headers, body, InboundFormat::OpenAi, None).await
}

/// Axum handler for the Anthropic surface (`POST /v1/messages`). Same core; the inbound
/// format selects the Anthropic parser/renderer (P2.3).
pub async fn messages(
    State(ctx): State<LlmRouterCtx>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    let store = PgRegistry::new(ctx.db.clone());
    chat_core(&ctx, &store, &headers, body, InboundFormat::Anthropic, None).await
}

/// Axum handler for the Gemini surface (`POST /v1beta/models/{model}:{method}`). Gemini
/// signals streaming by the endpoint method (`:streamGenerateContent`), not a body field,
/// so we force the stream flag from the path (P2.4).
pub async fn gemini_generate(
    State(ctx): State<LlmRouterCtx>,
    Path(model_method): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    let stream = model_method
        .rsplit(':')
        .next()
        .map(|m| m.eq_ignore_ascii_case("streamGenerateContent"))
        .unwrap_or(false);
    let store = PgRegistry::new(ctx.db.clone());
    chat_core(
        &ctx,
        &store,
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
async fn chat_core(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    headers: &HeaderMap,
    body: Value,
    format: InboundFormat,
    force_stream: Option<bool>,
) -> Result<Response, GatewayError> {
    let (agent_id, owner_id) = authenticate_request(headers, &ctx.cfg)?;
    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id, %owner_id, ?format,
        has_traceparent = headers.get(TRACEPARENT_HEADER).is_some(),
        "chat_core: request received (JWT verified)"
    );

    // Read before `parse_chat` consumes the body: `FunctionDef` drops keys such as `strict`, and
    // the compact-tools eligibility check must still see them (`compact_tools::inspect_raw`).
    let raw_facts = if ctx.cfg.compact_tools_enabled && format == InboundFormat::OpenAi {
        crate::compact_tools::inspect_raw(&body)
    } else {
        crate::compact_tools::RawFacts::default()
    };
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
        query: routing::latest_user_query(&req.messages),
        turn_ordinal: routing::user_turn_ordinal(&req.messages),
        is_tool_continuation: routing::is_tool_continuation(&req.messages),
    };
    let routed =
        resolve_routed_request(ctx, store, headers, agent_id, owner_id, hint, signals).await?;
    let RoutedRequest {
        agent_id,
        owner_id,
        resolved,
        flow_id,
        attribution_source,
    } = routed;

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

    let recovery = ctx
        .cfg
        .compress_recovery_enabled
        .then_some(crate::compress::Recovery {
            min_bytes: ctx.cfg.compress_recovery_min_bytes,
        });

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

    // ── compact tool definitions seam ─────────────────────────────────────────────────────
    // After brevity, which reads the original `req.tools` for its tool-continuation carve-out,
    // and before `sent_bytes`, so the inserted catalog counts as text actually sent. Off by
    // default; `plan` records exactly why a request was left native. The fallback executor
    // clones `req` per attempt, so every attempt sees the same compacted request.
    let compact_plan = crate::compact_tools::plan(
        &req,
        &raw_facts,
        format,
        &ctx.cfg,
        resolved.compact_tools_enabled,
    );
    // The uncompacted request is kept only when a decode failure may re-send it natively.
    let mut native_retry_req = None;
    let compiled = match &compact_plan {
        crate::compact_tools::Plan::Apply(c) => {
            if ctx.cfg.compact_tools_native_retry && !req.is_streaming() {
                native_retry_req = Some(req.clone());
            }
            crate::compact_tools::apply(&mut req, c);
            Some(c)
        }
        crate::compact_tools::Plan::Bypass(_) => None,
    };
    if ctx.cfg.compact_tools_enabled {
        tracing::debug!(
            target: "nasiko::llm_router::compact_tools",
            %agent_id,
            applied = compiled.is_some(),
            bypass = ?compact_plan.bypass().map(|b| b.as_label()),
            "compact_tools: request decision"
        );
    }

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
        nasiko.usage.prompt_convention = "exclusive",
    );
    if compression.messages_touched > 0 {
        llm_span.record("nasiko.compress.bytes_in", compression.bytes_in);
        llm_span.record("nasiko.compress.bytes_out", compression.bytes_out);
        llm_span.record("nasiko.compress.elapsed_us", compression.elapsed_us);
    }

    let started = Instant::now();
    let platform_paid = resolved.platform_paid;

    if req.is_streaming() {
        let (stream, (provider, model)) =
            fallback::execute_chat_stream(&ctx.http, &ctx.cfg, &resolved, &req)
                .instrument(llm_span.clone())
                .await?;
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
            compress_bytes,
            request_bytes: Some(sent_bytes),
            // Streaming is always a bypass today; the row says so instead of staying silent.
            compact_tools_metadata: crate::compact_tools::to_metadata(&compact_plan, None),
            span: llm_span.clone(),
        });
    }

    // Non-streaming: run with ordered fallbacks; usage records the effective provider/model.
    let (mut resp, (provider, model)) =
        fallback::execute_chat(&ctx.http, &ctx.cfg, &resolved, &req)
            .instrument(llm_span.clone())
            .await?;
    let latency_ms = started.elapsed().as_millis() as i64;

    // Rebuild native tool calls from a compacted reply. Usage is logged below BEFORE a decode
    // failure is returned: the provider billed this call whatever the reply looked like.
    let compact_outcome = compiled.map(|c| crate::compact_tools::restore(&mut resp, c));
    let compact_tools_metadata =
        crate::compact_tools::to_metadata(&compact_plan, compact_outcome.as_ref());

    // Record effective model and token usage on the server-side gen_ai span.
    llm_span.record("gen_ai.response.model", model.as_str());
    record_span_usage(&llm_span, resp.usage.as_ref());

    // Identity fields are cloned: a native retry below logs a second row for the same request.
    usage::spawn_log_with(
        ctx.db.clone(),
        ctx.pricing.clone(),
        UsageRecord {
            owner_id: owner_id.clone(),
            agent_id: agent_id.clone(),
            operation_type: "direct_llm",
            provider,
            model,
            usage: resp.usage.clone(),
            cached_tokens: None,
            reasoning_tokens: None,
            latency_ms,
            streaming: false,
            finish_reason: resp.choices.first().and_then(|c| c.finish_reason.clone()),
            flow_id: flow_id.clone(),
            attribution_source,
            platform_paid,
            compress_metadata: compression.to_metadata(),
            brevity_metadata: brevity_metadata.clone(),
            compress_bytes,
            request_bytes: Some(sent_bytes),
        },
        compact_tools_metadata,
    );

    if let Some(Err(failure)) = compact_outcome {
        tracing::warn!(
            target: "nasiko::llm_router::compact_tools",
            kind = %failure.kind,
            native_retry = native_retry_req.is_some(),
            "compact_tools: reply could not be decoded into tool calls"
        );
        let (Some(native_req), Some(compiled)) = (native_retry_req, compiled) else {
            return Err(GatewayError::CompactToolDecode(failure.kind));
        };
        // One native re-send of the original request through the same fallback chain. Its reply
        // is checked by the same finalization against the same original schemas, so a native
        // reply with an invalid call is still refused; there is no second retry.
        let retry_started = Instant::now();
        let (mut resp, (provider, model)) =
            fallback::execute_chat(&ctx.http, &ctx.cfg, &resolved, &native_req)
                .instrument(llm_span.clone())
                .await?;
        let retry_outcome = crate::compact_tools::restore(&mut resp, compiled);
        llm_span.record("gen_ai.response.model", model.as_str());
        record_span_usage(&llm_span, resp.usage.as_ref());
        usage::spawn_log_with(
            ctx.db.clone(),
            ctx.pricing.clone(),
            UsageRecord {
                owner_id,
                agent_id,
                operation_type: "direct_llm",
                provider,
                model,
                usage: resp.usage.clone(),
                cached_tokens: None,
                reasoning_tokens: None,
                latency_ms: retry_started.elapsed().as_millis() as i64,
                streaming: false,
                finish_reason: resp.choices.first().and_then(|c| c.finish_reason.clone()),
                flow_id,
                attribution_source,
                platform_paid,
                compress_metadata: compression.to_metadata(),
                brevity_metadata,
                compress_bytes,
                request_bytes: Some(crate::brevity::estimated_bytes(&native_req)),
            },
            Some(crate::compact_tools::native_retry_metadata(&failure)),
        );
        if let Err(retry_failure) = retry_outcome {
            return Err(GatewayError::CompactToolDecode(retry_failure.kind));
        }
        return Ok(Json(inbound.render_chat_response(resp)).into_response());
    }

    Ok(Json(inbound.render_chat_response(resp)).into_response())
}

pub(crate) async fn resolve_routed_request(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    headers: &HeaderMap,
    agent_id: String,
    owner_id: String,
    hint: RequestHint<'_>,
    signals: RequestSignals,
) -> Result<RoutedRequest, GatewayError> {
    let mut resolved = resolve(store, &ctx.cache, &ctx.cfg, &agent_id, &owner_id, hint).await?;
    // Model routing: the resolver fixed provider/key/params; routing may override only model.
    //
    // A coding-agent CLI (Claude Code, Codex, OpenCode, Cursor) is never dispatched through
    // the orchestrator, so it never has a `flows` row — derive_boundary_signals's
    // traceparent lookup is a permanent dead end for it (see that fn's doc comment), which
    // otherwise pins every request to Level 4 (the agent's configured `llm_config`) and
    // makes the prompt classifier (Level 3) unreachable. Derive signals from the transcript
    // itself instead for these agents.
    let (boundary, flow_id, billed_user_id, attribution_source) = if resolved.is_coding_agent {
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
        )
    };
    let decision = routing::route_model(
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
            query: signals.query.as_deref(),
        },
    )
    .await;
    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id,
        source = ?decision.source, tier = ?decision.tier,
        provider = %resolved.provider,
        resolved_model = %resolved.model,
        decision_model = %decision.model,
        model_overridden = decision.model != resolved.model,
        "chat_core: model routing decision"
    );
    if decision.model != resolved.model {
        tracing::info!(
            target: "nasiko::llm_router::chat",
            %agent_id,
            from = %resolved.model, to = %decision.model,
            "chat_core: router overrode the resolved model"
        );
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
    })
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
    /// Savings-ledger inputs, threaded through to the `Drop` write for the same reason
    /// `compress_metadata` is: on this path the `UsageRecord` is only built once the stream ends.
    compress_bytes: Option<(usize, usize)>,
    request_bytes: Option<usize>,
    compact_tools_metadata: Option<serde_json::Value>,
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
        compress_bytes,
        request_bytes,
        compact_tools_metadata,
    } = args;
    let state = Arc::new(Mutex::new(StreamState::default()));
    let guard = UsageGuard {
        db: ctx.db.clone(),
        pricing: ctx.pricing.clone(),
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
        compress_bytes,
        request_bytes,
        compact_tools_metadata,
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
                    }
                    for frame in renderer.render(chunk) {
                        yield Ok::<String, std::io::Error>(frame);
                    }
                }
                Err(e) => {
                    // Mid-stream provider failure: log and end the stream cleanly.
                    tracing::error!(error = %e, "provider stream error");
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
}

/// Writes the streaming usage row when dropped (stream completion or client disconnect).
struct UsageGuard {
    db: sqlx::PgPool,
    pricing: Arc<nasiko_pricing::PricingEngine>,
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
    /// `Copy`, so unlike the two above these are read rather than taken.
    compress_bytes: Option<(usize, usize)>,
    request_bytes: Option<usize>,
    /// Taken in `drop`, like the other metadata blocks.
    compact_tools_metadata: Option<serde_json::Value>,
}

impl Drop for UsageGuard {
    fn drop(&mut self) {
        let compress_metadata = self.compress_metadata.take();
        let brevity_metadata = self.brevity_metadata.take();
        let compact_tools_metadata = self.compact_tools_metadata.take();
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        record_span_usage(&self.span, st.usage.as_ref());
        usage::spawn_log_with(
            self.db.clone(),
            self.pricing.clone(),
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
                compress_bytes: self.compress_bytes,
                request_bytes: self.request_bytes,
            },
            compact_tools_metadata,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;
    use crate::resolver::{AgentConfigResult, ConfigCache, LLMConfig};
    use async_trait::async_trait;
    use jsonwebtoken::Algorithm;
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
                })),
                flow_id: None,
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
            };
            drop(guard);
        });
        let fields = captured.0.lock().unwrap();
        assert_eq!(fields["gen_ai.usage.input_tokens"], 800);
        assert_eq!(fields["gen_ai.usage.output_tokens"], 50);
        assert_eq!(fields["gen_ai.usage.cache_read_input_tokens"], 200);
    }

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SECRET: &str = "gateway-secret";

    struct Store {
        config: Option<LLMConfig>,
        is_coding_agent: bool,
        compress_enabled: bool,
        compact_tools_enabled: bool,
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
                compress_enabled: self.compress_enabled,
                compact_tools_enabled: self.compact_tools_enabled,
            }))
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
                agent_is_participant: true,
            }))
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
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
            pricing: Arc::new(nasiko_pricing::PricingEngine::new(
                PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            )),
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
            compress_enabled,
            compact_tools_enabled: false,
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
            compress_enabled: false,
            compact_tools_enabled: false,
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
            compress_enabled: false,
            compact_tools_enabled: false,
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
            compress_enabled: false,
            compact_tools_enabled: false,
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
            compress_enabled: false,
            compact_tools_enabled: false,
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

    #[tokio::test]
    async fn missing_auth_is_401_before_any_provider_call() {
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: None,
            is_coding_agent: false,
            compress_enabled: false,
            compact_tools_enabled: false,
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

    #[test]
    fn boundary_signals_in_flow_when_attributed() {
        let attribution = routing::attribution::FlowAttribution {
            flow_id: "f1".into(),
            user_id: None,
            context_id: Some("ses_1".into()),
            mode: routing::Mode::FreeFlowing,
            source: routing::attribution::AttributionSource::Traceparent,
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
            compress_enabled: false,
            compact_tools_enabled: false,
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
            compact_tools_enabled: false,
            // A configured model that is NOT one of openai's seeded tier models
            // (gpt-5.5 / gpt-5.4 / gpt-4o-mini) — if the classifier never fires, the
            // resolved model will be exactly this. If it does fire, it will be one of the
            // seeded tier models instead.
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
            }),
            is_coding_agent: true,
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
        )
        .await
        .unwrap();
        assert!(
            ["gpt-5.5", "gpt-5.4", "gpt-4o-mini"].contains(&routed.resolved.model.as_str()),
            "expected a classifier-selected tier model, got {}",
            routed.resolved.model
        );
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
            }),
            is_coding_agent: false,
            compress_enabled: false,
            compact_tools_enabled: false,
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
        )
        .await;
        let Err(error) = result else {
            panic!("expected strict attribution to reject the request");
        };
        assert!(matches!(error, GatewayError::Forbidden(_)));
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
            }),
            is_coding_agent: false,
            compress_enabled: false,
            compact_tools_enabled: false,
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

    // ── compact tool definitions: the flag reaches the wire, the reply comes back native ──────
    //
    // `compact_tools::tests` covers plan/apply/finalize on IR values. These prove the wiring:
    // what the provider is actually sent with the flag off and on, every bypass that is reachable
    // through `chat_core`, that a reply the decoder refuses becomes a 502 with no calls when the
    // native retry is off, and that with it on the original request is re-sent exactly once.

    /// `ctx_with` plus the compact-tools flag. Brevity's holdout is zeroed so the directive is
    /// deterministic when a test turns the agent's optimization switch on. The native retry is
    /// off here so each exchange is exactly one provider call; the retry tests turn it on.
    fn compact_ctx(base: String, enabled: bool) -> LlmRouterCtx {
        compact_ctx_with_retry(base, enabled, false)
    }

    fn compact_ctx_with_retry(base: String, enabled: bool, native_retry: bool) -> LlmRouterCtx {
        let mut ctx = ctx_with(base);
        let mut cfg = (*ctx.cfg).clone();
        cfg.compact_tools_enabled = enabled;
        cfg.compact_tools_native_retry = native_retry;
        cfg.brevity_holdout_pct = 0;
        ctx.cfg = Arc::new(cfg);
        ctx
    }

    fn weather_tool() -> Value {
        json!({"type": "function", "function": {
            "name": "get_weather",
            "description": "Current weather for a city",
            "parameters": {"type": "object", "properties": {
                "city": {"type": "string"},
                "unit": {"type": "string", "enum": ["c", "f"]}
            }, "required": ["city"]}
        }})
    }

    fn forecast_tool() -> Value {
        json!({"type": "function", "function": {
            "name": "get_forecast",
            "description": "Five-day forecast for a city",
            "parameters": {"type": "object", "properties": {
                "city": {"type": "string", "description": "City name"},
                "days": {"type": "integer", "minimum": 1, "maximum": 5, "description": "Number of days"}
            }, "required": ["city"]}
        }})
    }

    /// Two tools: enough for the compact message to be smaller than the tools JSON. A single
    /// small tool is correctly left native (`no_byte_saving`), which the bypass test covers.
    fn tools_request() -> Value {
        json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Weather in Paris?"}],
            "tools": [weather_tool(), forecast_tool()],
            "tool_choice": "auto"
        })
    }

    /// An upstream reply. A `Null` content is left out rather than written as `null`: the IR's
    /// `Message.content` is an `Option` that skips serialization when absent, so that is the only
    /// form the router can echo back byte-for-byte (today's behaviour, unrelated to this layer).
    fn reply(content: Value, finish: Value, tool_calls: Option<Value>) -> Value {
        let mut message = json!({"role": "assistant"});
        if !content.is_null() {
            message["content"] = content;
        }
        if let Some(tc) = tool_calls {
            message["tool_calls"] = tc;
        }
        json!({
            "id": "chatcmpl-c", "object": "chat.completion", "created": 1, "model": "gpt-4o",
            "choices": [{"index": 0, "message": message, "finish_reason": finish}],
            "usage": {"prompt_tokens": 40, "completion_tokens": 9, "total_tokens": 49}
        })
    }

    fn call_reply(text: &str) -> Value {
        reply(json!(text), json!("stop"), None)
    }

    /// Run one request through `chat_core` against a provider mock that replies with `upstream`.
    /// Returns the exact body the provider received and the handler's result.
    async fn exchange(
        enabled: bool,
        store: &dyn RegistryStore,
        format: InboundFormat,
        request: Value,
        upstream: Value,
    ) -> (Value, Result<Value, GatewayError>) {
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
                upstream.to_string().into_bytes()
            })
            .create_async()
            .await;
        let ctx = compact_ctx(server.url(), enabled);
        let result = chat_core(&ctx, store, &auth_headers(&token()), request, format, None).await;
        mock.assert_async().await;
        let sent: Value =
            serde_json::from_str(&seen.lock().unwrap_or_else(|e| e.into_inner())).unwrap();
        let result = match result {
            Ok(resp) => Ok(serde_json::from_str(&body_string(resp).await).unwrap()),
            Err(e) => Err(e),
        };
        (sent, result)
    }

    /// An agent that has opted in: `agents.compact_tools_enabled` is on.
    fn plain_store() -> Store {
        Store {
            config: None,
            is_coding_agent: false,
            compress_enabled: false,
            compact_tools_enabled: true,
        }
    }

    fn decode_failure(result: Result<Value, GatewayError>) -> String {
        match result {
            Err(GatewayError::CompactToolDecode(kind)) => kind,
            other => panic!("expected a decode failure, got {other:?}"),
        }
    }

    /// `exchange` with the native retry on and a provider that answers the compacted request
    /// (no `tools` on the wire) with `compact_reply` and the native one with `native_reply`.
    /// Returns every body the provider received, in order, and the handler's result.
    async fn exchange_with_retry(
        request: Value,
        compact_reply: Value,
        native_reply: Value,
    ) -> (Vec<Value>, Result<Value, GatewayError>) {
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let capture = Arc::clone(&seen);
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let body = request.body().map(Vec::as_slice).unwrap_or_default();
                let sent: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                let answer = if sent.get("tools").is_some() {
                    &native_reply
                } else {
                    &compact_reply
                };
                let out = answer.to_string().into_bytes();
                capture.lock().unwrap_or_else(|e| e.into_inner()).push(sent);
                out
            })
            .expect_at_least(1)
            .create_async()
            .await;
        let ctx = compact_ctx_with_retry(server.url(), true, true);
        let result = chat_core(
            &ctx,
            &plain_store(),
            &auth_headers(&token()),
            request,
            InboundFormat::OpenAi,
            None,
        )
        .await;
        mock.assert_async().await;
        let bodies = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let result = match result {
            Ok(resp) => Ok(serde_json::from_str(&body_string(resp).await).unwrap()),
            Err(e) => Err(e),
        };
        (bodies, result)
    }

    #[tokio::test]
    async fn a_refused_compact_reply_is_retried_once_with_the_original_request() {
        let native_calls = json!([{"id": "call_up1", "type": "function",
            "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]);
        let (bodies, result) = exchange_with_retry(
            tools_request(),
            call_reply("<<call get_weather {\"city\":\"Paris\",\"unit\":\"kelvin\"}>>"),
            reply(Value::Null, json!("tool_calls"), Some(native_calls)),
        )
        .await;
        assert_eq!(
            bodies.len(),
            2,
            "one compacted attempt, one native retry: {bodies:?}"
        );
        assert!(
            bodies[0].get("tools").is_none(),
            "first attempt is compacted"
        );
        // The retry is the original request, byte for byte what the flag-off path would send.
        let mut expected = tools_request();
        expected["stream"] = json!(false);
        assert_eq!(bodies[1], expected);
        let body = result.expect("the native retry's reply is released");
        let calls = &body["choices"][0]["message"]["tool_calls"];
        assert_eq!(calls[0]["function"]["name"], "get_weather");
        assert_eq!(calls[0]["function"]["arguments"], "{\"city\":\"Paris\"}");
    }

    #[tokio::test]
    async fn a_native_retry_reply_with_an_invalid_call_is_still_refused() {
        // The retry does not relax validation: the native reply is checked against the same
        // original schemas, and there is no second retry.
        let bad_native = json!([{"id": "call_up1", "type": "function",
            "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\",\"unit\":\"kelvin\"}"}}]);
        let (bodies, result) = exchange_with_retry(
            tools_request(),
            call_reply("<<call delete_everything {}>>"),
            reply(Value::Null, json!("tool_calls"), Some(bad_native)),
        )
        .await;
        assert_eq!(bodies.len(), 2, "{bodies:?}");
        assert!(matches!(
            result,
            Err(GatewayError::CompactToolDecode(ref k)) if k == "invalid_arguments"
        ));
    }

    #[tokio::test]
    async fn a_decodable_compact_reply_is_not_retried() {
        let (bodies, result) = exchange_with_retry(
            tools_request(),
            call_reply("<<call get_weather {\"city\":\"Paris\"}>>"),
            reply(json!("unused"), json!("stop"), None),
        )
        .await;
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        let body = result.unwrap();
        assert_eq!(
            body["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
    }

    #[tokio::test]
    async fn flag_off_sends_the_tools_request_verbatim_and_returns_the_reply_verbatim() {
        let native_calls = json!([{"id": "call_up1", "type": "function",
            "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]);
        let upstream = reply(Value::Null, json!("tool_calls"), Some(native_calls));
        let (sent, result) = exchange(
            false,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            upstream.clone(),
        )
        .await;
        // Exactly what the OpenAI provider sends today: the request plus the resolved model and
        // `stream: false`. Nothing added, nothing removed.
        let mut expected = tools_request();
        expected["stream"] = json!(false);
        assert_eq!(sent, expected);
        assert_eq!(result.unwrap(), upstream);
    }

    #[tokio::test]
    async fn flag_on_compacts_the_request_and_rebuilds_native_tool_calls() {
        let (sent, result) = exchange(
            true,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            call_reply("<<call get_weather {\"city\":\"Paris\",\"unit\":\"c\"}>>"),
        )
        .await;
        assert!(
            sent.get("tools").is_none(),
            "tools reached the provider: {sent}"
        );
        assert!(sent.get("tool_choice").is_none());
        let messages = sent["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        let catalog = messages[0]["content"].as_str().unwrap();
        assert!(catalog.starts_with(nasiko_tool_compact::HEADER));
        assert!(
            catalog.contains("\nget_weather(city:str, unit?:c|f) - Current weather for a city\n")
        );
        assert!(catalog.ends_with(nasiko_tool_compact::INSTRUCTIONS));
        assert_eq!(
            messages[1],
            json!({"role": "user", "content": "Weather in Paris?"})
        );
        assert_eq!(sent["model"], "gpt-4o");

        let v = result.unwrap();
        let choice = &v["choices"][0];
        assert_eq!(choice["finish_reason"], "tool_calls");
        assert_eq!(choice["message"]["content"], Value::Null);
        let call = &choice["message"]["tool_calls"][0];
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "get_weather");
        assert_eq!(
            call["function"]["arguments"],
            json!("{\"city\":\"Paris\",\"unit\":\"c\"}")
        );
        let id = call["id"].as_str().unwrap();
        assert!(id.starts_with("call_") && id.len() == 29, "{id}");
        assert!(id[5..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            v["usage"],
            json!({"prompt_tokens": 40, "completion_tokens": 9, "total_tokens": 49})
        );
        assert_eq!(v["model"], "gpt-4o");
        assert_eq!(v["id"], "chatcmpl-c");
        assert!(
            !v.to_string().contains("<<call"),
            "compact syntax leaked: {v}"
        );
    }

    #[tokio::test]
    async fn surrounding_prose_is_kept_exactly_and_plain_answers_pass_through() {
        let (_, result) = exchange(
            true,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            call_reply("Let me check.\n<<call get_weather {\"city\":\"Paris\"}>>\n  Done. "),
        )
        .await;
        let v = result.unwrap();
        assert_eq!(
            v["choices"][0]["message"]["content"],
            "Let me check.\n\n  Done. "
        );

        let upstream = call_reply("It is sunny in Paris.");
        let (_, result) = exchange(
            true,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            upstream.clone(),
        )
        .await;
        assert_eq!(result.unwrap(), upstream);
    }

    #[tokio::test]
    async fn every_reachable_bypass_leaves_the_native_request_on_the_wire() {
        let with = |mutate: fn(&mut Value)| {
            let mut r = tools_request();
            mutate(&mut r);
            r
        };
        let cases: Vec<(&str, Value)> = vec![
            (
                "tool_choice none",
                with(|r| r["tool_choice"] = json!("none")),
            ),
            (
                "tool_choice required",
                with(|r| r["tool_choice"] = json!("required")),
            ),
            (
                "tool_choice named",
                with(|r| {
                    r["tool_choice"] =
                        json!({"type": "function", "function": {"name": "get_weather"}})
                }),
            ),
            (
                "response_format json",
                with(|r| r["response_format"] = json!({"type": "json_object"})),
            ),
            ("n > 1", with(|r| r["n"] = json!(2))),
            (
                "parallel_tool_calls false",
                with(|r| r["parallel_tool_calls"] = json!(false)),
            ),
            (
                "strict in the raw body",
                with(|r| r["tools"][0]["function"]["strict"] = json!(true)),
            ),
            (
                "unknown tool wrapper key",
                with(|r| r["tools"][0]["cache_control"] = json!({"type": "ephemeral"})),
            ),
            (
                "non-function tool kind",
                with(|r| r["tools"][0]["type"] = json!("custom")),
            ),
            (
                "tool history",
                with(|r| {
                    r["messages"] = json!([
                        {"role": "user", "content": "Weather?"},
                        {"role": "assistant", "tool_calls": [{"id": "c1", "type": "function",
                            "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]},
                        {"role": "tool", "tool_call_id": "c1", "content": "sunny"},
                        {"role": "user", "content": "And Rome?"}
                    ])
                }),
            ),
            (
                "unsupported schema",
                with(|r| {
                    r["tools"][0]["function"]["parameters"]["properties"]["city"]["pattern"] =
                        json!("^[A-Z]")
                }),
            ),
            (
                "single tiny tool: compact message would not be smaller",
                with(|r| {
                    r["tools"] = json!([{"type": "function", "function": {
                        "name": "ping", "parameters": {"type": "object"}}}])
                }),
            ),
        ];
        let upstream = call_reply("plain text, no call");
        for (label, request) in cases {
            let (sent, result) = exchange(
                true,
                &plain_store(),
                InboundFormat::OpenAi,
                request.clone(),
                upstream.clone(),
            )
            .await;
            assert!(
                sent["tools"].is_array(),
                "{label}: tools were stripped: {sent}"
            );
            assert_eq!(
                sent["messages"], request["messages"],
                "{label}: messages changed"
            );
            assert!(
                !sent.to_string().contains("<<call"),
                "{label}: catalog injected on a bypassed request"
            );
            assert_eq!(result.unwrap(), upstream, "{label}: reply changed");
        }
    }

    #[tokio::test]
    async fn an_agent_that_has_not_opted_in_keeps_the_native_request() {
        let store = Store {
            config: None,
            is_coding_agent: false,
            compress_enabled: false,
            compact_tools_enabled: false,
        };
        let upstream = call_reply("plain text");
        let (sent, result) = exchange(
            true,
            &store,
            InboundFormat::OpenAi,
            tools_request(),
            upstream.clone(),
        )
        .await;
        assert!(sent["tools"].is_array(), "{sent}");
        assert_eq!(sent["messages"], tools_request()["messages"]);
        assert_eq!(result.unwrap(), upstream);
    }

    #[tokio::test]
    async fn streaming_and_non_openai_inbound_bypass_too() {
        // Streaming: the provider still receives `tools` and the SSE path is untouched.
        let mut server = mockito::Server::new_async().await;
        let sse = "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({"stream": true, "tools": [weather_tool(), forecast_tool()]}),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;
        let ctx = compact_ctx(server.url(), true);
        let mut request = tools_request();
        request["stream"] = json!(true);
        let resp = chat_core(
            &ctx,
            &plain_store(),
            &auth_headers(&token()),
            request,
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();
        assert!(body_string(resp).await.contains("\"content\":\"hi\""));
        mock.assert_async().await;

        // Anthropic inbound with tools: translated natively, never compacted.
        let anthropic = json!({
            "model": "claude", "max_tokens": 100,
            "messages": [{"role": "user", "content": "Weather in Paris?"}],
            "tools": [{"name": "get_weather", "description": "d", "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}}]
        });
        let store = Store {
            config: Some(openai_config()),
            is_coding_agent: false,
            compress_enabled: false,
            compact_tools_enabled: false,
        };
        let (sent, result) = exchange(
            true,
            &store,
            InboundFormat::Anthropic,
            anthropic,
            call_reply("text"),
        )
        .await;
        assert!(sent["tools"].is_array(), "{sent}");
        assert_eq!(result.unwrap()["type"], "message");
    }

    #[tokio::test]
    async fn a_reply_the_decoder_refuses_is_a_502_with_only_the_kind_in_the_body() {
        let secret_text = "<<call delete_everything {\"target\":\"PRODUCTION-DB\"}>>";
        let (_, result) = exchange(
            true,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            call_reply(secret_text),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(v) => panic!("unknown tool was released: {v}"),
        };
        assert!(matches!(err, GatewayError::CompactToolDecode(ref k) if k == "unknown_tool"));
        let resp = err.into_response();
        assert_eq!(resp.status(), 502);
        let body = body_string(resp).await;
        assert_eq!(
            body,
            json!({"detail": "compact tool call decoding failed: unknown_tool"}).to_string()
        );
        assert!(!body.contains("PRODUCTION-DB"));
    }

    #[tokio::test]
    async fn invalid_incomplete_and_mixed_replies_release_no_calls() {
        let cases: Vec<(&str, Value, &str)> = vec![
            (
                "valid then invalid",
                call_reply(
                    "<<call get_weather {\"city\":\"Paris\"}>>\n<<call get_weather {\"unit\":\"c\"}>>",
                ),
                "invalid_arguments",
            ),
            (
                "bad enum",
                call_reply("<<call get_weather {\"city\":\"Paris\",\"unit\":\"kelvin\"}>>"),
                "invalid_arguments",
            ),
            (
                "truncated by length",
                reply(
                    json!("<<call get_weather {\"city\":\"Par"),
                    json!("length"),
                    None,
                ),
                "incomplete_completion",
            ),
            (
                "complete call but finish_reason length",
                reply(
                    json!("<<call get_weather {\"city\":\"Paris\"}>>"),
                    json!("length"),
                    None,
                ),
                "incomplete_completion",
            ),
            (
                "marker with no finish reason",
                reply(
                    json!("<<call get_weather {\"city\":\"Paris\"}>>"),
                    Value::Null,
                    None,
                ),
                "incomplete_completion",
            ),
            (
                "native calls with finish_reason stop",
                reply(
                    Value::Null,
                    json!("stop"),
                    Some(json!([{"id": "c", "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}])),
                ),
                "incomplete_completion",
            ),
            (
                "native calls with invalid arguments",
                reply(
                    Value::Null,
                    json!("tool_calls"),
                    Some(json!([{"id": "c", "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":7}"}}])),
                ),
                "invalid_arguments",
            ),
            (
                "mixed native and textual",
                reply(
                    json!("<<call get_weather {\"city\":\"Paris\"}>>"),
                    json!("tool_calls"),
                    Some(json!([{"id": "c", "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}])),
                ),
                "mixed_call_representations",
            ),
            (
                "malformed json",
                call_reply("<<call get_weather {city: Paris}>>"),
                "malformed_call",
            ),
        ];
        for (label, upstream, kind) in cases {
            let (_, result) = exchange(
                true,
                &plain_store(),
                InboundFormat::OpenAi,
                tools_request(),
                upstream,
            )
            .await;
            assert_eq!(decode_failure(result), kind, "{label}");
        }

        // Two choices on a request that asked for one.
        let mut two = call_reply("<<call get_weather {\"city\":\"Paris\"}>>");
        let first = two["choices"][0].clone();
        two["choices"].as_array_mut().unwrap().push(first);
        let (_, result) = exchange(
            true,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            two,
        )
        .await;
        assert_eq!(decode_failure(result), "unexpected_choice_count");
    }

    #[tokio::test]
    async fn filtered_or_truncated_prose_without_executable_output_passes_through() {
        for finish in ["content_filter", "length"] {
            let upstream = reply(json!("I cannot help with"), json!(finish), None);
            let (_, result) = exchange(
                true,
                &plain_store(),
                InboundFormat::OpenAi,
                tools_request(),
                upstream.clone(),
            )
            .await;
            assert_eq!(result.unwrap(), upstream, "{finish}");
        }
    }

    #[tokio::test]
    async fn an_oversized_native_tool_call_is_a_502_and_releases_nothing() {
        let big = format!(
            "{{\"city\":\"{}\"}}",
            "x".repeat(nasiko_tool_compact::limits::MAX_ARGS_BYTES)
        );
        let upstream = reply(
            Value::Null,
            json!("tool_calls"),
            Some(json!([
                {"id": "c1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}},
                {"id": "c2", "type": "function", "function": {"name": "get_weather", "arguments": big}}
            ])),
        );
        let (_, result) = exchange(
            true,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            upstream,
        )
        .await;
        assert_eq!(decode_failure(result), "limit_exceeded");
    }

    #[tokio::test]
    async fn valid_native_tool_calls_from_the_provider_are_validated_and_kept() {
        let upstream = reply(
            Value::Null,
            json!("tool_calls"),
            Some(json!([{"id": "call_upstream", "type": "function",
                "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}])),
        );
        let (_, result) = exchange(
            true,
            &plain_store(),
            InboundFormat::OpenAi,
            tools_request(),
            upstream.clone(),
        )
        .await;
        assert_eq!(result.unwrap(), upstream);
    }

    #[tokio::test]
    async fn brevity_and_the_compact_catalog_coexist_without_tools_on_the_wire() {
        let store = Store {
            config: None,
            is_coding_agent: false,
            compress_enabled: true,
            compact_tools_enabled: true,
        };
        let mut request = tools_request();
        request["messages"] = json!([
            {"role": "system", "content": "You are terse."},
            {"role": "user", "content": "Weather in Paris?"}
        ]);
        let (sent, result) = exchange(
            true,
            &store,
            InboundFormat::OpenAi,
            request,
            call_reply("<<call get_weather {\"city\":\"Paris\"}>>"),
        )
        .await;
        assert!(sent.get("tools").is_none());
        let messages = sent["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4, "{messages:?}");
        assert_eq!(messages[0]["content"], "You are terse.");
        assert!(
            messages[1]["content"]
                .as_str()
                .unwrap()
                .contains("get_weather(")
        );
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[3]["role"], "system");
        assert_eq!(messages[3]["content"], crate::brevity::DIRECTIVE);
        assert_eq!(result.unwrap()["choices"][0]["finish_reason"], "tool_calls");
    }

    #[tokio::test]
    async fn fallback_attempts_see_the_compacted_request_too() {
        let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
        let mut server = mockito::Server::new_async().await;
        let primary_bodies = Arc::clone(&bodies);
        let primary = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({"model": "gpt-4o"})))
            .with_status(500)
            .with_body_from_request(move |request| {
                let body = request.body().map(Vec::as_slice).unwrap_or_default();
                primary_bodies
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(body).unwrap());
                b"{\"error\":\"boom\"}".to_vec()
            })
            .create_async()
            .await;
        let fallback_bodies = Arc::clone(&bodies);
        let fallback = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model": "gpt-4o-mini"}),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let body = request.body().map(Vec::as_slice).unwrap_or_default();
                fallback_bodies
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice(body).unwrap());
                call_reply("<<call get_weather {\"city\":\"Paris\"}>>")
                    .to_string()
                    .into_bytes()
            })
            .create_async()
            .await;
        let store = Store {
            config: Some(LLMConfig {
                fallback_models: vec!["gpt-4o-mini".into()],
                model: Some("gpt-4o".into()),
                ..openai_config()
            }),
            is_coding_agent: false,
            compress_enabled: false,
            compact_tools_enabled: true,
        };
        let ctx = compact_ctx(server.url(), true);
        let resp = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            tools_request(),
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();
        primary.assert_async().await;
        fallback.assert_async().await;
        let v: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
        assert_eq!(v["model"], "gpt-4o-mini");
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        for body in bodies.iter() {
            assert!(body.get("tools").is_none(), "{body}");
            assert!(
                body["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("<<call")
            );
        }
    }

    /// A store whose agent is configured for an admin-registered OpenAI-compatible endpoint.
    struct CustomStore {
        base_url: String,
    }
    #[async_trait]
    impl RegistryStore for CustomStore {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(Some(AgentConfigResult {
                config: Some(LLMConfig {
                    provider: "mygateway".into(),
                    model: Some("local-model".into()),
                    ..openai_config()
                }),
                agent_pinned_model: None,
                is_coding_agent: false,
                compress_enabled: false,
                compact_tools_enabled: true,
            }))
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
            Ok(Some(routing::attribution::LiveFlow {
                user_id: None,
                context_id: Some("ses_test".into()),
                mode: None,
                agent_is_participant: true,
            }))
        }
        async fn fetch_custom_provider(
            &self,
            label: &str,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            assert_eq!(label, "mygateway");
            Ok(Some(crate::resolver::CustomProvider {
                base_url: format!("{}/v1", self.base_url),
                dialect: crate::providers::dialect::ProviderDialect::OpenAi,
                api_key: "gw-key".into(),
                default_model: None,
            }))
        }
    }

    #[tokio::test]
    async fn custom_openai_compatible_providers_take_the_same_path() {
        let seen = Arc::new(Mutex::new(String::new()));
        let capture = Arc::clone(&seen);
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer gw-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                let body = request.body().map(Vec::as_slice).unwrap_or_default();
                *capture.lock().unwrap() = String::from_utf8_lossy(body).into_owned();
                call_reply("<<call get_weather {\"city\":\"Paris\"}>>")
                    .to_string()
                    .into_bytes()
            })
            .create_async()
            .await;
        let store = CustomStore {
            base_url: server.url(),
        };
        let ctx = compact_ctx("http://unused.invalid".into(), true);
        let resp = chat_core(
            &ctx,
            &store,
            &auth_headers(&token()),
            tools_request(),
            InboundFormat::OpenAi,
            None,
        )
        .await
        .unwrap();
        mock.assert_async().await;
        let sent: Value = serde_json::from_str(&seen.lock().unwrap()).unwrap();
        assert!(sent.get("tools").is_none());
        assert_eq!(sent["model"], "local-model");
        let v: Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
    }
}
