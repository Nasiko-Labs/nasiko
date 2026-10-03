//! OpenAI Responses endpoint for Codex and other Responses clients.
//!
//! OpenAI requests remain native passthroughs. Anthropic and Gemini requests use
//! the documented lossy translation and carry `x-nasiko-responses-translation`.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{Value, json};

use super::chat::{RequestSignals, RoutedRequest, authenticate_request, resolve_routed_request};
use crate::LlmRouterCtx;
use crate::error::GatewayError;
use crate::inbound::responses::{
    ResponsesRequest, ResponsesStreamRenderer, TerminalOutcome, parse_request, render_response,
};
use crate::ir::Usage;
use crate::providers::{ProviderError, fallback, provider_for};
use crate::resolver::{PgRegistry, RegistryStore, RequestHint};
use crate::usage::{self, UsageRecord};

const MAX_INSPECTION_BYTES: usize = 1024 * 1024;
const TRANSLATION_HEADER: &str = "x-nasiko-responses-translation";
const REQUEST_HEADERS: &[&str] = &[
    "x-codex-turn-state",
    "x-codex-turn-metadata",
    "x-codex-beta-features",
    "x-codex-installation-id",
    "x-codex-window-id",
    "x-codex-parent-thread-id",
    "session-id",
    "thread-id",
    "x-client-request-id",
    "x-openai-subagent",
    "x-codex-routing-hint",
    "x-oai-attestation",
    "x-openai-internal-codex-responses-lite",
    "x-responsesapi-include-timing-metrics",
    "openai-beta",
    "traceparent",
    "tracestate",
];
const RESPONSE_HEADERS: &[&str] = &[
    "retry-after",
    "x-request-id",
    "x-oai-request-id",
    "cf-ray",
    "x-codex-turn-state",
    "openai-model",
    "openai-processing-ms",
    "x-reasoning-included",
    "x-models-etag",
];

pub async fn responses(
    State(ctx): State<LlmRouterCtx>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let store = PgRegistry::new(ctx.db.clone());
    match responses_core(&ctx, &store, &headers, body).await {
        Ok(response) => response,
        Err(error) => gateway_error_response(error),
    }
}

fn gateway_error_response(error: GatewayError) -> Response {
    let status = error.status();
    let code = error_code(&error);
    let message = if matches!(error, GatewayError::Internal(_)) {
        tracing::error!(error = %error, "Responses handler internal error");
        "Internal server error".to_string()
    } else {
        error.to_string()
    };
    responses_error(status, message, code)
}

async fn responses_core(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    headers: &HeaderMap,
    mut body: Value,
) -> Result<Response, GatewayError> {
    let (agent_id, owner_id) = authenticate_request(headers, &ctx.cfg)?;
    let (requested_model, stream, query) = {
        let object = body.as_object().ok_or_else(|| {
            GatewayError::BadRequest("Responses request must be a JSON object".into())
        })?;
        (
            object
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            object
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            latest_user_text(object.get("input")),
        )
    };
    let signals = RequestSignals {
        context: if ctx.request_classifier.experimental {
            let messages: Vec<crate::ir::Message> = body
                .get("input")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect();
            crate::routing::decision::conversation_context(&messages)
        } else {
            None
        },
        turn_ordinal: user_turn_ordinal(body.get("input")),
        is_tool_continuation: is_tool_continuation(body.get("input")),
        query,
    };
    let routed = resolve_routed_request(
        ctx,
        store,
        headers,
        agent_id,
        owner_id,
        RequestHint {
            provider: Some("openai"),
            model: requested_model.as_deref(),
        },
        signals,
    )
    .await?;
    let attempts = fallback::build_attempts(&routed.resolved, &ctx.cfg);
    let native_primary = routed.resolved.provider == "openai";
    let mut translated = if native_primary {
        None
    } else {
        Some(parse_request(&body)?)
    };
    let mut last_response = None;
    let mut last_error = None;
    for attempt in attempts {
        let native = native_primary && attempt.provider == "openai";
        if !native {
            let parsed = match translated.as_ref() {
                Some(parsed) => parsed,
                None => {
                    translated = Some(parse_request(&body)?);
                    translated.as_ref().expect("inserted above")
                }
            };
            let started = Instant::now();
            match translated_attempt(ctx, &routed, &attempt, parsed, started).await {
                Ok(response) => return Ok(response),
                Err(TranslatedAttemptError::Terminal(response)) => return Ok(*response),
                Err(TranslatedAttemptError::Configuration(error)) => return Err(error),
                Err(TranslatedAttemptError::Retry(error)) => {
                    last_error = Some(error.into());
                    continue;
                }
            }
        }

        let object = body.as_object_mut().expect("Responses body was validated");
        object.insert("model".into(), Value::String(attempt.model.clone()));
        if let Some(temperature) = attempt.temperature {
            object.insert("temperature".into(), json!(temperature));
        }
        if let Some(max_tokens) = attempt.max_tokens {
            object.insert("max_output_tokens".into(), json!(max_tokens));
        }
        let started = Instant::now();
        let mut guard = AttemptGuard::new(ctx, &routed, &attempt, started, stream);
        let mut request = ctx
            .http
            .post(format!(
                "{}/responses",
                ctx.cfg.openai_api_base.trim_end_matches('/')
            ))
            .bearer_auth(&attempt.api_key)
            .json(&body);
        request = forward_request_headers(request, headers);
        let upstream = match request.send().await {
            Ok(upstream) => upstream,
            Err(error) => {
                guard.fail("send", &error);
                let retryable = is_retryable_transport_error(&error);
                let error = GatewayError::Upstream(error.to_string());
                if retryable {
                    last_error = Some(error);
                    continue;
                }
                return Err(error);
            }
        };
        let status = upstream.status();
        if !status.is_success() {
            guard.fail_status(status.as_u16());
            let response = passthrough_error(upstream).await?;
            if status.as_u16() == 429 || status.is_server_error() {
                last_response = Some(response);
                continue;
            }
            return Ok(response);
        }
        let status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
        return if stream {
            guard.disarm();
            stream_response(ctx, upstream, routed, attempt.clone(), started, status)
        } else {
            nonstream_response(
                ctx,
                upstream,
                routed,
                attempt.clone(),
                started,
                status,
                guard,
            )
            .await
        };
    }
    last_response.map(Ok).unwrap_or_else(|| {
        Err(last_error.unwrap_or_else(|| GatewayError::Upstream("no Responses attempts".into())))
    })
}

fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout()
}

enum TranslatedAttemptError {
    Terminal(Box<Response>),
    Configuration(GatewayError),
    Retry(ProviderError),
}

