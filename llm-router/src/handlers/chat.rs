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
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::Json;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::Value;

use futures::stream::BoxStream;

use crate::LlmRouterCtx;
use crate::auth::verify_agent_jwt;
use crate::error::GatewayError;
use crate::inbound::{ChatStreamRenderer, InboundFormat, inbound_for};
use crate::ir::{ChatChunk, Usage};
use crate::providers::{ProviderError, fallback};
use crate::resolver::{PgRegistry, RegistryStore, RequestHint, resolve};
use crate::routing::boundary::{TRACEPARENT_HEADER, parse_flow_id};
use crate::routing::{self, BoundarySignals, Mode, RouteInputs};
use crate::usage::{self, UsageRecord};

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
    chat_core(&ctx, &store, &headers, body, InboundFormat::Gemini, Some(stream)).await
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
    let authz = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    let (agent_id, owner_id) = verify_agent_jwt(authz, &ctx.cfg)?;
    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id, %owner_id, ?format,
        has_traceparent = headers.get(TRACEPARENT_HEADER).is_some(),
        "chat_core: request received (JWT verified)"
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
    let mut resolved = resolve(store, &ctx.cache, &ctx.cfg, &agent_id, &owner_id, hint).await?;

    // Model routing: the resolver fixed the provider/key/params; the router may override
    // the *model* at a conversation boundary (else it stays the resolved model). Signals are
    // derived at the gateway from the agent-forwarded traceparent; no trace context ⇒ inert,
    // so the resolved model is used (behaviour identical to before this layer).
    let signals = derive_boundary_signals(headers, &ctx.db).await;
    let query = routing::latest_user_query(&req.messages);
    let decision = routing::route_model(
        ctx.router_cache.as_ref(),
        ctx.tier_registry.as_ref(),
        &RouteInputs {
            agent_id: &agent_id,
            provider: &resolved.provider,
            fallback_model: &resolved.model,
            has_llm_config: resolved.has_llm_config,
            pinned_model: resolved.pinned_model.as_deref(),
            signals: &signals,
            query: query.as_deref(),
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
    tracing::info!(
        target: "nasiko::llm_router::chat",
        %agent_id,
        litellm_model = %resolved.litellm_model,
        provider = %resolved.provider,
        fallback_models = ?resolved.fallback_models,
        streaming = req.is_streaming(),
        "chat_core: final model selected — dispatching to provider"
    );

    let started = Instant::now();

    if req.is_streaming() {
        let (stream, (provider, model)) =
            fallback::execute_chat_stream(&ctx.http, &ctx.cfg, &resolved, &req).await?;
        let renderer = inbound.chat_stream_renderer();
        return stream_chat(ctx, renderer, stream, provider, model, agent_id, owner_id, started);
    }

    // Non-streaming: run with ordered fallbacks; usage records the effective provider/model.
    let (resp, (provider, model)) =
        fallback::execute_chat(&ctx.http, &ctx.cfg, &resolved, &req).await?;
    let latency_ms = started.elapsed().as_millis() as i64;

    usage::spawn_log(
        ctx.db.clone(),
        UsageRecord {
            owner_id,
            agent_id,
            operation_type: "direct_llm",
            provider,
            model,
            usage: resp.usage.clone(),
            latency_ms,
            streaming: false,
            finish_reason: resp.choices.first().and_then(|c| c.finish_reason.clone()),
        },
    );

    Ok(Json(inbound.render_chat_response(resp)).into_response())
}

/// Derive the model-routing [`BoundarySignals`] for this request (S5).
///
/// The opaque agent sets nothing: the gateway reads the `traceparent` the agent forwards,
/// maps its trace id to a `flows` row (the conversation), and builds the signals from that
/// trusted state. The flow lookup does double duty — it both confirms this is a genuine
/// platform conversation (not an arbitrary trace) and reads the conversation `mode`.
///
/// Any of: no `traceparent`, an unparseable one, an unknown flow, or a DB error ⇒
/// [`BoundarySignals::inert`] — the router doesn't fire and the resolved model is used.
async fn derive_boundary_signals(headers: &HeaderMap, db: &sqlx::PgPool) -> BoundarySignals {
    let Some(flow_id) = headers
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_flow_id)
    else {
        tracing::info!(
            target: "nasiko::llm_router::boundary",
            conv_id = "None",
            "derive_boundary_signals: no/unparseable traceparent → INERT (router will not fire; resolved model used)"
        );
        return BoundarySignals::inert();
    };
    tracing::debug!(
        target: "nasiko::llm_router::boundary",
        %flow_id, "derive_boundary_signals: parsed flow_id from traceparent; looking up flow in DB"
    );

    let row: Result<Option<(Option<String>,)>, sqlx::Error> =
        sqlx::query_as("SELECT metadata->>'mode' FROM flows WHERE flow_id = $1")
            .bind(&flow_id)
            .fetch_optional(db)
            .await;
    match row {
        Ok(Some((mode,))) => {
            let mode = mode.as_deref().map(Mode::from_label).unwrap_or(Mode::FreeFlowing);
            let signals = BoundarySignals::in_flow(flow_id.clone(), mode);
            tracing::info!(
                target: "nasiko::llm_router::boundary",
                %flow_id, mode = ?mode, phase = ?signals.phase,
                is_fireable_boundary = signals.is_fireable_boundary(),
                "derive_boundary_signals: known flow → IN-FLOW signals (router may re-select the model at this boundary)"
            );
            signals
        }
        Ok(None) => {
            tracing::info!(
                target: "nasiko::llm_router::boundary",
                flow_id_lookup = %flow_id,
                conv_id = "None",
                "derive_boundary_signals: forwarded trace id is not a known flow → INERT (router will not fire; check the orchestrator's `nasiko::flow` flow_id — a mismatch means the agent didn't propagate the trace)"
            );
            BoundarySignals::inert() // trace id isn't a known flow → don't fire
        }
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::boundary",
                error = %e, %flow_id, "derive_boundary_signals: flow lookup failed; router INERT for request"
            );
            BoundarySignals::inert()
        }
    }
}