async fn translated_attempt(
    ctx: &LlmRouterCtx,
    routed: &RoutedRequest,
    attempt: &crate::resolver::ResolvedConfig,
    parsed: &ResponsesRequest,
    started: Instant,
) -> Result<Response, TranslatedAttemptError> {
    if !parsed.advisory_fields.is_empty() {
        tracing::warn!(
            provider = %attempt.provider,
            advisory_fields = ?parsed.advisory_fields,
            "lossy Responses translation accepted advisory controls"
        );
    }
    let mut guard = AttemptGuard::new(ctx, routed, attempt, started, parsed.stream);
    let provider = provider_for(attempt, &ctx.http, &ctx.cfg).map_err(|error| {
        guard.fail("routing", &error);
        TranslatedAttemptError::Configuration(error)
    })?;
    let mut request = parsed.chat.clone();
    let mut config = attempt.clone();
    let mut applied = Vec::new();
    loop {
        if parsed.stream {
            match provider.chat_stream(&request, &config).await {
                Ok(stream) => {
                    let response =
                        translated_stream_response(ctx, routed, attempt, parsed, stream, started)
                            .map_err(|error| {
                            guard.fail("stream_setup", &error);
                            TranslatedAttemptError::Retry(ProviderError::Parse(error.to_string()))
                        })?;
                    guard.disarm();
                    return Ok(response);
                }
                Err(error) => {
                    if fallback::try_fix_param(
                        &*provider,
                        &error,
                        &mut request,
                        &mut config,
                        &mut applied,
                    ) {
                        continue;
                    }
                    guard.fail("provider", &error);
                    return Err(classify_translated_error(error));
                }
            }
        } else {
            match provider.chat(&request, &config).await {
                Ok(chat) => {
                    let finish_reason = chat
                        .choices
                        .first()
                        .and_then(|choice| choice.finish_reason.clone());
                    let usage = chat.usage.clone().map(|usage| ResponseUsage {
                        usage,
                        cached_tokens: None,
                        reasoning_tokens: None,
                    });
                    let value = render_response(chat, &attempt.model, &parsed.tool_kinds).map_err(
                        |error| {
                            guard.fail("render", &error);
                            classify_translated_error(ProviderError::Parse(error.to_string()))
                        },
                    )?;
                    let finish_reason = response_outcome(&value)
                        .map(outcome_finish_reason)
                        .or(finish_reason);
                    guard.disarm();
                    log_response_usage(
                        ctx,
                        routed.clone(),
                        attempt,
                        started,
                        false,
                        usage,
                        finish_reason,
                    );
                    return Ok(mark_lossy(Json(value).into_response()));
                }
                Err(error) => {
                    if fallback::try_fix_param(
                        &*provider,
                        &error,
                        &mut request,
                        &mut config,
                        &mut applied,
                    ) {
                        continue;
                    }
                    guard.fail("provider", &error);
                    return Err(classify_translated_error(error));
                }
            }
        }
    }
}

fn classify_translated_error(error: ProviderError) -> TranslatedAttemptError {
    if error.retryable() {
        return TranslatedAttemptError::Retry(error);
    }
    let (status, message) = match &error {
        ProviderError::Status {
            status, message, ..
        } if (400..500).contains(status) => (
            StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_REQUEST),
            message.clone(),
        ),
        _ => (StatusCode::BAD_GATEWAY, error.to_string()),
    };
    TranslatedAttemptError::Terminal(Box::new(mark_lossy(responses_error(
        status,
        message,
        if status.is_client_error() {
            "invalid_request_error"
        } else {
            "upstream_error"
        },
    ))))
}

fn translated_stream_response(
    ctx: &LlmRouterCtx,
    routed: &RoutedRequest,
    attempt: &crate::resolver::ResolvedConfig,
    parsed: &ResponsesRequest,
    provider_stream: futures::stream::BoxStream<
        'static,
        Result<crate::ir::ChatChunk, ProviderError>,
    >,
    started: Instant,
) -> Result<Response, GatewayError> {
    let usage_state = Arc::new(Mutex::new(ResponseStreamState::default()));
    let guard = ResponsesUsageGuard {
        ctx: ctx.clone(),
        routed: Some((routed.clone(), attempt.clone())),
        started,
        state: Arc::clone(&usage_state),
    };
    let model = attempt.model.clone();
    let mut renderer = ResponsesStreamRenderer::new(model.clone(), parsed.tool_kinds.clone());
    let stream = async_stream::stream! {
        let _guard = guard;
        for frame in renderer.start() { yield Ok::<String, std::io::Error>(frame); }
        futures::pin_mut!(provider_stream);
        while let Some(item) = provider_stream.next().await {
            match item {
                Ok(mut chunk) => {
                    chunk.model = model.clone();
                    if let Some(usage) = chunk.usage.clone() {
                        usage_state.lock().unwrap_or_else(|error| error.into_inner()).details = Some(ResponseUsage {
                            usage,
                            cached_tokens: None,
                            reasoning_tokens: None,
                        });
                    }
                    if let Some(reason) = chunk.choices.first().and_then(|choice| choice.finish_reason.clone()) {
                        usage_state.lock().unwrap_or_else(|error| error.into_inner()).finish_reason = Some(reason);
                    }
                    for frame in renderer.render(chunk) { yield Ok(frame); }
                }
                Err(error) => {
                    usage_state.lock().unwrap_or_else(|error| error.into_inner()).finish_reason = Some("failed:stream".into());
                    for frame in renderer.fail(error.to_string()) { yield Ok(frame); }
                    return;
                }
            }
        }
        for frame in renderer.finish() { yield Ok(frame); }
        if let Some(outcome) = renderer.outcome() {
            usage_state.lock().unwrap_or_else(|error| error.into_inner()).finish_reason =
                Some(outcome_finish_reason(outcome));
        }
    };
    Response::builder()
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .header(TRANSLATION_HEADER, "lossy")
        .body(Body::from_stream(stream))
        .map_err(|error| {
            GatewayError::Internal(format!("failed to build Responses stream: {error}"))
        })
}

fn mark_lossy(mut response: Response) -> Response {
    response.headers_mut().insert(
        TRANSLATION_HEADER,
        "lossy".parse().expect("static header value"),
    );
    response
}

fn forward_request_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for name in REQUEST_HEADERS {
        if let Some(value) = headers.get(*name) {
            request = request.header(*name, value);
        }
    }
    request
}

async fn nonstream_response(
    ctx: &LlmRouterCtx,
    upstream: reqwest::Response,
    routed: RoutedRequest,
    attempt: crate::resolver::ResolvedConfig,
    started: Instant,
    status: StatusCode,
    mut guard: AttemptGuard,
) -> Result<Response, GatewayError> {
    let headers = upstream.headers().clone();
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            guard.fail("body_read", &error);
            return Err(GatewayError::Upstream(error.to_string()));
        }
    };
    let parsed = serde_json::from_slice::<Value>(&bytes).ok();
    let details = parsed
        .as_ref()
        .and_then(|value| response_usage(value.get("usage")));
    let finish_reason = parsed
        .as_ref()
        .and_then(response_outcome)
        .map(outcome_finish_reason);
    guard.disarm();
    log_response_usage(
        ctx,
        routed,
        &attempt,
        started,
        false,
        details,
        finish_reason,
    );
    build_success_response(status, &headers, Body::from(bytes))
}

fn stream_response(
    ctx: &LlmRouterCtx,
    upstream: reqwest::Response,
    routed: RoutedRequest,
    attempt: crate::resolver::ResolvedConfig,
    started: Instant,
    status: StatusCode,
) -> Result<Response, GatewayError> {
    let headers = upstream.headers().clone();
    let state = Arc::new(Mutex::new(ResponseStreamState::default()));
    let guard = ResponsesUsageGuard {
        ctx: ctx.clone(),
        routed: Some((routed, attempt)),
        started,
        state: Arc::clone(&state),
    };
    let bytes = upstream.bytes_stream();
    let stream = async_stream::stream! {
        let _guard = guard;
        let mut inspector = SseInspector::new(Arc::clone(&state));
        futures::pin_mut!(bytes);
        while let Some(next) = bytes.next().await {
            match next {
                Ok(chunk) => {
                    inspector.push(&chunk);
                    yield Ok::<Bytes, std::io::Error>(chunk);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Responses upstream stream failed midstream");
                    yield Err(std::io::Error::other(error));
                    return;
                }
            }
        }
        inspector.finish();
    };
    build_success_response(status, &headers, Body::from_stream(stream))
}

fn build_success_response(
    status: StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    let mut builder = Response::builder().status(status);
    for name in [CONTENT_TYPE, CACHE_CONTROL] {
        if let Some(value) = upstream_headers.get(&name) {
            builder = builder.header(name, value);
        }
    }
    builder = copy_response_headers(builder, upstream_headers);
    builder.body(body).map_err(|error| {
        GatewayError::Internal(format!("failed to build Responses response: {error}"))
    })
}

async fn passthrough_error(upstream: reqwest::Response) -> Result<Response, GatewayError> {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = upstream.headers().clone();
    let bytes = upstream.bytes().await.map_err(|error| {
        GatewayError::Upstream(format!("failed to read upstream error: {error}"))
    })?;
    let mut response = Response::builder().status(status);
    if let Some(content_type) = headers.get(CONTENT_TYPE) {
        response = response.header(CONTENT_TYPE, content_type);
    }
    response = copy_response_headers(response, &headers);
    response.body(Body::from(bytes)).map_err(|error| {
        GatewayError::Internal(format!("failed to proxy upstream error response: {error}"))
    })
}

fn copy_response_headers(
    mut builder: axum::http::response::Builder,
    headers: &reqwest::header::HeaderMap,
) -> axum::http::response::Builder {
    for name in RESPONSE_HEADERS {
        if let Some(value) = headers.get(*name) {
            builder = builder.header(*name, value);
        }
    }
    for (name, value) in headers {
        if name.as_str().starts_with("x-ratelimit-")
            || name.as_str().starts_with("x-codex-primary-")
            || name.as_str().starts_with("x-codex-secondary-")
        {
            builder = builder.header(name, value);
        }
    }
    builder
}

struct SseInspector {
    event: Vec<u8>,
    oversized: bool,
    separator_tail: Vec<u8>,
    state: Arc<Mutex<ResponseStreamState>>,
}

impl SseInspector {
    fn new(state: Arc<Mutex<ResponseStreamState>>) -> Self {
        Self {
            event: Vec::new(),
            oversized: false,
            separator_tail: Vec::new(),
            state,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if self.oversized {
                self.separator_tail.push(byte);
                if separator_len_at_end(&self.separator_tail).is_some() {
                    self.oversized = false;
                    self.separator_tail.clear();
                } else if self.separator_tail.len() > 3 {
                    self.separator_tail.remove(0);
                }
                continue;
            }

            self.event.push(byte);
            if let Some(separator_len) = separator_len_at_end(&self.event) {
                self.event.truncate(self.event.len() - separator_len);
                inspect_event(&self.event, &self.state);
                self.event.clear();
            } else if self.event.len() > MAX_INSPECTION_BYTES {
                tracing::warn!(
                    limit = MAX_INSPECTION_BYTES,
                    "Responses SSE event exceeded inspection limit; skipping event"
                );
                let keep_from = self.event.len().saturating_sub(3);
                self.separator_tail
                    .extend_from_slice(&self.event[keep_from..]);
                self.event.clear();
                self.oversized = true;
            }
        }
    }

    fn finish(&mut self) {
        if !self.oversized && !self.event.is_empty() {
            inspect_event(&self.event, &self.state);
        }
    }
}

fn separator_len_at_end(buffer: &[u8]) -> Option<usize> {
    if buffer.ends_with(b"\r\n\r\n") {
        Some(4)
    } else if buffer.ends_with(b"\n\n") {
        Some(2)
    } else {
        None
    }
}

fn inspect_event(event: &[u8], state: &Arc<Mutex<ResponseStreamState>>) {
    let text = String::from_utf8_lossy(event);
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&data) else {
        return;
    };
    let Some(outcome) = terminal_event_outcome(value.get("type").and_then(Value::as_str)) else {
        return;
    };
    let details = value
        .get("response")
        .and_then(|response| response_usage(response.get("usage")));
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    state.details = details;
    state.finish_reason = Some(outcome_finish_reason(outcome));
}

fn terminal_event_outcome(kind: Option<&str>) -> Option<TerminalOutcome> {
    match kind {
        Some("response.completed") => Some(TerminalOutcome::Completed),
        Some("response.incomplete") => Some(TerminalOutcome::Incomplete),
        Some("response.failed") => Some(TerminalOutcome::Failed),
        _ => None,
    }
}

fn response_outcome(response: &Value) -> Option<TerminalOutcome> {
    match response.get("status").and_then(Value::as_str) {
        Some("completed") => Some(TerminalOutcome::Completed),
        Some("incomplete") => Some(TerminalOutcome::Incomplete),
        Some("failed") => Some(TerminalOutcome::Failed),
        _ => None,
    }
}