/// Stream provider chunks back as OpenAI SSE: `data: <chunk>\n\n` … `data: [DONE]\n\n`.
/// Usage is captured as chunks flow and written when the stream ends — including on
/// client disconnect — via a `Drop` guard. `provider`/`model` are the effective
/// (possibly fallback) values chosen by the executor.
#[allow(clippy::too_many_arguments)]
fn stream_chat(
    ctx: &LlmRouterCtx,
    renderer: Box<dyn ChatStreamRenderer>,
    provider_stream: BoxStream<'static, Result<ChatChunk, ProviderError>>,
    provider: String,
    model: String,
    agent_id: String,
    owner_id: String,
    started: Instant,
) -> Result<Response, GatewayError> {
    let state = Arc::new(Mutex::new(StreamState::default()));
    let guard = UsageGuard {
        db: ctx.db.clone(),
        owner_id,
        agent_id,
        provider,
        model: model.clone(),
        started,
        state: state.clone(),
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
    owner_id: String,
    agent_id: String,
    provider: String,
    model: String,
    started: Instant,
    state: Arc<Mutex<StreamState>>,
}

impl Drop for UsageGuard {
    fn drop(&mut self) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        usage::spawn_log(
            self.db.clone(),
            UsageRecord {
                owner_id: self.owner_id.clone(),
                agent_id: self.agent_id.clone(),
                operation_type: "direct_llm",
                provider: self.provider.clone(),
                model: self.model.clone(),
                usage: st.usage.clone(),
                latency_ms: self.started.elapsed().as_millis() as i64,
                streaming: true,
                finish_reason: st.finish_reason.clone(),
            },
        );
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;
    use crate::resolver::{ConfigCache, LLMConfig};
    use async_trait::async_trait;
    use jsonwebtoken::Algorithm;
    use serde_json::json;
    use sqlx::PgPool;
    use std::time::Duration;
    use uuid::Uuid;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SECRET: &str = "gateway-secret";

    struct Store {
        config: Option<LLMConfig>,
    }
    #[async_trait]
    impl RegistryStore for Store {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<Option<LLMConfig>>, sqlx::Error> {
            Ok(Some(self.config.clone()))
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(None)
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
            tier_registry: Arc::new(crate::routing::StaticTierRegistry),
        }
    }

    fn auth_headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
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
            model: "gpt-4o-mini".into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            api_key_secret_name: None,
            pinned: false,
            pinned_model: None,
        }
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
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
        let store = Store { config: None };
        let body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] });
        let resp = chat_core(&ctx, &store, &auth_headers(&token()), body, InboundFormat::OpenAi, None).await.unwrap();

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
        let store = Store { config: Some(openai_config()) };
        // Anthropic Messages request shape: top-level system + max_tokens.
        let body = json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "system": "You are helpful.",
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let resp = chat_core(&ctx, &store, &auth_headers(&token()), body, InboundFormat::Anthropic, None)
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
        let store = Store { config: Some(openai_config()) };
        // Gemini Messages request shape: systemInstruction + contents.
        let body = json!({
            "systemInstruction": { "parts": [{ "text": "sys" }] },
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }]
        });
        let resp = chat_core(&ctx, &store, &auth_headers(&token()), body, InboundFormat::Gemini, Some(false))
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
        let store = Store { config: None };
        let body = json!({ "model": "gpt-4o", "stream": true, "messages": [{ "role": "user", "content": "hi" }] });
        let resp = chat_core(&ctx, &store, &auth_headers(&token()), body, InboundFormat::OpenAi, None).await.unwrap();
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
        let store = Store { config: None };
        let body = json!({ "model": "gpt-4o", "messages": [] });
        let err = chat_core(&ctx, &store, &HeaderMap::new(), body, InboundFormat::OpenAi, None).await.unwrap_err();
        assert!(matches!(err, GatewayError::MissingAuthHeader));
    }

    #[tokio::test]
    async fn derive_signals_inert_without_traceparent() {
        // No traceparent ⇒ inert, returned before any DB access.
        let ctx = ctx_with("http://unused".into());
        let signals = derive_boundary_signals(&HeaderMap::new(), &ctx.db).await;
        assert!(signals.conv_id.is_none());
        assert!(!signals.is_fireable_boundary());
    }

    #[tokio::test]
    async fn unsupported_provider_is_internal_error() {
        let ctx = ctx_with("http://unused".into());
        let store = Store {
            config: Some(LLMConfig {
                provider: "cohere".into(),
                model: "command-r".into(),
                fallback_models: vec![],
                temperature: None,
                max_tokens: None,
                api_key_secret_name: None,
                pinned: false,
                pinned_model: None,
            }),
        };
        let body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] });
        let err = chat_core(&ctx, &store, &auth_headers(&token()), body, InboundFormat::OpenAi, None).await.unwrap_err();
        assert!(matches!(err, GatewayError::Internal(_)));
    }
}