fn outcome_finish_reason(outcome: TerminalOutcome) -> String {
    match outcome {
        TerminalOutcome::Completed => "completed",
        TerminalOutcome::Incomplete => "incomplete",
        TerminalOutcome::Failed => "failed:response",
    }
    .into()
}

#[derive(Clone, Default)]
struct ResponseStreamState {
    details: Option<ResponseUsage>,
    finish_reason: Option<String>,
}

#[derive(Clone)]
struct ResponseUsage {
    usage: Usage,
    cached_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
}

fn response_usage(value: Option<&Value>) -> Option<ResponseUsage> {
    let value = value?;
    Some(ResponseUsage {
        usage: Usage {
            prompt_tokens: value.get("input_tokens").and_then(Value::as_i64),
            completion_tokens: value.get("output_tokens").and_then(Value::as_i64),
            total_tokens: value.get("total_tokens").and_then(Value::as_i64),
            prompt_tokens_details: None,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation: None,
        },
        cached_tokens: value
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_i64),
        reasoning_tokens: value
            .get("output_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_i64),
    })
}

struct ResponsesUsageGuard {
    ctx: LlmRouterCtx,
    routed: Option<(RoutedRequest, crate::resolver::ResolvedConfig)>,
    started: Instant,
    state: Arc<Mutex<ResponseStreamState>>,
}

impl Drop for ResponsesUsageGuard {
    fn drop(&mut self) {
        let Some((routed, attempt)) = self.routed.take() else {
            return;
        };
        let state = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        log_response_usage(
            &self.ctx,
            routed,
            &attempt,
            self.started,
            true,
            state.details,
            state.finish_reason,
        );
    }
}

struct AttemptGuard {
    ctx: LlmRouterCtx,
    record: Option<UsageRecord>,
    started: Instant,
}

impl AttemptGuard {
    fn new(
        ctx: &LlmRouterCtx,
        routed: &RoutedRequest,
        attempt: &crate::resolver::ResolvedConfig,
        started: Instant,
        streaming: bool,
    ) -> Self {
        Self {
            ctx: ctx.clone(),
            started,
            record: Some(UsageRecord {
                owner_id: routed.owner_id.clone(),
                agent_id: routed.agent_id.clone(),
                operation_type: "direct_llm",
                provider: attempt.provider.clone(),
                model: attempt.model.clone(),
                usage: None,
                cached_tokens: None,
                reasoning_tokens: None,
                latency_ms: started.elapsed().as_millis() as i64,
                streaming,
                finish_reason: None,
                flow_id: routed.flow_id.clone(),
                attribution_source: routed.attribution_source,
                platform_paid: attempt.platform_paid,
                // Never compressed: this surface does not go through `chat_core`.
                compress_metadata: None,
                // /v1/responses does not share chat_core, so IP-1/IP-2 never run here (PRD §9).
                brevity_metadata: None,
                // Nothing was compressed, so there is nothing to credit to a savings layer.
                compress_bytes: None,
                request_bytes: None,
            }),
        }
    }

    fn fail(&mut self, stage: &'static str, error: &dyn std::fmt::Display) {
        if let Some(record) = &mut self.record {
            record.latency_ms = record.latency_ms.max(1);
            record.finish_reason = Some(format!("failed:{stage}"));
            tracing::warn!(stage, model = %record.model, error = %error, "Responses upstream attempt failed");
        }
    }

    fn fail_status(&mut self, status: u16) {
        if let Some(record) = &mut self.record {
            record.latency_ms = record.latency_ms.max(1);
            record.finish_reason = Some(format!("http:{status}"));
            tracing::warn!(status, model = %record.model, "Responses upstream attempt returned non-success status");
        }
    }

    fn disarm(&mut self) {
        self.record = None;
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if let Some(mut record) = self.record.take() {
            record.latency_ms = record
                .latency_ms
                .max(self.started.elapsed().as_millis() as i64);
            usage::spawn_log(self.ctx.db.clone(), self.ctx.pricing.clone(), record);
        }
    }
}

fn log_response_usage(
    ctx: &LlmRouterCtx,
    routed: RoutedRequest,
    attempt: &crate::resolver::ResolvedConfig,
    started: Instant,
    streaming: bool,
    details: Option<ResponseUsage>,
    finish_reason: Option<String>,
) {
    let cached_tokens = details.as_ref().and_then(|details| details.cached_tokens);
    let reasoning_tokens = details
        .as_ref()
        .and_then(|details| details.reasoning_tokens);
    usage::spawn_log(
        ctx.db.clone(),
        ctx.pricing.clone(),
        UsageRecord {
            owner_id: routed.owner_id,
            agent_id: routed.agent_id,
            operation_type: "direct_llm",
            provider: attempt.provider.clone(),
            model: attempt.model.clone(),
            usage: details.map(|details| details.usage),
            cached_tokens,
            reasoning_tokens,
            latency_ms: started.elapsed().as_millis() as i64,
            streaming,
            finish_reason,
            flow_id: routed.flow_id,
            attribution_source: routed.attribution_source,
            platform_paid: attempt.platform_paid,
            // Never compressed: this surface does not go through `chat_core`.
            compress_metadata: None,
            brevity_metadata: None,
            compress_bytes: None,
            request_bytes: None,
        },
    );
}

fn latest_user_text(input: Option<&Value>) -> Option<String> {
    input
        .and_then(Value::as_array)?
        .iter()
        .rev()
        .find(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|item| item.get("content"))
        .and_then(content_text)
}

/// Number of top-level user turns so far in a Responses-API `input` array — the
/// `routing::user_turn_ordinal` equivalent for this wire format. Used only for a
/// coding-agent integration's `conv_id` anchor (see `RequestSignals::turn_ordinal`).
fn user_turn_ordinal(input: Option<&Value>) -> usize {
    input
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("role").and_then(Value::as_str) == Some("user"))
                .count()
        })
        .unwrap_or(0)
}

/// Whether the `input` array's last item is a tool result — the `routing::is_tool_continuation`
/// equivalent for the Responses API, whose tool results are top-level `function_call_output`
/// (or `custom_tool_call_output`) items rather than a `{role: "tool"}` message.
fn is_tool_continuation(input: Option<&Value>) -> bool {
    input
        .and_then(Value::as_array)
        .and_then(|items| items.last())
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|t| t == "function_call_output" || t == "custom_tool_call_output")
}

fn content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let text = content
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn error_code(error: &GatewayError) -> &'static str {
    match error {
        GatewayError::MissingAuthHeader
        | GatewayError::JwtSecretNotConfigured
        | GatewayError::TokenExpired
        | GatewayError::InvalidToken(_)
        | GatewayError::MissingAgentId => "invalid_api_key",
        GatewayError::Forbidden(_) => "permission_denied",
        GatewayError::BadRequest(_) => "invalid_request_error",
        GatewayError::NoRegistryEntry(_)
        | GatewayError::SecretNotFound(_, _)
        | GatewayError::NoApiKey => "routing_configuration_error",
        GatewayError::Upstream(_) => "upstream_error",
        GatewayError::Internal(_) => "internal_error",
    }
}

fn responses_error(status: StatusCode, message: String, code: &'static str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": if status == StatusCode::UNAUTHORIZED {
                    "authentication_error"
                } else if status.is_server_error() {
                    "server_error"
                } else {
                    "invalid_request_error"
                },
                "param": Value::Null,
                "code": code,
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use jsonwebtoken::Algorithm;
    use sqlx::PgPool;
    use std::time::Duration;
    use uuid::Uuid;

    use crate::config::GatewayConfig;
    use crate::resolver::{AgentConfigResult, ConfigCache, LLMConfig};

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SECRET: &str = "responses-secret";

    struct Store {
        provider: &'static str,
        fallback_models: Vec<String>,
    }

    struct NoTiers;

    #[async_trait]
    impl crate::routing::TierRegistry for NoTiers {
        async fn model_for(&self, _: &str, _: crate::routing::Tier) -> Option<String> {
            None
        }
    }

    impl Store {
        fn new(provider: &'static str) -> Self {
            Self {
                provider,
                fallback_models: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl RegistryStore for Store {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(Some(AgentConfigResult {
                compress_enabled: false,
                config: Some(LLMConfig {
                    provider: self.provider.into(),
                    model: Some("resolved-model".into()),
                    fallback_models: self.fallback_models.clone(),
                    temperature: Some(0.2),
                    max_tokens: Some(4096),
                    api_key_secret_name: None,
                    pinned: false,
                    pinned_model: None,
                    tier1_model: None,
                    tier2_model: None,
                    tier3_model: None,
                }),
                agent_pinned_model: None,
                is_coding_agent: true,
            }))
        }

        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<crate::routing::attribution::LiveFlow>, sqlx::Error> {
            Ok(None)
        }

        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(None)
        }

        async fn fetch_custom_provider(
            &self,
            _: &str,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
    }

    fn ctx(base: String) -> LlmRouterCtx {
        let provider_base = base.clone();
        LlmRouterCtx {
            db: PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            http: reqwest::Client::new(),
            cfg: Arc::new(GatewayConfig {
                agent_jwt_secret: SECRET.into(),
                openai_api_base: base,
                anthropic_api_base: provider_base.clone(),
                gemini_api_base: provider_base,
                platform_openai_api_key: "upstream-key".into(),
                platform_anthropic_api_key: "anthropic-key".into(),
                platform_gemini_api_key: "gemini-key".into(),
                ..Default::default()
            }),
            cache: Arc::new(ConfigCache::new(Duration::from_secs(30))),
            router_cache: Arc::new(crate::routing::NoopCache),
            tier_registry: Arc::new(NoTiers),
            cell_store: Arc::new(crate::routing::InMemoryCellStore::new()),
            salience_gate: Arc::new(crate::routing::salience::AllowAllGate),
            request_classifier: Arc::new(crate::routing::decision::ClassifierRuntime::default()),
            pricing: Arc::new(nasiko_pricing::PricingEngine::new(
                PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            )),
        }
    }

    fn headers() -> HeaderMap {
        let token =
            crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        headers
    }

    async fn body(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn auth_is_required_before_upstream() {
        let response = responses(
            State(ctx("http://unused".into())),
            HeaderMap::new(),
            Json(json!({})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["error"]["code"], "invalid_api_key");
    }

    #[tokio::test]
    async fn internal_error_details_are_not_exposed() {
        let response =
            gateway_error_response(GatewayError::Internal("database password leaked".into()));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["error"]["message"], "Internal server error");
        assert_eq!(value["error"]["type"], "server_error");
        assert_eq!(value["error"]["code"], "internal_error");
    }

    #[tokio::test]
    async fn nonstream_overrides_model_key_and_params_and_preserves_unknown_fields() {
        let mut server = mockito::Server::new_async().await;
        let request = server
            .mock("POST", "/responses")
            .match_header("authorization", "Bearer upstream-key")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "resolved-model",
                "temperature": 0.2,
                "max_output_tokens": 4096,
                "instructions": "keep",
                "reasoning": {"effort":"high"},
                "input": [{"role":"user","content":[{"type":"input_text","text":"hello"}]}],
                "tools": [{"type":"custom","name":"apply_patch","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}}],
                "include": ["reasoning.encrypted_content"],
                "text": {"verbosity":"low"},
                "store": false,
                "custom": {"kept":true}
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "request-1")
            .with_header("x-oai-request-id", "oai-request-1")
            .with_header("cf-ray", "ray-1")
            .with_header("x-codex-primary-model", "primary-model")
            .with_header("x-codex-secondary-region", "secondary-region")
            .with_header("openai-model", "resolved-model")
            .with_header("x-ratelimit-remaining-requests", "9")
            .with_body(
                json!({
                    "id":"resp_1", "status":"completed", "output":[],
                    "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5,
                        "input_tokens_details":{"cached_tokens":1},
                        "output_tokens_details":{"reasoning_tokens":1}}
                })
                .to_string(),
            )
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()), &Store::new("openai"), &headers(),
            json!({
                "model":"requested", "instructions":"keep",
                "input":[{"role":"user","content":[{"type":"input_text","text":"hello"}]}],
                "tools":[{"type":"custom","name":"apply_patch","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}}],
                "reasoning":{"effort":"high"}, "include":["reasoning.encrypted_content"],
                "text":{"verbosity":"low"}, "store":false, "custom":{"kept":true}
            }),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-request-id"], "request-1");
        assert_eq!(response.headers()["x-oai-request-id"], "oai-request-1");
        assert_eq!(response.headers()["cf-ray"], "ray-1");
        assert_eq!(response.headers()["x-codex-primary-model"], "primary-model");
        assert_eq!(
            response.headers()["x-codex-secondary-region"],
            "secondary-region"
        );
        assert_eq!(response.headers()["openai-model"], "resolved-model");
        assert_eq!(response.headers()["x-ratelimit-remaining-requests"], "9");
        assert!(!response.headers().contains_key(TRANSLATION_HEADER));
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["id"], "resp_1");
        request.assert_async().await;
    }

    #[tokio::test]
    async fn streaming_bytes_and_completed_usage_event_pass_through() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":2,\"output_tokens\":1,\"total_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":1},\"output_tokens_details\":{\"reasoning_tokens\":1}}}}\n\n"
        );
        let request = server
            .mock("POST", "/responses")
            .match_body(mockito::Matcher::PartialJson(
                json!({"stream":true,"model":"resolved-model"}),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_header("x-codex-turn-state", "turn-state-1")
            .with_body(sse)
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("openai"),
            &headers(),
            json!({"model":"requested","stream":true,"input":[]}),
        )
        .await
        .unwrap();
        assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
        assert_eq!(response.headers()["x-codex-turn-state"], "turn-state-1");
        assert!(!response.headers().contains_key(TRANSLATION_HEADER));
        assert_eq!(body(response).await, sse.as_bytes());
        request.assert_async().await;
    }

    #[tokio::test]
    async fn anthropic_text_and_custom_tool_calls_render_as_responses() {
        let mut server = mockito::Server::new_async().await;
        let request = server.mock("POST", "/messages")
            .match_header("x-api-key", "anthropic-key")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model":"resolved-model",
                "system":"be useful",
                "messages":[{"role":"user","content":"edit"}],
                "tools":[{"name":"apply_patch","input_schema":{"type":"object","properties":{"input":{"type":"string"}},"required":["input"],"additionalProperties":false}}]
            })))
            .with_status(200).with_header("content-type", "application/json")
            .with_body(json!({
                "id":"msg_1","type":"message","role":"assistant",
                "content":[{"type":"text","text":"Applying."},{"type":"tool_use","id":"call_keep","name":"apply_patch","input":{"input":"*** Begin Patch"}}],
                "stop_reason":"tool_use","usage":{"input_tokens":8,"output_tokens":4}
            }).to_string()).create_async().await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("anthropic"),
            &headers(),
            json!({
                "instructions":"be useful","input":[{"role":"user","content":"edit"}],
                "tools":[{"type":"custom","name":"apply_patch","description":"patch files"}]
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.headers()[TRANSLATION_HEADER], "lossy");
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["status"], "completed");
        assert_eq!(value["output"][0]["content"][0]["text"], "Applying.");
        assert_eq!(value["output"][1]["type"], "custom_tool_call");
        assert_eq!(value["output"][1]["call_id"], "call_keep");
        assert_eq!(value["output"][1]["input"], "*** Begin Patch");
        assert_eq!(value["usage"]["total_tokens"], 12);
        request.assert_async().await;
    }

    #[tokio::test]
    async fn gemini_text_and_function_calls_render_as_responses() {
        let mut server = mockito::Server::new_async().await;
        let request = server.mock("POST", "/models/resolved-model:generateContent")
            .match_header("x-goog-api-key", "gemini-key")
            .match_body(mockito::Matcher::PartialJson(json!({
                "contents":[{"role":"user","parts":[{"text":"weather"}]}],
                "tools":[{"functionDeclarations":[{"name":"weather","parameters":{"type":"object"}}]}]
            })))
            .with_status(200).with_header("content-type", "application/json")
            .with_body(json!({
                "responseId":"gem_1","candidates":[{"content":{"role":"model","parts":[{"text":"Checking."},{"functionCall":{"name":"weather","args":{"city":"Paris"}}}]},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}
            }).to_string()).create_async().await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("gemini"),
            &headers(),
            json!({
                "input":[{"role":"user","content":"weather"}],
                "tools":[{"type":"function","name":"weather","parameters":{"type":"object"}}]
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.headers()[TRANSLATION_HEADER], "lossy");
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["output"][0]["content"][0]["text"], "Checking.");
        assert_eq!(value["output"][1]["type"], "function_call");
        assert_eq!(value["output"][1]["name"], "weather");
        assert_eq!(value["model"], "resolved-model");
        request.assert_async().await;
    }

    #[tokio::test]
    async fn provider_blocking_reasons_render_failed_without_calls() {
        for (provider, path, provider_body) in [
            (
                "anthropic",
                "/messages",
                json!({
                    "id":"m","content":[{"type":"tool_use","id":"c","name":"f","input":{}}],
                    "stop_reason":"refusal","usage":{"input_tokens":1,"output_tokens":1}
                }),
            ),
            (
                "gemini",
                "/models/resolved-model:generateContent",
                json!({
                    "candidates":[{"content":{"parts":[{"functionCall":{"name":"f","args":{}}}]},"finishReason":"SAFETY"}],
                    "usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2}
                }),
            ),
        ] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", path)
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(provider_body.to_string())
                .create_async()
                .await;
            let response = responses_core(
                &ctx(server.url()),
                &Store::new(provider),
                &headers(),
                json!({"input":"hi","tools":[{"type":"function","name":"f"}]}),
            )
            .await
            .unwrap();
            let value: Value = serde_json::from_slice(&body(response).await).unwrap();
            assert_eq!(value["status"], "failed", "{provider}: {value}");
            assert_eq!(value["error"]["code"], "content_filter");
            assert!(value["output"].as_array().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn translated_stream_has_terminal_response_event_and_no_done_sentinel() {
        let mut server = mockito::Server::new_async().await;
        let upstream = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_s\",\"usage\":{\"input_tokens\":2}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        );
        server
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({"stream":true})))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(upstream)
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("anthropic"),
            &headers(),
            json!({"stream":true,"input":[{"role":"user","content":"hi"}]}),
        )
        .await
        .unwrap();
        assert_eq!(response.headers()[TRANSLATION_HEADER], "lossy");
        let body = String::from_utf8(body(response).await).unwrap();
        assert!(body.contains("event: response.created"));
        assert!(body.contains("event: response.output_text.delta"));
        assert!(body.contains("event: response.completed"));
        assert!(body.contains("\"total_tokens\":3"));
        assert!(!body.contains("[DONE]"));
    }

    #[tokio::test]
    async fn translated_max_tokens_is_incomplete_nonstream_and_stream() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id":"m1","content":[{"type":"text","text":"partial"}],
                    "stop_reason":"max_tokens","usage":{"input_tokens":2,"output_tokens":3}
                })
                .to_string(),
            )
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("anthropic"),
            &headers(),
            json!({"input":"hi","stream":false}),
        )
        .await
        .unwrap();
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["status"], "incomplete");
        assert_eq!(value["incomplete_details"]["reason"], "max_output_tokens");

        server.reset();
        server
            .mock("POST", "/messages")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m2\",\"usage\":{\"input_tokens\":2}}}\n\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":3}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            ))
            .create_async().await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("anthropic"),
            &headers(),
            json!({"input":"hi","stream":true}),
        )
        .await
        .unwrap();
        let stream = String::from_utf8(body(response).await).unwrap();
        assert_eq!(stream.matches("event: response.incomplete").count(), 1);
        assert!(!stream.contains("event: response.completed"));
        assert!(stream.contains("\"reason\":\"max_output_tokens\""));
    }

    #[tokio::test]
    async fn malformed_or_premature_translated_stream_fails_exactly_once() {
        for (provider, path, payload) in [
            ("anthropic", "/messages", "data: {not-json}\n\n"),
            (
                "anthropic",
                "/messages",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\"}}\n\n",
            ),
            (
                "gemini",
                "/models/resolved-model:streamGenerateContent?alt=sse",
                "data: {\"error\":{\"code\":503,\"message\":\"down\"}}\n\n",
            ),
        ] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", path)
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body(payload)
                .create_async()
                .await;
            let response = responses_core(
                &ctx(server.url()),
                &Store::new(provider),
                &headers(),
                json!({"input":"hi","stream":true}),
            )
            .await
            .unwrap();
            let stream = String::from_utf8(body(response).await).unwrap();
            assert_eq!(
                stream.matches("event: response.failed").count(),
                1,
                "{stream}"
            );
            assert!(!stream.contains("event: response.completed"), "{stream}");
        }
    }

    #[tokio::test]
    async fn malformed_translated_tool_call_returns_failed_response() {
        let mut server = mockito::Server::new_async().await;
        server.mock("POST", "/models/resolved-model:generateContent")
            .with_status(200).with_header("content-type", "application/json")
            .with_body(json!({
                "candidates":[{"content":{"parts":[{"functionCall":{"name":"custom","args":{"wrong":true}}}]},"finishReason":"STOP"}]
            }).to_string()).create_async().await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("gemini"),
            &headers(),
            json!({"input":"hi","tools":[{"type":"custom","name":"custom"}]}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["status"], "failed");
        assert_eq!(value["error"]["code"], "invalid_tool_call");
        assert!(value["output"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unsupported_provider_configuration_is_terminal_internal_error() {
        let context = ctx("http://unused".into());
        let attempt = crate::resolver::ResolvedConfig {
            provider: "unsupported".into(),
            model: "model".into(),
            litellm_model: "unsupported/model".into(),
            api_key: "key".into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            has_llm_config: true,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: false,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: false,
        };
        let routed = RoutedRequest {
            agent_id: AGENT.into(),
            owner_id: OWNER.into(),
            resolved: attempt.clone(),
            flow_id: None,
            attribution_source: None,
        };
        let parsed = parse_request(&json!({"input":"hi"})).unwrap();
        let result = translated_attempt(&context, &routed, &attempt, &parsed, Instant::now()).await;
        assert!(matches!(
            result,
            Err(TranslatedAttemptError::Configuration(
                GatewayError::Internal(_)
            ))
        ));
    }

    #[tokio::test]
    async fn native_upstream_errors_are_passed_through() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/responses")
            .with_status(429)
            .with_header("content-type", "application/json")
            .with_header("retry-after", "3")
            .with_header("x-request-id", "request-error")
            .with_header("x-oai-request-id", "oai-request-error")
            .with_header("cf-ray", "ray-error")
            .with_header("x-codex-primary-error", "primary-error")
            .with_header("x-codex-secondary-error", "secondary-error")
            .with_header("x-codex-turn-state", "next-state")
            .with_header("x-ratelimit-reset-requests", "10ms")
            .with_body(r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#)
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("openai"),
            &headers(),
            json!({"model":"requested","input":[]}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()["retry-after"], "3");
        assert_eq!(response.headers()["x-request-id"], "request-error");
        assert_eq!(response.headers()["x-oai-request-id"], "oai-request-error");
        assert_eq!(response.headers()["cf-ray"], "ray-error");
        assert_eq!(response.headers()["x-codex-primary-error"], "primary-error");
        assert_eq!(
            response.headers()["x-codex-secondary-error"],
            "secondary-error"
        );
        assert_eq!(response.headers()["x-codex-turn-state"], "next-state");
        assert_eq!(response.headers()["x-ratelimit-reset-requests"], "10ms");
        assert!(
            String::from_utf8(body(response).await)
                .unwrap()
                .contains("rate_limit_exceeded")
        );
    }

    #[tokio::test]
    async fn request_headers_use_a_strict_codex_allowlist() {
        let mut server = mockito::Server::new_async().await;
        let request = server
            .mock("POST", "/responses")
            .match_header("authorization", "Bearer upstream-key")
            .match_header("x-codex-turn-state", "turn-state")
            .match_header("x-codex-turn-metadata", "turn-metadata")
            .match_header("x-codex-installation-id", "installation")
            .match_header("session-id", "session")
            .match_header("thread-id", "thread")
            .match_header("x-client-request-id", "thread")
            .match_header("openai-project", mockito::Matcher::Missing)
            .match_header("x-api-key", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"resp_headers","usage":{}}"#)
            .create_async()
            .await;
        let mut inbound = headers();
        inbound.insert("x-codex-turn-state", "turn-state".parse().unwrap());
        inbound.insert("x-codex-turn-metadata", "turn-metadata".parse().unwrap());
        inbound.insert("x-codex-installation-id", "installation".parse().unwrap());
        inbound.insert("session-id", "session".parse().unwrap());
        inbound.insert("thread-id", "thread".parse().unwrap());
        inbound.insert("x-client-request-id", "thread".parse().unwrap());
        inbound.insert("openai-project", "unsafe-project".parse().unwrap());
        inbound.insert("x-api-key", "unsafe-key".parse().unwrap());
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("openai"),
            &inbound,
            json!({"model":"requested","input":[]}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        request.assert_async().await;
    }

    #[tokio::test]
    async fn same_provider_and_cross_provider_fallbacks_are_supported() {
        let mut server = mockito::Server::new_async().await;
        let primary = server
            .mock("POST", "/responses")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model":"resolved-model"}),
            ))
            .with_status(503)
            .with_body("unavailable")
            .create_async()
            .await;
        let fallback = server
            .mock("POST", "/responses")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model":"gpt-fallback"}),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"resp_fallback","usage":{}}"#)
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store {
                provider: "openai",
                fallback_models: vec!["openai/gpt-fallback".into()],
            },
            &headers(),
            json!({"model":"requested","input":[]}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        primary.assert_async().await;
        fallback.assert_async().await;

        let anthropic = server
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model":"claude-opus-4",
                "messages":[{"role":"user","content":"hello"}]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id":"msg_fallback","type":"message","role":"assistant",
                    "content":[{"type":"text","text":"from claude"}],
                    "stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":2}
                })
                .to_string(),
            )
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store {
                provider: "openai",
                fallback_models: vec!["anthropic/claude-opus-4".into()],
            },
            &headers(),
            json!({"model":"requested","input":[{"role":"user","content":"hello"}]}),
        )
        .await
        .unwrap();
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["model"], "claude-opus-4");
        assert_eq!(value["output"][0]["content"][0]["text"], "from claude");
        anthropic.assert_async().await;
    }

    #[tokio::test]
    async fn fallback_retries_429_but_not_400() {
        let mut server = mockito::Server::new_async().await;
        let bad_request = server
            .mock("POST", "/responses")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model":"resolved-model"}),
            ))
            .with_status(400)
            .with_body("bad request")
            .create_async()
            .await;
        let unused_fallback = server
            .mock("POST", "/responses")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model":"gpt-fallback"}),
            ))
            .expect(0)
            .with_status(200)
            .create_async()
            .await;
        let store = Store {
            provider: "openai",
            fallback_models: vec!["openai/gpt-fallback".into()],
        };
        let response = responses_core(
            &ctx(server.url()),
            &store,
            &headers(),
            json!({"model":"requested","input":[]}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response.headers().contains_key(TRANSLATION_HEADER));
        bad_request.assert_async().await;
        unused_fallback.assert_async().await;

        server.reset();
        let rate_limited = server
            .mock("POST", "/responses")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model":"resolved-model"}),
            ))
            .with_status(429)
            .with_body("slow down")
            .create_async()
            .await;
        let fallback = server
            .mock("POST", "/responses")
            .match_body(mockito::Matcher::PartialJson(
                json!({"model":"gpt-fallback"}),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"resp_fallback","usage":{}}"#)
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &store,
            &headers(),
            json!({"model":"requested","input":[]}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        rate_limited.assert_async().await;
        fallback.assert_async().await;
    }

    #[tokio::test]
    async fn translated_provider_400_is_responses_shaped_and_does_not_fallback() {
        let mut server = mockito::Server::new_async().await;
        let rejected = server
            .mock("POST", "/messages")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"type":"error","error":{"message":"bad tool schema"}}"#)
            .expect(1)
            .create_async()
            .await;
        let fallback = server
            .mock("POST", "/models/gemini-fallback:generateContent")
            .with_status(200)
            .expect(0)
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store {
                provider: "anthropic",
                fallback_models: vec!["gemini/gemini-fallback".into()],
            },
            &headers(),
            json!({"input":"hello"}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[TRANSLATION_HEADER], "lossy");
        let value: Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(value["error"]["type"], "invalid_request_error");
        assert_eq!(value["error"]["code"], "invalid_request_error");
        rejected.assert_async().await;
        fallback.assert_async().await;
    }

    #[tokio::test]
    async fn huge_malformed_sse_event_is_passed_through() {
        let mut server = mockito::Server::new_async().await;
        let sse = format!(
            "data: {{\"unterminated\":\"{}\"",
            "x".repeat(MAX_INSPECTION_BYTES + 1)
        );
        server
            .mock("POST", "/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse.clone())
            .create_async()
            .await;
        let response = responses_core(
            &ctx(server.url()),
            &Store::new("openai"),
            &headers(),
            json!({"model":"requested","stream":true,"input":[]}),
        )
        .await
        .unwrap();
        assert_eq!(body(response).await, sse.as_bytes());
    }

    #[test]
    fn oversized_sse_event_is_skipped_and_later_completion_is_inspected() {
        let state = Arc::new(Mutex::new(ResponseStreamState::default()));
        let mut inspector = SseInspector::new(Arc::clone(&state));
        let oversized = format!("data: {}", "x".repeat(MAX_INSPECTION_BYTES + 1));
        for chunk in oversized.as_bytes().chunks(8191) {
            inspector.push(chunk);
            assert!(inspector.event.len() <= MAX_INSPECTION_BYTES);
            assert!(inspector.separator_tail.len() <= 3);
        }
        inspector.push(b"\r\n");
        inspector.push(b"\r");
        inspector.push(b"\n");
        inspector.push(
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":1,\"total_tokens\":3}}}\n",
        );
        inspector.push(b"\n");
        inspector.finish();

        let usage = state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .details
            .clone()
            .expect("completion usage should be captured");
        assert_eq!(usage.usage.total_tokens, Some(3));
        assert_eq!(
            state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .finish_reason
                .as_deref(),
            Some("completed")
        );
    }

    #[test]
    fn inspector_accounts_for_all_terminal_outcomes() {
        for (event, expected) in [
            ("response.completed", "completed"),
            ("response.incomplete", "incomplete"),
            ("response.failed", "failed:response"),
        ] {
            let state = Arc::new(Mutex::new(ResponseStreamState::default()));
            inspect_event(
                format!(
                    "data: {{\"type\":\"{event}\",\"response\":{{\"usage\":{{\"input_tokens\":2,\"output_tokens\":1,\"total_tokens\":3}}}}}}"
                )
                .as_bytes(),
                &state,
            );
            let state = state.lock().unwrap_or_else(|error| error.into_inner());
            assert_eq!(state.finish_reason.as_deref(), Some(expected));
            assert_eq!(state.details.as_ref().unwrap().usage.total_tokens, Some(3));
        }
    }

    #[tokio::test]
    async fn attempt_guard_can_be_disarmed_without_a_database() {
        let context = ctx("http://unused".into());
        let routed = RoutedRequest {
            agent_id: AGENT.into(),
            owner_id: OWNER.into(),
            resolved: crate::resolver::ResolvedConfig {
                provider: "openai".into(),
                model: "model".into(),
                litellm_model: "openai/model".into(),
                api_key: "key".into(),
                fallback_models: vec![],
                temperature: None,
                max_tokens: None,
                has_llm_config: true,
                pinned_model: None,
                tier1_model: None,
                tier2_model: None,
                tier3_model: None,
                platform_paid: true,
                custom_endpoint: None,
                is_coding_agent: false,
                compress_enabled: false,
            },
            flow_id: None,
            attribution_source: None,
        };
        let mut guard =
            AttemptGuard::new(&context, &routed, &routed.resolved, Instant::now(), false);
        let record = guard.record.as_ref().unwrap();
        assert_eq!(record.provider, "openai");
        assert_eq!(record.model, "model");
        assert!(record.platform_paid);
        assert!(record.finish_reason.is_none());
        guard.disarm();
        assert!(guard.record.is_none());
    }
}
