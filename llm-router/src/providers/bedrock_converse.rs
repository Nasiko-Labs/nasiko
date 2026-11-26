//! AWS Bedrock Converse provider — OpenAI ⇄ Bedrock Converse API translation.
//!
//! The Bedrock Runtime Converse API (`/model/{id}/converse`) supports every Bedrock
//! model, including INFERENCE_PROFILE-only ones (Claude, GPT-6, Grok, …) that the
//! OpenAI-compatible Mantle surface cannot serve. The trade-off: the wire format is
//! Bedrock's own, so this spoke translates in both directions.
//!
//! Request: system messages → top-level `system`; user/assistant content → `[{text}]`
//! blocks; tool calls → `toolUse` blocks; tool results → `toolResult` in a following
//! user turn; tools → `toolConfig.tools[].toolSpec`; params → `inferenceConfig`. An
//! agent's `cache_control` markers become `cachePoint` blocks where the model accepts them.
//! Brevity's directive ends the last user turn rather than joining `system` ([`crate::brevity`]).
//! With `TOKEN_CACHE_MARKERS` on, the router fills cache points the agent left free
//! ([`add_cache_points`]).
//!
//! Response: `output.message.content[].text` → concatenated content; `toolUse` →
//! OpenAI `tool_calls[]`; `stopReason` mapped; `usage` fields renamed.
//!
//! Streaming: Bedrock uses AWS event-stream binary framing
//! (`application/vnd.amazon.eventstream`), not SSE. A minimal decoder extracts the
//! JSON payloads from each frame.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use bytes::{Buf, BytesMut};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Map, Value, json};

use super::media;
use super::{ProviderClient, ProviderError, delta_chunk, finish_chunk, now_unix, usage_chunk};
use crate::ir::{
    CACHE_CONTROL, ChatChunk, ChatRequest, ChatResponse, Choice, ContentPart, ContentView, Delta,
    EmbeddingsRequest, EmbeddingsResponse, FunctionCall, FunctionCallDelta, Message, ToolCall,
    ToolCallDelta, ToolDef, UnsupportedPart, Usage,
};
use crate::resolver::{ResolvedConfig, ResolvedEmbeddingConfig};

use super::dialect::{bedrock_region, bedrock_region_prefix};

/// Bedrock requires `maxTokens` in `inferenceConfig`; used when neither config nor
/// request sets it.
const DEFAULT_MAX_TOKENS: i64 = 4096;
/// Bedrock accepts at most this many cache points in one request, on Claude and Nova alike.
const MAX_CACHE_POINTS: usize = 4;

pub struct BedrockConverseProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://bedrock-runtime.us-west-2.amazonaws.com`.
    base: String,
    /// Add the router's own cache points ([`add_cache_points`]).
    cache_markers: bool,
}

impl BedrockConverseProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self {
            http,
            base,
            cache_markers: false,
        }
    }

    /// Fill the cache points an agent leaves free (`TOKEN_CACHE_MARKERS`).
    pub fn with_cache_markers(mut self, enabled: bool) -> Self {
        self.cache_markers = enabled;
        self
    }

    fn request_body(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<(Value, HashMap<String, String>), ProviderError> {
        let (mut body, name_map) = to_converse_request_inner(req, cfg)?;
        if self.cache_markers {
            add_cache_points(&mut body, CacheSupport::for_router(&cfg.model));
        }
        Ok((body, name_map))
    }

    fn status_error(status: reqwest::StatusCode, body: String) -> ProviderError {
        ProviderError::Status {
            status: status.as_u16(),
            message: body,
            retryable: status.as_u16() == 429 || status.is_server_error(),
        }
    }
}

/// Resolve the model ID to send to the Bedrock Runtime. INFERENCE_PROFILE models
/// need a region prefix (`us.`, `eu.`, `ap.`); ON_DEMAND models must NOT have one.
///
/// Heuristic: if the model already carries a known prefix, use it as-is. Otherwise
/// the caller should try the raw ID first; if Bedrock returns an access error the
/// prefixed version can be retried. For now we expose the helper so the admin API's
/// probe can test both forms.
pub fn converse_model_id(model: &str, _base: &str) -> String {
    // Already has a region prefix.
    if model.starts_with("us.")
        || model.starts_with("eu.")
        || model.starts_with("ap.")
        || model.starts_with("global.")
    {
        return model.to_string();
    }
    model.to_string()
}

/// Build the prefixed model ID for inference-profile models.
pub fn prefixed_model_id(model: &str, base: &str) -> String {
    if model.starts_with("us.")
        || model.starts_with("eu.")
        || model.starts_with("ap.")
        || model.starts_with("global.")
    {
        return model.to_string();
    }
    let prefix = bedrock_region(base)
        .map(bedrock_region_prefix)
        .unwrap_or("us");
    format!("{prefix}.{model}")
}

#[async_trait]
impl ProviderClient for BedrockConverseProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let model_id = converse_model_id(&cfg.model, &self.base);
        let (body, name_map) = self.request_body(req, cfg)?;
        let url = format!("{}/model/{}/converse", self.base, model_id);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&cfg.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if should_retry_with_prefix(status.as_u16(), &text, &model_id) {
                let prefixed = prefixed_model_id(&cfg.model, &self.base);
                if prefixed != model_id {
                    return self.chat_with_model(req, cfg, &prefixed).await;
                }
            }
            return Err(Self::status_error(status, text));
        }

        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        from_converse_response(&value, &cfg.model, &name_map)
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let model_id = converse_model_id(&cfg.model, &self.base);
        let (body, name_map) = self.request_body(req, cfg)?;
        let url = format!("{}/model/{}/converse-stream", self.base, model_id);

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let tool_count = body
            .pointer("/toolConfig/tools")
            .and_then(|t| t.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let msg_count = body
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        tracing::info!(
            target: "nasiko::llm_router::bedrock",
            model = %model_id,
            body_bytes = body_str.len(),
            tool_count,
            msg_count,
            has_system = body.get("system").is_some(),
            has_tool_config = body.get("toolConfig").is_some(),
            "bedrock converse-stream: outgoing request stats"
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&cfg.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(none)")
            .to_string();
        let amzn_error = resp
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        tracing::info!(
            target: "nasiko::llm_router::bedrock",
            %status,
            content_type = %content_type,
            amzn_error = %amzn_error,
            "bedrock converse-stream: response status and headers"
        );
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(
                target: "nasiko::llm_router::bedrock",
                %status,
                model = %model_id,
                body_preview = &text[..text.len().min(500)],
                "bedrock converse-stream: non-success response body"
            );
            if should_retry_with_prefix(status.as_u16(), &text, &model_id) {
                let prefixed = prefixed_model_id(&cfg.model, &self.base);
                if prefixed != model_id {
                    tracing::info!(
                        target: "nasiko::llm_router::bedrock",
                        original = %model_id,
                        prefixed = %prefixed,
                        "bedrock: retrying with region-prefixed model ID"
                    );
                    return self.stream_with_model(req, cfg, &prefixed).await;
                }
            }
            return Err(Self::status_error(status, text));
        }

        Ok(Box::pin(converse_event_stream(
            resp.bytes_stream(),
            cfg.model.clone(),
            name_map,
            "primary",
        )))
    }

    async fn embeddings(
        &self,
        _req: &EmbeddingsRequest,
        _cfg: &ResolvedEmbeddingConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        Err(ProviderError::Status {
            status: 501,
            message: "Bedrock Converse has no embeddings endpoint".to_string(),
            retryable: false,
        })
    }

    fn droppable_param(&self, err: &ProviderError) -> Option<String> {
        let ProviderError::Status {
            status: 400,
            message,
            ..
        } = err
        else {
            return None;
        };
        let detail = serde_json::from_str::<Value>(message)
            .ok()
            .and_then(|body| body.get("message")?.as_str().map(str::to_string))
            .unwrap_or_else(|| message.clone());
        super::param_rejection(&detail)
    }
}

impl BedrockConverseProvider {
    /// Chat with an explicit model ID (used for prefix retry).
    async fn chat_with_model(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
        model_id: &str,
    ) -> Result<ChatResponse, ProviderError> {
        let (body, name_map) = self.request_body(req, cfg)?;
        let url = format!("{}/model/{}/converse", self.base, model_id);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&cfg.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, text));
        }

        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        from_converse_response(&value, &cfg.model, &name_map)
    }

    /// Stream with an explicit model ID (used for prefix retry).
    async fn stream_with_model(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
        model_id: &str,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let (body, name_map) = self.request_body(req, cfg)?;
        let url = format!("{}/model/{}/converse-stream", self.base, model_id);

        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let tool_count = body
            .pointer("/toolConfig/tools")
            .and_then(|t| t.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        tracing::info!(
            target: "nasiko::llm_router::bedrock",
            model = %model_id,
            body_bytes = body_str.len(),
            tool_count,
            "bedrock converse-stream (prefix retry): outgoing request stats"
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&cfg.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(none)")
            .to_string();
        let amzn_error = resp
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        tracing::info!(
            target: "nasiko::llm_router::bedrock",
            %status,
            content_type = %content_type,
            amzn_error = %amzn_error,
            model = %model_id,
            "bedrock converse-stream (prefix retry): response status"
        );
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(
                target: "nasiko::llm_router::bedrock",
                %status,
                model = %model_id,
                body_preview = &text[..text.len().min(500)],
                "bedrock converse-stream (prefix retry): error body"
            );
            return Err(Self::status_error(status, text));
        }

        Ok(Box::pin(converse_event_stream(
            resp.bytes_stream(),
            cfg.model.clone(),
            name_map,
            "prefix-retry",
        )))
    }
}

/// Shared event-stream decoder for all Bedrock Converse streaming paths (primary,
/// prefix-retry). Handles text content, tool-use blocks,
/// exception frames, and terminal usage/finish events.
fn converse_event_stream(
    bytes_stream: impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    model: String,
    name_map: HashMap<String, String>,
    label: &'static str,
) -> BoxStream<'static, Result<ChatChunk, ProviderError>> {
    let stream = async_stream::stream! {
        futures::pin_mut!(bytes_stream);
        let mut buf = BytesMut::new();
        let mut id = String::new();
        let mut block_to_tool: HashMap<i64, i64> = HashMap::new();
        let mut tool_blocks_with_arguments: HashSet<i64> = HashSet::new();
        let mut next_tool_index: i64 = 0;
        let mut usage = None;
        let mut finish: Option<String> = None;
        let mut got_metadata = false;
        let mut first_chunk = true;

        while let Some(chunk) = bytes_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => { yield Err(ProviderError::Transport(e.to_string())); return; }
            };
            if first_chunk {
                first_chunk = false;
                // Its length only: the chunk is the start of the model's reply.
                tracing::info!(
                    target: "nasiko::llm_router::bedrock",
                    chunk_len = chunk.len(),
                    label,
                    "bedrock converse stream: first chunk received"
                );
            }
            buf.extend_from_slice(&chunk);

            while let Some(frame) = decode_event_stream_frame(&mut buf) {
                tracing::trace!(
                    target: "nasiko::llm_router::bedrock",
                    event_type = %frame.event_type,
                    exception_type = %frame.exception_type,
                    message_type = %frame.message_type,
                    payload_len = frame.payload.len(),
                    payload_preview = &frame.payload[..frame.payload.len().min(200)],
                    "bedrock event-stream frame decoded"
                );

                if frame.is_exception() {
                    let msg = format!(
                        "Bedrock stream exception ({}): {}",
                        frame.exception_type, frame.payload
                    );
                    tracing::error!(
                        target: "nasiko::llm_router::bedrock",
                        exception_type = %frame.exception_type,
                        payload = %frame.payload,
                        label,
                        "bedrock: received exception frame in event stream"
                    );
                    yield Err(ProviderError::Status {
                        status: 400,
                        message: msg,
                        retryable: false,
                    });
                    return;
                }

                let event: Value = match serde_json::from_str(&frame.payload) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            target: "nasiko::llm_router::bedrock",
                            event_type = %frame.event_type, error = %e,
                            "bedrock frame JSON parse failed"
                        );
                        continue;
                    }
                };

                match frame.event_type.as_str() {
                    "messageStart" => {
                        id = uuid::Uuid::new_v4().to_string();
                        yield Ok(delta_chunk(&id, &model, Delta {
                            role: Some("assistant".to_string()),
                            ..Delta::default()
                        }));
                    }
                    "contentBlockStart" => {
                        if let Some(start) = event.get("start").and_then(|s| s.get("toolUse")) {
                            let oa_index = next_tool_index;
                            next_tool_index += 1;
                            let block_idx = event.get("contentBlockIndex")
                                .and_then(|i| i.as_i64())
                                .unwrap_or(0);
                            block_to_tool.insert(block_idx, oa_index);
                            yield Ok(delta_chunk(&id, &model, Delta {
                                tool_calls: Some(vec![ToolCallDelta {
                                    index: oa_index,
                                    id: start.get("toolUseId").and_then(|v| v.as_str()).map(str::to_string),
                                    kind: Some("function".to_string()),
                                    function: Some(FunctionCallDelta {
                                        name: start.get("name")
                                            .and_then(|v| v.as_str())
                                            .map(|n| unsanitize_tool_name(n, &name_map)),
                                        arguments: Some(String::new()),
                                    }),
                                    ..ToolCallDelta::default()
                                }]),
                                ..Delta::default()
                            }));
                        }
                    }
                    "contentBlockDelta" => {
                        let block_idx = event.get("contentBlockIndex")
                            .and_then(|i| i.as_i64())
                            .unwrap_or(0);
                        if let Some(delta) = event.get("delta") {
                            if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                yield Ok(delta_chunk(&id, &model, Delta {
                                    content: Some(text.to_string()),
                                    ..Delta::default()
                                }));
                            } else if let Some(input) = delta.get("toolUse")
                                .and_then(|tu| tu.get("input"))
                                .and_then(|i| i.as_str())
                                && let Some(&oa_index) = block_to_tool.get(&block_idx)
                            {
                                tool_blocks_with_arguments.insert(block_idx);
                                yield Ok(delta_chunk(&id, &model, Delta {
                                    tool_calls: Some(vec![ToolCallDelta {
                                        index: oa_index,
                                        id: None,
                                        kind: None,
                                        function: Some(FunctionCallDelta {
                                            name: None,
                                            arguments: Some(input.to_string()),
                                        }),
                                        ..ToolCallDelta::default()
                                    }]),
                                    ..Delta::default()
                                }));
                            }
                        }
                    }
                    "contentBlockStop" => {
                        let block_idx = event.get("contentBlockIndex")
                            .and_then(|i| i.as_i64())
                            .unwrap_or(0);
                        if !tool_blocks_with_arguments.contains(&block_idx)
                            && let Some(&oa_index) = block_to_tool.get(&block_idx)
                        {
                            yield Ok(delta_chunk(&id, &model, Delta {
                                tool_calls: Some(vec![ToolCallDelta {
                                    index: oa_index,
                                    id: None,
                                    kind: None,
                                    function: Some(FunctionCallDelta {
                                        name: None,
                                        arguments: Some("{}".into()),
                                    }),
                                    ..ToolCallDelta::default()
                                }]),
                                ..Delta::default()
                            }));
                        }
                    }
                    "messageStop" => {
                        if let Some(sr) = event.get("stopReason").and_then(|s| s.as_str()) {
                            finish = Some(map_stop_reason(sr).to_string());
                        }
                    }
                    "metadata" => {
                        got_metadata = true;
                        if let Some(u) = event.get("usage") {
                            usage = Some(converse_usage(u));
                        }
                    }
                    other => {
                        tracing::debug!(
                            target: "nasiko::llm_router::bedrock",
                            event_type = other,
                            "bedrock: unrecognized event type — skipped"
                        );
                    }
                }
            }
        }

        tracing::info!(
            target: "nasiko::llm_router::bedrock",
            has_finish = finish.is_some(),
            got_metadata,
            ?usage,
            label,
            "bedrock converse stream ended — emitting terminal chunks"
        );
        if let Some(finish) = finish {
            yield Ok(finish_chunk(&id, &model, finish));
        }
        if let Some(usage) = usage {
            yield Ok(usage_chunk(&id, &model, usage));
        }
    };
    Box::pin(stream)
}

/// Converse reports fresh input separately from cache reads/writes.
fn converse_usage(value: &Value) -> Usage {
    let count = |key: &str| value.get(key).and_then(Value::as_i64).filter(|n| *n >= 0);
    let cache_creation = value
        .get("cacheDetails")
        .and_then(Value::as_array)
        .map(|details| {
            let tokens = |ttl: &str| {
                let counts: Vec<i64> = details
                    .iter()
                    .filter(|d| d["ttl"] == ttl)
                    .filter_map(|d| d["inputTokens"].as_i64().filter(|n| *n >= 0))
                    .collect();
                if counts.is_empty() {
                    None
                } else {
                    counts.into_iter().try_fold(0_i64, i64::checked_add)
                }
            };
            crate::ir::CacheCreationUsage {
                ephemeral_5m_input_tokens: tokens("5m"),
                ephemeral_1h_input_tokens: tokens("1h"),
            }
        });
    Usage {
        prompt_tokens: count("inputTokens"),
        completion_tokens: count("outputTokens"),
        total_tokens: count("totalTokens"),
        cache_read_input_tokens: count("cacheReadInputTokens"),
        cache_creation_input_tokens: count("cacheWriteInputTokens"),
        cache_creation,
        prompt_tokens_details: None,
    }
}

/// Whether a failed Bedrock call should be retried with a region-prefixed model ID.
fn should_retry_with_prefix(status: u16, body: &str, model_id: &str) -> bool {
    if status != 400 && status != 404 {
        return false;
    }
    // Already has a region prefix — retrying with the same prefix won't help.
    if model_id.starts_with("us.")
        || model_id.starts_with("eu.")
        || model_id.starts_with("ap.")
        || model_id.starts_with("global.")
    {
        return false;
    }
    body.contains("is not authorized")
        || body.contains("Could not resolve")
        || body.contains("invalid")
        || body.contains("AccessDeniedException")
        || body.contains("inference profile")
        || body.contains("on-demand throughput isn't supported")
}

// ── OpenAI → Bedrock Converse (request) ─────────────────────────────────────

/// A part the hub carries but Converse cannot, rendered onto this edge's error type.
fn unsupported(err: UnsupportedPart) -> ProviderError {
    ProviderError::Rejected(format!(
        "Bedrock does not accept this input: {}",
        err.into_message()
    ))
}

/// Converse names media by a **bare format token** (`png`, `pdf`, `mp4`) — never a mime
/// type. A `split('/')` guess is wrong often enough to matter (`image/jpeg` → `jpg`,
/// `video/quicktime` → `quicktime`) and the service answers with a validation error, so
/// every accepted type is listed explicitly.
fn converse_image_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpeg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// `video/quicktime` is the one that proves the point: a `split('/')` guess yields
/// "quicktime", and Converse only knows "mov".
fn converse_video_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "video/x-matroska" => Some("mkv"),
        "video/quicktime" => Some("mov"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "video/x-flv" => Some("flv"),
        "video/mpeg" => Some("mpeg"),
        "video/x-ms-wmv" => Some("wmv"),
        "video/3gpp" => Some("three_gp"),
        _ => None,
    }
}

fn converse_document_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "application/pdf" => Some("pdf"),
        "text/csv" => Some("csv"),
        "application/msword" => Some("doc"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some("docx"),
        "application/vnd.ms-excel" => Some("xls"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
        "text/html" => Some("html"),
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        _ => None,
    }
}

/// Converse requires `document.name` and validates it against
/// `[a-zA-Z0-9\s\-\(\)\[\]]+` — notably **no dots**, so a filename can never be passed
/// through as-is. Everything outside the class becomes a space, runs collapse, and an
/// empty result falls back to a constant so the required field is always populated.
fn sanitize_document_name(filename: Option<&str>) -> String {
    let swept: String = filename
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '(' | ')' | '[' | ']') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let collapsed = swept.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "document".to_string()
    } else {
        collapsed
    }
}

/// Converse also requires document names to be **unique within a request**, so a second
/// `report.pdf` (or a second unnamed document) gets a numeric suffix rather than a 400.
fn unique_document_name(filename: Option<&str>, used: &mut HashSet<String>) -> String {
    let base = sanitize_document_name(filename);
    let mut name = base.clone();
    let mut n = 2;
    while !used.insert(name.clone()) {
        name = format!("{base} {n}");
        n += 1;
    }
    name
}

/// Canonical part → Converse content block.
///
/// Converse takes media as inline bytes only (an S3 location is a different, bucket-
/// scoped shape we do not mint), so a URL source is refused here rather than sent as an
/// empty block. Converse has no audio input, so audio is refused by name.
fn converse_block(
    part: &ContentPart,
    doc_names: &mut HashSet<String>,
) -> Result<Value, UnsupportedPart> {
    match part {
        ContentPart::Text(text) => Ok(json!({ "text": text })),
        ContentPart::Image { source, .. } => {
            let (media_type, data) = source.inline().ok_or_else(|| {
                UnsupportedPart::new(
                    "unsupported content part type: 'image_url' with web URL (Bedrock requires base64 data URL)",
                )
            })?;
            let format = converse_image_format(media_type).ok_or_else(|| {
                UnsupportedPart::new(format!(
                    "unsupported image media type for Bedrock: '{media_type}'"
                ))
            })?;
            Ok(json!({
                "image": { "format": format, "source": { "bytes": data } },
            }))
        }
        ContentPart::File { source, filename } => {
            let (media_type, data) = source.inline().ok_or_else(|| {
                UnsupportedPart::new("unsupported file_data format (must be base64 data URL)")
            })?;
            let format = converse_document_format(media_type).ok_or_else(|| {
                UnsupportedPart::new(format!(
                    "unsupported document media type for Bedrock: '{media_type}'"
                ))
            })?;
            Ok(json!({
                "document": {
                    "format": format,
                    "name": unique_document_name(filename.as_deref(), doc_names),
                    "source": { "bytes": data },
                },
            }))
        }
        ContentPart::Audio { .. } => Err(UnsupportedPart::new(
            "unsupported content part type: 'input_audio'",
        )),
        ContentPart::Video { source } => {
            let (media_type, data) = source.inline().ok_or_else(|| {
                UnsupportedPart::new(
                    "unsupported content part type: 'video_url' with web URL (Bedrock requires base64 data URL)",
                )
            })?;
            let format = converse_video_format(media_type).ok_or_else(|| {
                UnsupportedPart::new(format!(
                    "unsupported video media type for Bedrock: '{media_type}'"
                ))
            })?;
            Ok(json!({
                "video": { "format": format, "source": { "bytes": data } },
            }))
        }
    }
}

/// A message's content → Converse content blocks.
///
/// An absent/unmodelled content value becomes a single empty text block, which is what
/// Converse expects for a turn that carries only tool calls.
fn converse_content(
    m: &Message,
    doc_names: &mut HashSet<String>,
) -> Result<Vec<Value>, ProviderError> {
    match m.content_view().map_err(unsupported)? {
        ContentView::Text(text) => Ok(vec![json!({ "text": text })]),
        ContentView::Parts(parts) => parts
            .iter()
            .map(|p| converse_block(p, doc_names).map_err(unsupported))
            .collect(),
        ContentView::Empty => Ok(vec![json!({ "text": "" })]),
    }
}

fn without_empty_text(blocks: Vec<Value>) -> Vec<Value> {
    let is_empty_text = |block: &Value| block.get("text").and_then(Value::as_str) == Some("");
    if !blocks
        .iter()
        .any(|block| block.get("cachePoint").is_none() && !is_empty_text(block))
    {
        return blocks;
    }
    let mut kept = Vec::with_capacity(blocks.len());
    let mut after_dropped_text = false;
    for block in blocks {
        if is_empty_text(&block) {
            after_dropped_text = true;
            continue;
        }
        let its_cache_point = after_dropped_text && block.get("cachePoint").is_some();
        after_dropped_text = false;
        if !its_cache_point {
            kept.push(block);
        }
    }
    kept
}

const BEDROCK_IMAGE_MAX_BYTES: u64 = 3_750_000;
const BEDROCK_DOCUMENT_MAX_BYTES: u64 = 4_500_000;
const BEDROCK_MAX_IMAGES: usize = 20;
const BEDROCK_MAX_DOCUMENTS: usize = 5;

fn check_media_limits(messages: &[Value]) -> Result<(), UnsupportedPart> {
    let mut images = 0;
    let mut documents = 0;
    let blocks = messages
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .flat_map(|block| {
            let nested = block["toolResult"]["content"].as_array();
            std::iter::once(block).chain(nested.into_iter().flatten())
        });
    for block in blocks {
        if let Some(data) = block["image"]["source"]["bytes"].as_str() {
            images += 1;
            if media::decoded_len(data) > BEDROCK_IMAGE_MAX_BYTES {
                return Err(UnsupportedPart::new("an image over 3.75 MB"));
            }
        }
        if let Some(data) = block["document"]["source"]["bytes"].as_str() {
            documents += 1;
            if media::decoded_len(data) > BEDROCK_DOCUMENT_MAX_BYTES {
                return Err(UnsupportedPart::new("a document over 4.5 MB"));
            }
        }
    }
    if images > BEDROCK_MAX_IMAGES {
        return Err(UnsupportedPart::new("more than 20 images in one request"));
    }
    if documents > BEDROCK_MAX_DOCUMENTS {
        return Err(UnsupportedPart::new("more than 5 documents in one request"));
    }
    Ok(())
}

/// Returns `(body, tool_name_map)` where `tool_name_map` maps sanitized names
/// back to the originals (only entries that changed).
fn to_converse_request_inner(
    req: &ChatRequest,
    cfg: &ResolvedConfig,
) -> Result<(Value, HashMap<String, String>), ProviderError> {
    let caching = CacheSupport::of(&cfg.model);
    let mut system_parts: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();
    // Converse requires document names to be unique across the whole request.
    let mut doc_names: HashSet<String> = HashSet::new();

    for m in &req.messages {
        match m.role.as_str() {
            "system" => {
                // Brevity's directive goes after every cache point, so a turn that skips it still
                // matches this turn's cached prefix; in `system` it would sit ahead of them all.
                let placed = crate::brevity::is_directive(m)
                    && pending_tool_results.is_empty()
                    && end_user_turn_with(messages.last_mut(), crate::brevity::DIRECTIVE, caching);
                if placed {
                    continue;
                }
                match caching.marked_text_blocks(m) {
                    Some(blocks) => system_parts.extend(blocks),
                    None => {
                        if let Some(t) = m.text() {
                            system_parts.push(json!({ "text": t }));
                        }
                    }
                }
            }
            "tool" => {
                pending_tool_results.push(json!({
                    "toolResult": {
                        "toolUseId": m.tool_call_id.clone().unwrap_or_default(),
                        "content": without_empty_text(converse_content(m, &mut doc_names)?),
                    }
                }));
                pending_tool_results.extend(caching.after_tool_block(m.last_cache_marker()));
            }
            "assistant" => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                messages.push(assistant_to_converse(m, caching));
            }
            _ => {
                // "user" and any unexpected role → user turn.
                flush_tool_results(&mut pending_tool_results, &mut messages);
                let content = without_empty_text(
                    caching.with_cache_points(m, converse_content(m, &mut doc_names)?),
                );
                messages.push(json!({ "role": "user", "content": content }));
            }
        }
    }
    flush_tool_results(&mut pending_tool_results, &mut messages);
    check_media_limits(&messages).map_err(unsupported)?;

    let max_tokens = cfg
        .max_tokens
        .or(req.max_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS);

    let mut body = json!({
        "messages": messages,
        "inferenceConfig": {
            "maxTokens": max_tokens,
        },
    });

    if let Some(t) = cfg.temperature.or(req.temperature) {
        body["inferenceConfig"]["temperature"] = json!(t);
    }
    // OpenAI `stop` is a string or an array of strings, and not a named IR field.
    let stop: Vec<&str> = match req.extra.get("stop") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !stop.is_empty() {
        body["inferenceConfig"]["stopSequences"] = json!(stop);
    }
    if !system_parts.is_empty() {
        body["system"] = json!(system_parts);
    }
    // Converse has no "none" tool choice, so honour it by sending no tools. Bedrock rejects
    // toolUse/toolResult blocks without a `toolConfig`, so with tool history the tools stay.
    let has_tool_history = req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.as_ref().is_some_and(|c| !c.is_empty()));
    let tools_off =
        req.tool_choice.as_ref().and_then(Value::as_str) == Some("none") && !has_tool_history;
    let mut name_map = HashMap::new();
    if let Some(tools) = req.tools.as_ref().filter(|_| !tools_off) {
        name_map = build_tool_name_map(tools);
        let mut translated: Vec<Value> = Vec::new();
        for t in tools {
            translated.push(tool_to_converse(t));
            translated.extend(caching.after_tool_block(t.extra.get(CACHE_CONTROL)));
        }
        if !translated.is_empty() {
            body["toolConfig"] = json!({ "tools": translated });
            if let Some(choice) = req.tool_choice.as_ref().and_then(tool_choice_to_converse) {
                body["toolConfig"]["toolChoice"] = choice;
            }
        }
    }
    Ok((body, name_map))
}

fn flush_tool_results(pending: &mut Vec<Value>, messages: &mut Vec<Value>) {
    if !pending.is_empty() {
        messages.push(json!({ "role": "user", "content": std::mem::take(pending) }));
    }
}

/// Add `text` as a block at the end of `turn` when it is a user turn; `false` when it is not,
/// or there is none. On Claude the text before it loses its trailing whitespace, unless that
/// would empty it: the next request ends the turn at that text, and with trailing whitespace it
/// missed the prefix this request cached there (checked live, Nov 2026). Nova read it either way.
fn end_user_turn_with(turn: Option<&mut Value>, text: &str, caching: CacheSupport) -> bool {
    match turn
        .filter(|t| t["role"] == "user")
        .and_then(|t| t["content"].as_array_mut())
    {
        Some(content) => {
            if matches!(caching, CacheSupport::Everywhere { .. })
                && let Some(last) = content
                    .iter_mut()
                    .rev()
                    .find(|b| b.get("cachePoint").is_none())
                && let Some(before) = last["text"].as_str()
                && before.trim_end().len() != before.len()
                && !before.trim_end().is_empty()
            {
                last["text"] = json!(before.trim_end());
            }
            content.push(json!({ "text": text }));
            true
        }
        None => false,
    }
}

/// Where a model accepts a `cachePoint`, Bedrock's equivalent of `cache_control`: a block that
/// ends the cached prefix after the content before it. A cache point a model does not accept
/// fails the whole request, so these follow what Converse accepted live (Oct 2026).
#[derive(Clone, Copy, PartialEq)]
enum CacheSupport {
    /// Claude: after any block and in the tool list; with the agent's TTL when `ttl`.
    Everywhere {
        ttl: bool,
    },
    /// Nova: only right after a text block; no TTL ("only supported for Anthropic models").
    AfterText,
    None,
}

/// Claude models from before the 1-hour TTL. Opus 4.1 rejects a cache point carrying one
/// ("ttl: Extra inputs are not permitted"); the rest are past end of life on Bedrock.
const CLAUDE_WITHOUT_TTL: [&str; 4] = [
    "claude-3",
    "claude-opus-4-2025",
    "claude-sonnet-4-2025",
    "claude-opus-4-1",
];

/// Claude models Bedrock serves without prompt caching. The router adds no cache points
/// there; an agent's own marker still passes through as it did before the router added any.
const CLAUDE_WITHOUT_CACHING: [&str; 4] = [
    "claude-3-haiku",
    "claude-3-sonnet",
    "claude-3-opus",
    "claude-3-5-sonnet",
];

impl CacheSupport {
    fn of(model: &str) -> Self {
        if model.contains("anthropic.claude") {
            Self::Everywhere {
                ttl: !CLAUDE_WITHOUT_TTL.iter().any(|old| model.contains(old)),
            }
        } else if model.contains("amazon.nova") {
            Self::AfterText
        } else {
            Self::None
        }
    }

    /// Where the router may add cache points of its own: as [`of`](Self::of), except no
    /// Claude in [`CLAUDE_WITHOUT_CACHING`]. A cache point there would fail the request,
    /// and nothing retries without it.
    fn for_router(model: &str) -> Self {
        if CLAUDE_WITHOUT_CACHING.iter().any(|old| model.contains(old)) {
            Self::None
        } else {
            Self::of(model)
        }
    }

    /// One `{text}` block per text part, with a cache point after each marked one; `None` when
    /// no part carries a marker or the model caches nothing.
    fn marked_text_blocks(self, m: &Message) -> Option<Vec<Value>> {
        if self == Self::None {
            return None;
        }
        let mut blocks = Vec::new();
        for part in m.marked_text_parts()? {
            blocks.push(json!({ "text": part["text"] }));
            if let Some(marker) = part.get(CACHE_CONTROL) {
                blocks.push(self.cache_point(marker));
            }
        }
        Some(blocks)
    }

    /// `blocks`, the Converse rendering of `m`'s parts, with a cache point after each block whose
    /// part carries a marker and that the model accepts one after.
    fn with_cache_points(self, m: &Message, blocks: Vec<Value>) -> Vec<Value> {
        let Some(Value::Array(parts)) = &m.content else {
            return blocks;
        };
        let mut out = Vec::with_capacity(blocks.len());
        // `content_view` projects the raw array one part to one part, so the two line up.
        for (block, part) in blocks.into_iter().zip(parts) {
            let point = part
                .get(CACHE_CONTROL)
                .filter(|_| self.accepts_point_after(&block))
                .map(|marker| self.cache_point(marker));
            out.push(block);
            out.extend(point);
        }
        out
    }

    fn accepts_point_after(self, block: &Value) -> bool {
        match self {
            Self::Everywhere { .. } => true,
            Self::AfterText => block.get("text").is_some(),
            Self::None => false,
        }
    }

    /// The cache point to follow a tool call, tool result or tool definition carrying `marker`.
    fn after_tool_block(self, marker: Option<&Value>) -> Option<Value> {
        marker
            .filter(|_| matches!(self, Self::Everywhere { .. }))
            .map(|m| self.cache_point(m))
    }

    fn cache_point(self, marker: &Value) -> Value {
        let mut point = json!({ "cachePoint": { "type": "default" } });
        if self == (Self::Everywhere { ttl: true })
            && let Some(ttl) = marker.get("ttl")
        {
            point["cachePoint"]["ttl"] = ttl.clone();
        }
        point
    }
}

/// Add the router's own cache points in the slots the agent left free: first at the end of the
/// conversation, on Nova again where the previous request put that point, then after `system`
/// (tools come before it, so that point covers them too). Each is a 5-minute point. Only where
/// the model accepts one: after any block on Claude, only after text on Nova, so on Nova the end
/// of a tool loop is its last assistant text.
fn add_cache_points(body: &mut Value, caching: CacheSupport) {
    if caching == CacheSupport::None {
        return;
    }
    let mut free = MAX_CACHE_POINTS.saturating_sub(count_cache_points(body));
    // A 5-minute point may not come before a 1-hour one, and `system` comes before every turn.
    let turns_hold_long_point = turn_blocks(body).any(|block| block["cachePoint"]["ttl"] == "1h");

    if free > 0
        && let Some(slot) = cache_point_slot(turns(body), caching)
    {
        insert_cache_point(body, slot);
        free -= 1;
    }
    // Claude finds the previous request's cached prefix by looking back from this request's
    // point. Nova reads only at a point the request repeats, so it gets the previous request's
    // end point again: the same slot, found in the turns before the last answer.
    if free > 0
        && caching == CacheSupport::AfterText
        && let Some(answer) = turns(body).iter().rposition(|t| t["role"] == "assistant")
        && let Some(slot) = cache_point_slot(&turns(body)[..answer], caching)
    {
        insert_cache_point(body, slot);
        free -= 1;
    }
    if free > 0
        && !turns_hold_long_point
        && let Some(system) = body.get_mut("system").and_then(Value::as_array_mut)
        && system
            .last()
            .and_then(|block| block["text"].as_str())
            .is_some_and(|t| !t.is_empty() && !t.ends_with(crate::brevity::DIRECTIVE))
    {
        system.push(json!({ "cachePoint": { "type": "default" } }));
    }
}

fn turns(body: &Value) -> &[Value] {
    body["messages"].as_array().map_or(&[], Vec::as_slice)
}

/// Put a cache point at `(turn, index)` in the conversation.
fn insert_cache_point(body: &mut Value, (turn, at): (usize, usize)) {
    if let Some(content) = body["messages"][turn]["content"].as_array_mut() {
        content.insert(at, json!({ "cachePoint": { "type": "default" } }));
    }
}

fn count_cache_points(body: &Value) -> usize {
    [&body["system"], &body["toolConfig"]["tools"]]
        .into_iter()
        .filter_map(Value::as_array)
        .flatten()
        .chain(turn_blocks(body))
        .filter(|block| block.get("cachePoint").is_some())
        .count()
}

/// Every content block of every turn.
fn turn_blocks(body: &Value) -> impl Iterator<Item = &Value> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|turn| turn["content"].as_array())
        .flatten()
}

/// Where a cache point ending `turns` goes, as `(turn, index)`: after the last block the model
/// accepts one after, walking back past brevity's directive. On Claude that is in the last turn.
/// Nova takes one only after text, so it walks back past turns of tool calls and results to the
/// last text, which may sit ahead of an assistant's tool call. `None` when a point already ends
/// that stretch or no block qualifies.
fn cache_point_slot(turns: &[Value], caching: CacheSupport) -> Option<(usize, usize)> {
    for (t, turn) in turns.iter().enumerate().rev() {
        let blocks = turn["content"].as_array().map_or(&[][..], Vec::as_slice);
        for (i, block) in blocks.iter().enumerate().rev() {
            if block.get("cachePoint").is_some() {
                return None;
            }
            let text = block["text"].as_str();
            if text == Some(crate::brevity::DIRECTIVE) {
                continue;
            }
            let accepted = match caching {
                CacheSupport::Everywhere { .. } => text.is_none_or(|t| !t.is_empty()),
                CacheSupport::AfterText => text.is_some_and(|t| !t.is_empty()),
                CacheSupport::None => false,
            };
            if accepted {
                return Some((t, i + 1));
            }
        }
    }
    None
}

fn assistant_to_converse(m: &Message, caching: CacheSupport) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    match caching.marked_text_blocks(m) {
        Some(text_blocks) => blocks.extend(text_blocks),
        None => {
            if let Some(t) = m.text()
                && !t.is_empty()
            {
                blocks.push(json!({ "text": t }));
            }
        }
    }
    if let Some(tool_calls) = &m.tool_calls {
        for tc in tool_calls {
            let input: Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
            blocks.push(json!({
                "toolUse": {
                    "toolUseId": tc.id,
                    "name": sanitize_tool_name(&tc.function.name),
                    "input": input,
                }
            }));
            blocks.extend(caching.after_tool_block(tc.extra.get(CACHE_CONTROL)));
        }
    }
    if blocks.is_empty() {
        blocks.push(json!({ "text": "" }));
    }
    json!({ "role": "assistant", "content": blocks })
}

/// Sanitize a tool name for Bedrock Converse, which requires `^[a-zA-Z][a-zA-Z0-9_]*$`.
/// Replaces hyphens and other invalid characters with underscores, and ensures the
/// name starts with a letter.
fn sanitize_tool_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    // Ensure starts with a letter.
    if out.is_empty() || !out.as_bytes()[0].is_ascii_alphabetic() {
        out.insert(0, 't');
    }
    out
}

/// Build a mapping of sanitized → original tool names. Only includes entries
/// where the name actually changed.
fn build_tool_name_map(tools: &[ToolDef]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for t in tools {
        let sanitized = sanitize_tool_name(&t.function.name);
        if sanitized != t.function.name {
            map.insert(sanitized, t.function.name.clone());
        }
    }
    map
}

/// Restore the original tool name from the sanitized→original map, falling back
/// to the (possibly sanitized) name if no mapping exists.
fn unsanitize_tool_name(name: &str, map: &HashMap<String, String>) -> String {
    map.get(name).cloned().unwrap_or_else(|| name.to_string())
}

fn tool_to_converse(t: &ToolDef) -> Value {
    let sanitized_name = sanitize_tool_name(&t.function.name);
    let schema = t
        .function
        .parameters
        .clone()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let mut spec = json!({
        "toolSpec": {
            "name": sanitized_name,
            "inputSchema": { "json": schema },
        }
    });
    if let Some(desc) = &t.function.description {
        spec["toolSpec"]["description"] = json!(desc);
    }
    spec
}

fn tool_choice_to_converse(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(s) => match s.as_str() {
            "required" => Some(json!({ "any": {} })),
            "none" => None,
            _ => Some(json!({ "auto": {} })),
        },
        Value::Object(o) => o
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .map(|name| json!({ "tool": { "name": sanitize_tool_name(name) } })),
        _ => None,
    }
}

// ── Bedrock Converse → OpenAI (response) ────────────────────────────────────

fn from_converse_response(
    body: &Value,
    model: &str,
    name_map: &HashMap<String, String>,
) -> Result<ChatResponse, ProviderError> {
    let message = body
        .pointer("/output/message")
        .ok_or_else(|| ProviderError::Parse("Converse response has no output.message".into()))?;

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(blocks) = message.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                text.push_str(t);
            }
            if let Some(tu) = block.get("toolUse") {
                let raw_name = tu.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                tool_calls.push(ToolCall {
                    id: tu
                        .get("toolUseId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name: unsanitize_tool_name(raw_name, name_map),
                        arguments: tu
                            .get("input")
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                    extra: Map::new(),
                });
            }
        }
    }

    let content = if tool_calls.is_empty() {
        Some(Value::String(text))
    } else if text.is_empty() {
        Some(Value::Null)
    } else {
        Some(Value::String(text))
    };

    let stop_reason = body
        .get("stopReason")
        .and_then(|s| s.as_str())
        .unwrap_or("end_turn");
    let finish_reason = map_stop_reason(stop_reason);

    let usage = body.get("usage").map(converse_usage);

    let id = uuid::Uuid::new_v4().to_string();
    Ok(ChatResponse {
        id: format!("chatcmpl-{id}"),
        object: "chat.completion".to_string(),
        created: Some(now_unix()),
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".to_string(),
                content,
                name: None,
                tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
                tool_call_id: None,
                extra: Map::new(),
            },
            finish_reason: Some(finish_reason.to_string()),
        }],
        usage,
        extra: Map::new(),
    })
}

fn map_stop_reason(reason: &str) -> &'static str {
    match reason {
        "end_turn" | "stop_sequence" => "stop",
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        "content_filtered" => "content_filter",
        _ => "stop",
    }
}

// ── AWS Event Stream binary decoder ─────────────────────────────────────────
//
// AWS event-stream frame layout:
//   [4 bytes total_length] [4 bytes headers_length] [4 bytes prelude_crc]
//   [headers...] [payload...] [4 bytes message_crc]
//
// Headers are length-prefixed key-value pairs. We extract `:event-type` and
// the JSON payload. The CRC checks are skipped (the HTTP transport is already
// integrity-checked).

/// Decoded AWS event-stream frame.
struct EventStreamFrame {
    /// `:event-type` header value (e.g. "messageStart", "contentBlockDelta").
    /// Empty for exception frames.
    event_type: String,
    /// `:exception-type` header value (e.g. "ValidationException").
    /// Empty for normal event frames.
    exception_type: String,
    /// `:message-type` header value ("event" or "exception").
    message_type: String,
    /// JSON payload.
    payload: String,
}

impl EventStreamFrame {
    fn is_exception(&self) -> bool {
        self.message_type == "exception" || !self.exception_type.is_empty()
    }
}

/// Maximum reasonable event-stream frame size (16 MiB). Real Bedrock frames are
/// a few KB; a `total_length` larger than this means the bytes are not valid
/// event-stream data (e.g. a JSON or XML error body was returned instead).
const MAX_EVENT_STREAM_FRAME: usize = 16 * 1024 * 1024;

/// Try to decode one complete event-stream frame from `buf`. Returns the
/// decoded frame and advances `buf` past it. Returns `None` if the buffer
/// doesn't contain a complete frame yet. Returns an error-sentinel frame if
/// the prelude indicates the data is not valid event-stream binary.
fn decode_event_stream_frame(buf: &mut BytesMut) -> Option<EventStreamFrame> {
    if buf.len() < 12 {
        return None; // need at least the prelude
    }

    let total_length = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

    // Sanity check: a frame larger than MAX_EVENT_STREAM_FRAME almost certainly
    // means the response is not event-stream binary (e.g. a JSON/XML error page
    // whose first 4 bytes decoded to a huge u32). Surface it as a synthetic
    // exception so the stream fails fast instead of waiting forever.
    if !(16..=MAX_EVENT_STREAM_FRAME).contains(&total_length) {
        let preview = String::from_utf8_lossy(&buf[..buf.len().min(512)]).to_string();
        tracing::error!(
            target: "nasiko::llm_router::bedrock",
            total_length,
            buf_len = buf.len(),
            preview = %preview,
            "bedrock: event-stream frame has implausible length — response is likely \
             not event-stream binary (JSON/XML error?)"
        );
        // Drain the buffer so the caller stops looping.
        buf.clear();
        return Some(EventStreamFrame {
            event_type: String::new(),
            exception_type: "InvalidEventStream".to_string(),
            message_type: "exception".to_string(),
            payload: format!("Response is not valid event-stream binary (first bytes: {preview})"),
        });
    }

    if buf.len() < total_length {
        return None; // incomplete frame
    }

    let headers_length = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    // Skip prelude (12 bytes: total_len + headers_len + prelude_crc).
    let headers_start = 12;
    let headers_end = headers_start + headers_length;
    let payload_end = total_length - 4; // last 4 bytes are message CRC

    // Parse headers to find :event-type, :exception-type, :message-type.
    let mut event_type = String::new();
    let mut exception_type = String::new();
    let mut message_type = String::new();
    let headers_bytes = &buf[headers_start..headers_end];
    let mut pos = 0;
    while pos < headers_bytes.len() {
        let name_len = headers_bytes[pos] as usize;
        pos += 1;
        if pos + name_len > headers_bytes.len() {
            break;
        }
        let name = std::str::from_utf8(&headers_bytes[pos..pos + name_len]).unwrap_or("");
        pos += name_len;
        if pos >= headers_bytes.len() {
            break;
        }
        let value_type = headers_bytes[pos];
        pos += 1;
        match value_type {
            7 => {
                // String type: 2-byte big-endian length + string bytes.
                if pos + 2 > headers_bytes.len() {
                    break;
                }
                let val_len =
                    u16::from_be_bytes([headers_bytes[pos], headers_bytes[pos + 1]]) as usize;
                pos += 2;
                if pos + val_len > headers_bytes.len() {
                    break;
                }
                let val = std::str::from_utf8(&headers_bytes[pos..pos + val_len]).unwrap_or("");
                pos += val_len;
                match name {
                    ":event-type" => event_type = val.to_string(),
                    ":exception-type" => exception_type = val.to_string(),
                    ":message-type" => message_type = val.to_string(),
                    _ => {}
                }
            }
            _ => {
                // Unknown header type — can't parse further; bail.
                break;
            }
        }
    }

    // Extract payload.
    let payload = if headers_end < payload_end {
        std::str::from_utf8(&buf[headers_end..payload_end])
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };

    // Advance buffer past this frame.
    buf.advance(total_length);

    Some(EventStreamFrame {
        event_type,
        exception_type,
        message_type,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request as translated, before the router adds any cache point.
    fn to_converse_request(
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<(Value, HashMap<String, String>), ProviderError> {
        to_converse_request_inner(req, cfg)
    }

    fn resolved(model: &str) -> ResolvedConfig {
        ResolvedConfig {
            agent_id: String::new(),
            provider: "bedrock".into(),
            model: model.into(),
            litellm_model: format!("bedrock/{model}"),
            api_key: "test-key".into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        }
    }

    #[tokio::test]
    #[ignore = "two paid Bedrock requests; requires explicit live verification authorization"]
    async fn live_usage_survives_converse_and_anthropic_adapters() {
        use crate::inbound::{InboundParser, anthropic::AnthropicInbound};
        let db_url = std::env::var("DATABASE_URL").unwrap();
        let parsed = reqwest::Url::parse(&db_url).unwrap();
        assert!(matches!(parsed.host_str(), Some("localhost" | "127.0.0.1")));
        let db = sqlx::PgPool::connect(&db_url).await.unwrap();
        let (base, encrypted): (String, String) = sqlx::query_as("SELECT base_url,encrypted_api_key FROM custom_providers WHERE label='aws-bedrock' AND deleted_at IS NULL")
            .fetch_one(&db).await.unwrap();
        let key = nasiko_secrets::SecretsCrypto::for_platform_settings()
            .decrypt(&encrypted)
            .unwrap();
        db.close().await;
        let mut config = resolved("us.openai.gpt-6-astra");
        config.max_tokens = Some(16);
        let request: ChatRequest = serde_json::from_value(
            json!({"messages":[{"role":"user","content":"Reply with just OK."}]}),
        )
        .unwrap();
        let (body, names) = to_converse_request(&request, &config).unwrap();
        let http = reqwest::Client::new();
        for surface in ["converse", "converse-stream"] {
            let response = http
                .post(format!("{base}/model/{}/{surface}", config.model))
                .bearer_auth(&key)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap();
                panic!("Bedrock status {status}: {body}");
            }
            if surface == "converse" {
                let raw: Value = response.json().await.unwrap();
                let mapped = from_converse_response(&raw, &config.model, &names).unwrap();
                let wire = AnthropicInbound.render_chat_response(mapped);
                println!(
                    "buffered raw usage={} rendered usage={}",
                    raw["usage"], wire["usage"]
                );
                assert_eq!(raw["usage"]["inputTokens"], wire["usage"]["input_tokens"]);
                assert_eq!(raw["usage"]["outputTokens"], wire["usage"]["output_tokens"]);
                assert_eq!(
                    raw["usage"]["cacheReadInputTokens"],
                    wire["usage"]["cache_read_input_tokens"]
                );
                assert_eq!(
                    raw["usage"]["cacheWriteInputTokens"],
                    wire["usage"]["cache_creation_input_tokens"]
                );
            } else {
                let bytes = response.bytes().await.unwrap();
                let mut buffer = BytesMut::from(bytes.as_ref());
                let mut raw = Value::Null;
                while let Some(frame) = decode_event_stream_frame(&mut buffer) {
                    assert!(!frame.is_exception(), "upstream exception");
                    if frame.event_type == "metadata" {
                        raw =
                            serde_json::from_str::<Value>(&frame.payload).unwrap()["usage"].clone();
                    }
                }
                let input = futures::stream::iter(vec![Ok::<_, reqwest::Error>(bytes)]);
                let mut chunks = converse_event_stream(
                    Box::pin(input),
                    config.model.clone(),
                    names.clone(),
                    "usage-verification",
                );
                let mut renderer = AnthropicInbound.chat_stream_renderer();
                while let Some(chunk) = chunks.next().await {
                    renderer.render(chunk.unwrap());
                }
                let frames = renderer.finish();
                let final_event: Value = serde_json::from_str(
                    frames
                        .iter()
                        .find(|f| f.contains("event: message_delta"))
                        .unwrap()
                        .lines()
                        .find_map(|l| l.strip_prefix("data: "))
                        .unwrap(),
                )
                .unwrap();
                let wire = &final_event["usage"];
                println!("stream raw usage={raw} rendered usage={wire}");
                assert_eq!(raw["inputTokens"], wire["input_tokens"]);
                assert_eq!(raw["outputTokens"], wire["output_tokens"]);
                assert_eq!(raw["cacheReadInputTokens"], wire["cache_read_input_tokens"]);
                assert_eq!(
                    raw["cacheWriteInputTokens"],
                    wire["cache_creation_input_tokens"]
                );
            }
        }
    }

    /// An Anthropic-SDK agent's request with a cache marker on a system block, a tool, a tool
    /// result and a user text block, parsed as the router parses it.
    fn marked_anthropic_request() -> ChatRequest {
        use crate::inbound::{AnthropicInbound, InboundParser};
        let ephemeral = json!({ "type": "ephemeral" });
        AnthropicInbound
            .parse_chat(json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "system": [
                    { "type": "text", "text": "You are a coding agent." },
                    { "type": "text", "text": "Repository map.", "cache_control": ephemeral }
                ],
                "tools": [
                    { "name": "read", "input_schema": { "type": "object" } },
                    { "name": "edit", "input_schema": { "type": "object" }, "cache_control": ephemeral }
                ],
                "messages": [
                    { "role": "user", "content": "Fix the failing test." },
                    { "role": "assistant", "content": [
                        { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {} }
                    ]},
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}", "cache_control": ephemeral }
                    ]},
                    { "role": "user", "content": [
                        { "type": "text", "text": "Also run clippy." },
                        { "type": "text", "text": "Then stop.", "cache_control": ephemeral }
                    ]}
                ]
            }))
            .unwrap()
    }

    fn cache_point() -> Value {
        json!({ "cachePoint": { "type": "default" } })
    }

    fn converse_rejection(parts: Vec<Value>) -> Option<String> {
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": parts }]
        }))
        .unwrap();
        match to_converse_request(&req, &resolved("us.anthropic.claude-haiku-5-5")) {
            Ok(_) => None,
            Err(ProviderError::Rejected(message)) => Some(message),
            Err(other) => panic!("expected Ok or a by-name refusal, got {other:?}"),
        }
    }

    fn png_of(decoded_bytes: usize) -> Value {
        let data = "A".repeat(decoded_bytes.div_ceil(3) * 4);
        json!({ "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{data}") } })
    }

    fn pdf_of(decoded_bytes: usize, name: &str) -> Value {
        let data = "A".repeat(decoded_bytes.div_ceil(3) * 4);
        json!({ "type": "file", "file": {
            "file_data": format!("data:application/pdf;base64,{data}"), "filename": name
        }})
    }

    #[test]
    fn bedrock_media_over_its_size_limits_is_refused_before_sending() {
        assert_eq!(converse_rejection(vec![png_of(3_750_000)]), None);
        let message = converse_rejection(vec![png_of(3_750_003)]).unwrap();
        assert!(
            message.starts_with("Bedrock does not accept this input"),
            "{message}"
        );
        assert!(message.contains("3.75 MB"), "{message}");

        assert_eq!(converse_rejection(vec![pdf_of(4_500_000, "a.pdf")]), None);
        let message = converse_rejection(vec![pdf_of(4_500_003, "a.pdf")]).unwrap();
        assert!(message.contains("4.5 MB"), "{message}");
    }

    #[test]
    fn bedrock_media_over_its_count_limits_is_refused_before_sending() {
        assert_eq!(converse_rejection(vec![png_of(3); 20]), None);
        let message = converse_rejection(vec![png_of(3); 21]).unwrap();
        assert!(message.contains("20 images"), "{message}");

        let docs = |n: usize| {
            (0..n)
                .map(|i| pdf_of(3, &format!("d{i}.pdf")))
                .collect::<Vec<_>>()
        };
        assert_eq!(converse_rejection(docs(5)), None);
        let message = converse_rejection(docs(6)).unwrap();
        assert!(message.contains("5 documents"), "{message}");
    }

    #[test]
    fn a_cache_point_follows_a_marked_image_only_where_the_model_accepts_one() {
        let ephemeral = json!({ "type": "ephemeral" });
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": [
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" }, "cache_control": ephemeral },
                { "type": "text", "text": "What is this?", "cache_control": ephemeral }
            ]}]
        }))
        .unwrap();
        let shape = |model: &str| -> Vec<String> {
            let (body, _) = to_converse_request(&req, &resolved(model)).unwrap();
            body["messages"][0]["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|block| block.as_object().unwrap().keys().next().unwrap().clone())
                .collect()
        };

        assert_eq!(
            shape("us.anthropic.claude-haiku-5-5"),
            ["image", "cachePoint", "text", "cachePoint"]
        );
        // Nova takes a cache point only right after text.
        assert_eq!(
            shape("us.amazon.nova-pro-v1:0"),
            ["image", "text", "cachePoint"]
        );
    }

    #[test]
    fn an_anthropic_agents_cache_markers_become_cache_points_on_claude() {
        let (body, _) = to_converse_request(
            &marked_anthropic_request(),
            &resolved("us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
        )
        .unwrap();

        assert_eq!(
            body["system"],
            json!([{ "text": "You are a coding agent." }, { "text": "Repository map." }, cache_point()])
        );
        let tools = body["toolConfig"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[1]["toolSpec"]["name"], "edit");
        assert_eq!(tools[2], cache_point());
        assert_eq!(body["messages"][2]["content"][1], cache_point());
        assert_eq!(
            body["messages"][3]["content"],
            json!([{ "text": "Also run clippy." }, { "text": "Then stop." }, cache_point()])
        );
        assert!(!body.to_string().contains("cache_control"));
    }

    #[test]
    fn nova_gets_cache_points_only_after_text() {
        // Nova rejects the whole request for a cache point after a tool call, a tool result or
        // in the tool list, so the marked tool and tool result go uncached there.
        let (body, _) = to_converse_request(
            &marked_anthropic_request(),
            &resolved("us.amazon.nova-pro-v1:0"),
        )
        .unwrap();

        assert_eq!(
            body["system"].as_array().unwrap().last(),
            Some(&cache_point())
        );
        assert_eq!(
            body["messages"][3]["content"].as_array().unwrap().last(),
            Some(&cache_point())
        );
        assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 1);
        let tools = body["toolConfig"]["tools"].as_array().unwrap();
        assert!(!tools.contains(&cache_point()));
    }

    #[test]
    fn a_one_hour_marker_keeps_its_ttl_on_claude_but_not_on_nova() {
        let mut req = marked_anthropic_request();
        req.messages[0].content = Some(json!([
            { "type": "text", "text": "You are a coding agent.", "cache_control": { "type": "ephemeral", "ttl": "1h" } }
        ]));
        let one_hour = json!({ "cachePoint": { "type": "default", "ttl": "1h" } });

        let (claude, _) =
            to_converse_request(&req, &resolved("us.anthropic.claude-haiku-5-5")).unwrap();
        let (nova, _) = to_converse_request(&req, &resolved("us.amazon.nova-pro-v1:0")).unwrap();

        assert_eq!(claude["system"][1], one_hour);
        assert_eq!(nova["system"][1], cache_point());
    }

    #[test]
    fn claude_from_before_the_one_hour_ttl_gets_a_cache_point_without_it() {
        // Opus 4.1 rejects the whole request for a TTL, but caches for 5 minutes without one.
        let mut req = marked_anthropic_request();
        req.messages[0].content = Some(json!([
            { "type": "text", "text": "You are a coding agent.", "cache_control": { "type": "ephemeral", "ttl": "1h" } }
        ]));

        let (body, _) = to_converse_request(
            &req,
            &resolved("us.anthropic.claude-opus-4-1-20250805-v1:0"),
        )
        .unwrap();

        assert_eq!(body["system"][1], cache_point());
    }

    /// Brevity's seam, run as the handler runs it.
    fn with_brevity(mut req: ChatRequest) -> ChatRequest {
        let cfg = crate::config::GatewayConfig {
            brevity_enabled: true,
            brevity_min_bytes: 0,
            ..Default::default()
        };
        let resolved = ResolvedConfig {
            compress_enabled: true,
            ..resolved("unused")
        };
        crate::brevity::apply(&mut req, &cfg, &resolved, None).unwrap();
        req
    }

    #[test]
    fn brevitys_directive_follows_the_last_cache_point_on_claude_and_nova() {
        // In `system` it would change the cached prefix of every turn that applies or skips it.
        let directive = json!({ "text": crate::brevity::DIRECTIVE });
        for model in ["us.anthropic.claude-haiku-5-5", "us.amazon.nova-pro-v1:0"] {
            let (without, _) =
                to_converse_request(&marked_anthropic_request(), &resolved(model)).unwrap();
            let (with, _) =
                to_converse_request(&with_brevity(marked_anthropic_request()), &resolved(model))
                    .unwrap();

            assert_eq!(with["system"], without["system"], "{model}");
            assert_eq!(with["toolConfig"], without["toolConfig"], "{model}");
            let mut last_turn = without["messages"][3]["content"].clone();
            last_turn.as_array_mut().unwrap().push(directive.clone());
            assert_eq!(with["messages"][3]["content"], last_turn, "{model}");
            for turn in 0..3 {
                assert_eq!(with["messages"][turn], without["messages"][turn], "{model}");
            }
        }
    }

    #[test]
    fn on_claude_the_text_ahead_of_brevitys_directive_loses_its_trailing_whitespace() {
        // The next turn ends at that text; with the whitespace it missed this turn's cache live.
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "Fix the failing test.\n" }]
        }))
        .unwrap();
        let directive = json!({ "text": crate::brevity::DIRECTIVE });

        let claude =
            with_router_points(&with_brevity(req.clone()), "us.anthropic.claude-haiku-5-5");
        let nova = with_router_points(&with_brevity(req), "us.amazon.nova-pro-v1:0");

        assert_eq!(
            claude["messages"][0]["content"],
            json!([{ "text": "Fix the failing test." }, cache_point(), directive])
        );
        assert_eq!(
            nova["messages"][0]["content"],
            json!([{ "text": "Fix the failing test.\n" }, cache_point(), directive])
        );
    }

    #[test]
    fn brevitys_directive_stays_in_system_when_the_last_turn_is_not_the_users() {
        // No tools, so brevity does not read the trailing tool result as mid tool-loop.
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "why did the deploy fail?" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": { "name": "logs", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "call_1", "content": "exit 1" }
            ]
        }))
        .unwrap();

        let (body, _) = to_converse_request(
            &with_brevity(req),
            &resolved("us.anthropic.claude-haiku-5-5"),
        )
        .unwrap();

        assert_eq!(
            body["system"],
            json!([{ "text": "You are helpful." }, { "text": crate::brevity::DIRECTIVE }])
        );
        assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_model_without_prompt_caching_gets_no_cache_points() {
        let (body, _) =
            to_converse_request(&marked_anthropic_request(), &resolved("deepseek.v3.2")).unwrap();

        let sent = body.to_string();
        assert!(!sent.contains("cachePoint"));
        assert!(!sent.contains("cache_control"));
        assert_eq!(
            body["system"],
            json!([{ "text": "You are a coding agent.Repository map." }])
        );
    }

    #[test]
    fn request_extracts_system_and_translates_tools() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "temperature": 0.7,
            "max_tokens": 2000,
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "translate hello" }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "translate",
                    "description": "Translate text",
                    "parameters": { "type": "object", "properties": { "text": { "type": "string" } } }
                }
            }],
            "tool_choice": "auto"
        }))
        .unwrap();
        let (body, _map) = to_converse_request(&req, &resolved("deepseek.v3.2")).unwrap();

        // System extracted to top level.
        assert_eq!(body["system"][0]["text"], "You are helpful.");
        // Messages: only the user turn (system removed).
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["content"][0]["text"], "translate hello");
        // Inference config.
        assert_eq!(body["inferenceConfig"]["maxTokens"], 2000);
        assert_eq!(body["inferenceConfig"]["temperature"], 0.7);
        // Tool config.
        assert_eq!(
            body["toolConfig"]["tools"][0]["toolSpec"]["name"],
            "translate"
        );
        assert_eq!(
            body["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["properties"]["text"]
                ["type"],
            "string"
        );
        assert_eq!(body["toolConfig"]["toolChoice"], json!({ "auto": {} }));
    }

    fn tool_request(tool_choice: &str, extra_messages: Value) -> ChatRequest {
        let mut messages = vec![json!({ "role": "user", "content": "count to 10" })];
        messages.extend(extra_messages.as_array().cloned().unwrap_or_default());
        serde_json::from_value(json!({
            "messages": messages,
            "tools": [{ "type": "function", "function": { "name": "f", "parameters": { "type": "object" } } }],
            "tool_choice": tool_choice,
            "stop": ["6"]
        }))
        .unwrap()
    }

    #[test]
    fn tool_choice_none_without_tool_history_sends_no_tools() {
        let req = tool_request("none", json!([]));
        let (body, _map) = to_converse_request(&req, &resolved("amazon.nova-pro-v1:0")).unwrap();
        assert!(body.get("toolConfig").is_none());
    }

    #[test]
    fn tool_choice_none_with_tool_history_keeps_the_tools() {
        let history = json!([
            { "role": "assistant", "content": null, "tool_calls": [{
                "id": "c1", "type": "function", "function": { "name": "f", "arguments": "{}" }
            }]},
            { "role": "tool", "tool_call_id": "c1", "content": "ok" }
        ]);
        let req = tool_request("none", history);
        let (body, _map) = to_converse_request(&req, &resolved("amazon.nova-pro-v1:0")).unwrap();
        assert_eq!(body["toolConfig"]["tools"][0]["toolSpec"]["name"], "f");
        assert!(body["toolConfig"].get("toolChoice").is_none());
    }

    #[test]
    fn stop_becomes_stop_sequences() {
        let req = tool_request("auto", json!([]));
        let (body, _map) = to_converse_request(&req, &resolved("amazon.nova-pro-v1:0")).unwrap();
        assert_eq!(body["inferenceConfig"]["stopSequences"], json!(["6"]));

        let single: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "stop": "END"
        }))
        .unwrap();
        let (body, _map) = to_converse_request(&single, &resolved("amazon.nova-pro-v1:0")).unwrap();
        assert_eq!(body["inferenceConfig"]["stopSequences"], json!(["END"]));
        assert!(body.get("stop").is_none());
    }

    #[test]
    fn image_part_becomes_a_format_token_and_inline_bytes() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "anthropic.claude-3-5-sonnet",
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "what is this?" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }
            ]}]
        }))
        .unwrap();

        let (body, _map) =
            to_converse_request(&req, &resolved("anthropic.claude-3-5-sonnet")).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["text"], "what is this?");
        // A bare token, never the mime type — Converse rejects "image/png" here.
        assert_eq!(content[1]["image"]["format"], "png");
        assert_eq!(content[1]["image"]["source"]["bytes"], "iVBORw0KGgo=");
    }

    #[test]
    fn jpeg_maps_to_the_jpeg_token_not_a_split_of_the_mime_type() {
        // The trap this guards: `media_type.split('/').last()` yields "jpeg" here but
        // "quicktime" for video/quicktime, so the mapping is a table, not a guess.
        for (mime, token) in [
            ("image/jpeg", "jpeg"),
            ("image/jpg", "jpeg"),
            ("image/gif", "gif"),
            ("image/webp", "webp"),
        ] {
            assert_eq!(converse_image_format(mime), Some(token));
        }
        assert_eq!(converse_image_format("image/svg+xml"), None);
    }

    #[test]
    fn video_maps_to_converse_format_tokens_not_a_split_of_the_mime_type() {
        // video/quicktime is the case that proves the table earns its keep: a
        // split('/') guess yields "quicktime", and Converse only knows "mov".
        assert_eq!(converse_video_format("video/quicktime"), Some("mov"));
        assert_eq!(converse_video_format("video/x-matroska"), Some("mkv"));
        assert_eq!(converse_video_format("video/3gpp"), Some("three_gp"));
        assert_eq!(converse_video_format("video/mp4"), Some("mp4"));
        assert_eq!(converse_video_format("video/ogg"), None);

        let req: ChatRequest = serde_json::from_value(json!({
            "model": "anthropic.claude-3-5-sonnet",
            "messages": [{ "role": "user", "content": [
                { "type": "video_url", "video_url": { "url": "data:video/quicktime;base64,AAAA" } }
            ]}]
        }))
        .unwrap();
        let (body, _map) =
            to_converse_request(&req, &resolved("anthropic.claude-3-5-sonnet")).unwrap();
        let video = &body["messages"][0]["content"][0]["video"];
        assert_eq!(video["format"], "mov");
        assert_eq!(video["source"]["bytes"], "AAAA");
    }

    #[test]
    fn document_names_are_stripped_of_dots_and_made_unique() {
        // Converse validates `name` against [a-zA-Z0-9\s\-\(\)\[\]]+ — no dots.
        assert_eq!(sanitize_document_name(Some("q3.pdf")), "q3 pdf");
        assert_eq!(sanitize_document_name(Some("a/b_c!.pdf")), "a b c pdf");
        assert_eq!(sanitize_document_name(None), "document");
        assert_eq!(sanitize_document_name(Some("...")), "document");

        let mut used = HashSet::new();
        assert_eq!(
            unique_document_name(Some("report.pdf"), &mut used),
            "report pdf"
        );
        assert_eq!(
            unique_document_name(Some("report.pdf"), &mut used),
            "report pdf 2"
        );
        assert_eq!(unique_document_name(None, &mut used), "document");
        assert_eq!(unique_document_name(None, &mut used), "document 2");
    }

    #[test]
    fn pdf_part_becomes_a_named_document_block() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "anthropic.claude-3-5-sonnet",
            "messages": [{ "role": "user", "content": [
                { "type": "file", "file": {
                    "file_data": "data:application/pdf;base64,JVBERi0=",
                    "filename": "q3.pdf"
                }}
            ]}]
        }))
        .unwrap();

        let (body, _map) =
            to_converse_request(&req, &resolved("anthropic.claude-3-5-sonnet")).unwrap();
        let doc = &body["messages"][0]["content"][0]["document"];
        assert_eq!(doc["format"], "pdf");
        assert_eq!(doc["name"], "q3 pdf");
        assert_eq!(doc["source"]["bytes"], "JVBERi0=");
    }

    #[test]
    fn tool_results_carry_media_instead_of_flattening_to_text() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "anthropic.claude-3-5-sonnet",
            "messages": [
                { "role": "user", "content": "chart it" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "tc_1", "type": "function",
                    "function": { "name": "chart", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "tc_1", "content": [
                    { "type": "text", "text": "here" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }
                ]}
            ]
        }))
        .unwrap();

        let (body, _map) =
            to_converse_request(&req, &resolved("anthropic.claude-3-5-sonnet")).unwrap();
        let result = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|m| m["content"][0].get("toolResult"))
            .expect("tool result turn");
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["text"], "here");
        assert_eq!(content[1]["image"]["format"], "png");
    }

    #[test]
    fn unfetchable_and_unsupported_media_are_refused_by_name() {
        let cases = [
            (
                json!({ "type": "image_url", "image_url": { "url": "https://example.com/c.png" } }),
                "web URL",
            ),
            (
                json!({ "type": "input_audio", "input_audio": { "data": "UklG", "format": "wav" } }),
                "input_audio",
            ),
            (
                json!({ "type": "image_url", "image_url": { "url": "data:image/svg+xml;base64,PHN2" } }),
                "image/svg+xml",
            ),
        ];
        for (part, needle) in cases {
            let req: ChatRequest = serde_json::from_value(json!({
                "model": "anthropic.claude-3-5-sonnet",
                "messages": [{ "role": "user", "content": [part] }]
            }))
            .unwrap();
            let err =
                to_converse_request(&req, &resolved("anthropic.claude-3-5-sonnet")).unwrap_err();
            match err {
                ProviderError::Rejected(message) => {
                    assert!(message.contains(needle), "{message} should name {needle}");
                }
                other => panic!("expected a 400, got {other:?}"),
            }
        }
    }

    #[test]
    fn tool_history_translates_to_converse_shape() {
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "user", "content": "go" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "tc_1", "type": "function",
                    "function": { "name": "search", "arguments": "{\"q\":\"rust\"}" }
                }]},
                { "role": "tool", "tool_call_id": "tc_1", "content": "found it" }
            ]
        }))
        .unwrap();
        let (body, _map) = to_converse_request(&req, &resolved("deepseek.v3.2")).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        // Assistant turn with toolUse.
        assert_eq!(msgs[1]["content"][0]["toolUse"]["name"], "search");
        assert_eq!(msgs[1]["content"][0]["toolUse"]["input"]["q"], "rust");
        // Tool result as user turn with toolResult.
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["toolResult"]["toolUseId"], "tc_1");
        assert_eq!(
            msgs[2]["content"][0]["toolResult"]["content"][0]["text"],
            "found it"
        );
    }

    #[test]
    fn response_text_translates_to_openai_shape() {
        let converse = json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{ "text": "Hello!" }]
                }
            },
            "stopReason": "end_turn",
            "usage": { "inputTokens": 8, "outputTokens": 6, "totalTokens": 14 }
        });
        let resp = from_converse_response(&converse, "deepseek.v3.2", &HashMap::new()).unwrap();
        assert_eq!(resp.model, "deepseek.v3.2");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("Hello!"));
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(8));
        assert_eq!(usage.completion_tokens, Some(6));
        assert_eq!(usage.total_tokens, Some(14));
    }

    #[test]
    fn response_tool_use_translates_to_openai_tool_calls() {
        let converse = json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{
                        "toolUse": {
                            "toolUseId": "tu_1",
                            "name": "search",
                            "input": { "q": "rust" }
                        }
                    }]
                }
            },
            "stopReason": "tool_use",
            "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 }
        });
        let resp = from_converse_response(&converse, "deepseek.v3.2", &HashMap::new()).unwrap();
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let msg = &resp.choices[0].message;
        assert_eq!(msg.content, Some(Value::Null));
        let tc = &msg.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.id, "tu_1");
        assert_eq!(tc.function.name, "search");
        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["q"], "rust");
    }

    #[test]
    fn stop_reason_mapping() {
        assert_eq!(map_stop_reason("end_turn"), "stop");
        assert_eq!(map_stop_reason("stop_sequence"), "stop");
        assert_eq!(map_stop_reason("tool_use"), "tool_calls");
        assert_eq!(map_stop_reason("max_tokens"), "length");
        assert_eq!(map_stop_reason("content_filtered"), "content_filter");
        assert_eq!(map_stop_reason("unknown"), "stop");
    }

    #[test]
    fn model_id_preserves_existing_prefix() {
        let base = "https://bedrock-runtime.us-west-2.amazonaws.com";
        assert_eq!(
            converse_model_id("us.openai.gpt-6-astra", base),
            "us.openai.gpt-6-astra"
        );
        assert_eq!(converse_model_id("deepseek.v3.2", base), "deepseek.v3.2");
    }

    #[test]
    fn prefixed_model_id_adds_region_prefix() {
        let base = "https://bedrock-runtime.us-west-2.amazonaws.com";
        assert_eq!(
            prefixed_model_id("openai.gpt-6-astra", base),
            "us.openai.gpt-6-astra"
        );
        assert_eq!(
            prefixed_model_id("us.openai.gpt-6-astra", base),
            "us.openai.gpt-6-astra"
        );
        let eu_base = "https://bedrock-runtime.eu-west-1.amazonaws.com";
        assert_eq!(
            prefixed_model_id("openai.gpt-6-astra", eu_base),
            "eu.openai.gpt-6-astra"
        );
    }

    #[test]
    fn converse_usage_retains_cache_subdivisions_and_provider_total() {
        let usage = converse_usage(&json!({
            "inputTokens":2,"outputTokens":5,"totalTokens":7,
            "cacheReadInputTokens":1000,"cacheWriteInputTokens":300,
            "cacheDetails":[{"ttl":"5m","inputTokens":100},{"ttl":"1h","inputTokens":200}]
        }));
        assert_eq!(usage.prompt_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(7));
        assert_eq!(usage.cache_read_input_tokens, Some(1000));
        assert_eq!(usage.cache_creation_input_tokens, Some(300));
        let ttl = usage.cache_creation.unwrap();
        assert_eq!(ttl.ephemeral_5m_input_tokens, Some(100));
        assert_eq!(ttl.ephemeral_1h_input_tokens, Some(200));
        let missing = converse_usage(&json!({"inputTokens":0}));
        assert_eq!(missing.prompt_tokens, Some(0));
        assert!(missing.cache_read_input_tokens.is_none());
        assert!(missing.total_tokens.is_none());
    }

    #[test]
    fn event_stream_decoder_extracts_json_payload() {
        // Build a minimal event-stream frame with :event-type = "contentBlockDelta"
        // and a JSON payload.
        let payload = r#"{"contentBlockIndex":0,"delta":{"text":"Hi"}}"#;
        let frame = build_test_frame("contentBlockDelta", payload.as_bytes());

        let mut buf = BytesMut::from(&frame[..]);
        let frame = decode_event_stream_frame(&mut buf).unwrap();
        assert_eq!(frame.event_type, "contentBlockDelta");
        assert_eq!(frame.payload, payload);
        assert!(!frame.is_exception());
        assert!(buf.is_empty()); // fully consumed
    }

    #[test]
    fn event_stream_decoder_handles_incomplete_frame() {
        let payload = r#"{"text":"hi"}"#;
        let frame = build_test_frame("test", payload.as_bytes());
        // Feed only half the frame.
        let mut buf = BytesMut::from(&frame[..frame.len() / 2]);
        assert!(decode_event_stream_frame(&mut buf).is_none());
    }

    /// Build a test event-stream frame with one `:event-type` header.
    fn build_test_frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
        // Header: 1 byte name_len + name + 1 byte type (7=string) + 2 byte val_len + val
        let header_name = b":event-type";
        let header_val = event_type.as_bytes();
        let header_len = 1 + header_name.len() + 1 + 2 + header_val.len();
        let total_len = 12 + header_len + payload.len() + 4; // prelude + headers + payload + msg_crc

        let mut frame = Vec::with_capacity(total_len);
        frame.extend_from_slice(&(total_len as u32).to_be_bytes());
        frame.extend_from_slice(&(header_len as u32).to_be_bytes());
        frame.extend_from_slice(&[0u8; 4]); // prelude CRC (skipped)

        // Header
        frame.push(header_name.len() as u8);
        frame.extend_from_slice(header_name);
        frame.push(7); // string type
        frame.extend_from_slice(&(header_val.len() as u16).to_be_bytes());
        frame.extend_from_slice(header_val);

        // Payload
        frame.extend_from_slice(payload);

        // Message CRC (skipped)
        frame.extend_from_slice(&[0u8; 4]);

        assert_eq!(frame.len(), total_len);
        frame
    }

    #[test]
    fn sanitize_tool_name_replaces_hyphens_and_dots() {
        assert_eq!(sanitize_tool_name("query-docs"), "query_docs");
        assert_eq!(
            sanitize_tool_name("resolve-library-id"),
            "resolve_library_id"
        );
        assert_eq!(
            sanitize_tool_name("mcp__plugin_context7_context7__query-docs"),
            "mcp__plugin_context7_context7__query_docs"
        );
        // Already valid names pass through unchanged.
        assert_eq!(sanitize_tool_name("search"), "search");
        assert_eq!(sanitize_tool_name("my_tool_2"), "my_tool_2");
        // Starts with digit → prepend 't'.
        assert_eq!(sanitize_tool_name("123tool"), "t123tool");
    }

    #[test]
    fn tool_name_round_trips_through_sanitize_and_unsanitize() {
        let tools = vec![ToolDef {
            kind: "function".to_string(),
            function: crate::ir::FunctionDef {
                name: "query-docs".to_string(),
                description: None,
                parameters: None,
                strict: None,
            },
            extra: Map::new(),
        }];
        let map = build_tool_name_map(&tools);
        assert_eq!(map.get("query_docs").unwrap(), "query-docs");
        assert_eq!(unsanitize_tool_name("query_docs", &map), "query-docs");
        // Names that don't need sanitizing have no map entry.
        assert_eq!(unsanitize_tool_name("search", &map), "search");
    }

    /// The body the provider sends with `TOKEN_CACHE_MARKERS` on.
    fn with_router_points(req: &ChatRequest, model: &str) -> Value {
        BedrockConverseProvider::new(reqwest::Client::new(), "http://x".into())
            .with_cache_markers(true)
            .request_body(req, &resolved(model))
            .unwrap()
            .0
    }

    fn plain_request() -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": "Fix the failing test." }
            ]
        }))
        .unwrap()
    }

    /// An agent that set no markers, mid tool loop: its last turn is a tool result.
    fn tool_loop_request() -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": "Fix the failing test." },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "toolu_1", "type": "function",
                    "function": { "name": "read", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "toolu_1", "content": "fn main() {}" }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn the_router_ends_the_conversation_then_system_with_a_cache_point() {
        for model in ["us.anthropic.claude-haiku-5-5", "us.amazon.nova-pro-v1:0"] {
            let body = with_router_points(&plain_request(), model);

            assert_eq!(
                body["system"],
                json!([{ "text": "You are a coding agent." }, cache_point()]),
                "{model}"
            );
            assert_eq!(
                body["messages"][0]["content"],
                json!([{ "text": "Fix the failing test." }, cache_point()]),
                "{model}"
            );
        }
    }

    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";
    const CLAUDE: &str = "us.anthropic.claude-haiku-5-5";

    fn converse_body(request: Value, model: &str) -> Value {
        let ir: ChatRequest = serde_json::from_value(request).unwrap();
        BedrockConverseProvider::new(reqwest::Client::new(), "http://x".into())
            .request_body(&ir, &resolved(model))
            .unwrap()
            .0
    }

    fn png_block() -> Value {
        json!({ "image": { "format": "png", "source": { "bytes": "iVBORw0KGgo=" } } })
    }

    #[test]
    fn an_empty_text_part_beside_an_image_is_left_out_with_its_cache_point() {
        let ephemeral = json!({ "type": "ephemeral" });
        for (model, expected) in [
            (
                CLAUDE,
                json!([{ "text": "Look." }, cache_point(), png_block()]),
            ),
            (
                "us.amazon.nova-pro-v1:0",
                json!([{ "text": "Look." }, cache_point(), png_block()]),
            ),
        ] {
            let body = converse_body(
                json!({ "messages": [{ "role": "user", "content": [
                    { "type": "text", "text": "Look.", "cache_control": ephemeral },
                    { "type": "text", "text": "", "cache_control": ephemeral },
                    { "type": "image_url", "image_url": { "url": PNG } }
                ]}]}),
                model,
            );

            assert_eq!(body["messages"][0]["content"], expected, "{model}");
        }
    }

    #[test]
    fn an_empty_text_part_inside_a_tool_result_with_an_image_is_left_out() {
        let body = converse_body(
            json!({ "messages": [
                { "role": "user", "content": "Read the chart." },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "toolu_1", "type": "function",
                    "function": { "name": "read_file", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "toolu_1", "content": [
                    { "type": "text", "text": "" },
                    { "type": "image_url", "image_url": { "url": PNG } }
                ]}
            ]}),
            CLAUDE,
        );

        assert_eq!(
            body["messages"][2]["content"][0]["toolResult"]["content"],
            json!([png_block()])
        );
    }

    #[test]
    fn a_mixed_case_data_url_media_type_maps_to_its_bedrock_format() {
        let body = converse_body(
            json!({ "messages": [{ "role": "user", "content": [
                { "type": "image_url", "image_url": { "url": "data:Image/PNG;base64,iVBORw0KGgo=" } },
                { "type": "file", "file": { "file_data": "data:application/pdf;name=q3.pdf;base64,JVBERi0=" } }
            ]}]}),
            CLAUDE,
        );

        let content = &body["messages"][0]["content"];
        assert_eq!(content[0], png_block());
        assert_eq!(content[1]["document"]["format"], "pdf");
        assert_eq!(content[1]["document"]["name"], "q3 pdf");
    }

    #[test]
    fn content_with_nothing_but_empty_text_keeps_its_one_empty_block() {
        for content in [json!(""), json!([{ "type": "text", "text": "" }])] {
            let body = converse_body(
                json!({ "messages": [{ "role": "user", "content": content }] }),
                CLAUDE,
            );

            assert_eq!(
                body["messages"][0]["content"],
                json!([{ "text": "" }]),
                "{content}"
            );
        }
    }

    #[test]
    fn a_request_without_a_system_prompt_is_sent_without_a_system_field() {
        let request: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "Fix the failing test." }]
        }))
        .unwrap();

        for model in ["us.anthropic.claude-haiku-5-5", "us.amazon.nova-pro-v1:0"] {
            let body = with_router_points(&request, model);

            assert!(body.get("system").is_none(), "{model}");
            assert_eq!(
                body["messages"][0]["content"],
                json!([{ "text": "Fix the failing test." }, cache_point()]),
                "{model}"
            );
        }
    }

    #[test]
    fn a_turn_of_tool_results_gets_a_cache_point_on_claude_and_nova_takes_the_last_text() {
        let claude = with_router_points(&tool_loop_request(), "us.anthropic.claude-haiku-5-5");
        let nova = with_router_points(&tool_loop_request(), "us.amazon.nova-pro-v1:0");

        assert_eq!(claude["messages"][2]["content"][1], cache_point());
        // Nova rejects a cache point after a tool call or result, so the last text takes it.
        assert_eq!(nova["messages"][2]["content"].as_array().unwrap().len(), 1);
        assert_eq!(nova["messages"][1]["content"].as_array().unwrap().len(), 1);
        assert_eq!(
            nova["messages"][0]["content"],
            json!([{ "text": "Fix the failing test." }, cache_point()])
        );
        assert_eq!(nova["system"][1], cache_point());
    }

    /// Step `n` of a tool loop in which the agent says a line before each tool call.
    fn narrated_tool_loop(steps: usize) -> ChatRequest {
        let mut messages = vec![
            json!({ "role": "system", "content": "You are a coding agent." }),
            json!({ "role": "user", "content": "Fix the failing test." }),
        ];
        for i in 0..steps {
            messages.push(json!({ "role": "assistant", "content": format!("Reading file {i}."), "tool_calls": [{
                "id": format!("toolu_{i}"), "type": "function",
                "function": { "name": "read", "arguments": "{}" }
            }]}));
            messages.push(json!({ "role": "tool", "tool_call_id": format!("toolu_{i}"), "content": format!("fn f{i}() {{}}") }));
        }
        serde_json::from_value(json!({ "messages": messages })).unwrap()
    }

    #[test]
    fn in_a_nova_tool_loop_each_request_repeats_the_point_the_last_one_ended_with() {
        let nova = "us.amazon.nova-pro-v1:0";
        let before = with_router_points(&narrated_tool_loop(1), nova);
        let after = with_router_points(&narrated_tool_loop(2), nova);

        // The assistant's text ahead of its tool call ends each request.
        let narrated = |i: usize| {
            json!([
                { "text": format!("Reading file {i}.") },
                cache_point(),
                { "toolUse": { "toolUseId": format!("toolu_{i}"), "name": "read", "input": {} } }
            ])
        };
        assert_eq!(before["messages"][1]["content"], narrated(0));
        assert_eq!(after["messages"][3]["content"], narrated(1));
        // The next request repeats that point, so it reads everything up to it.
        assert_eq!(after["messages"][1]["content"], narrated(0));
        assert_eq!(before["messages"][2], after["messages"][2]);
        assert_eq!(count_cache_points(&after), 3);
    }

    #[test]
    fn the_routers_last_cache_point_goes_ahead_of_brevitys_directive() {
        for model in ["us.anthropic.claude-haiku-5-5", "us.amazon.nova-pro-v1:0"] {
            let body = with_router_points(&with_brevity(plain_request()), model);

            assert_eq!(
                body["messages"][0]["content"],
                json!([
                    { "text": "Fix the failing test." },
                    cache_point(),
                    { "text": crate::brevity::DIRECTIVE }
                ]),
                "{model}"
            );
        }
    }

    #[test]
    fn nova_gets_the_previous_turns_cache_point_again_and_claude_does_not() {
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": "Read the notes." },
                { "role": "assistant", "content": "Read." },
                { "role": "user", "content": "Say ok." }
            ]
        }))
        .unwrap();

        let nova = with_router_points(&req, "us.amazon.nova-pro-v1:0");
        let claude = with_router_points(&req, "us.anthropic.claude-haiku-5-5");

        let previous_turn_marked = json!([{ "text": "Read the notes." }, cache_point()]);
        assert_eq!(nova["messages"][0]["content"], previous_turn_marked);
        assert_eq!(nova["messages"][2]["content"][1], cache_point());
        assert_eq!(
            claude["messages"][0]["content"],
            json!([{ "text": "Read the notes." }])
        );
    }

    #[test]
    fn an_agent_holding_all_four_cache_points_gets_none_from_the_router() {
        use crate::inbound::{AnthropicInbound, InboundParser};
        let ephemeral = json!({ "type": "ephemeral" });
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "system": "You are a coding agent.",
                "tools": [{ "name": "read", "input_schema": { "type": "object" }, "cache_control": ephemeral }],
                "messages": [
                    { "role": "user", "content": [
                        { "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }
                    ]},
                    { "role": "assistant", "content": [
                        { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {}, "cache_control": ephemeral }
                    ]},
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}", "cache_control": ephemeral },
                        { "type": "text", "text": "Then stop." }
                    ]}
                ]
            }))
            .unwrap();
        let model = "us.anthropic.claude-haiku-5-5";

        // Neither the end of the conversation nor `system` has one, but a fifth is rejected.
        let body = with_router_points(&req, model);

        assert_eq!(count_cache_points(&body), MAX_CACHE_POINTS);
        assert_eq!(body, to_converse_request(&req, &resolved(model)).unwrap().0);
    }

    #[test]
    fn no_five_minute_system_point_goes_ahead_of_an_agents_one_hour_point() {
        use crate::inbound::{AnthropicInbound, InboundParser};
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "system": "You are a coding agent.",
                "messages": [
                    { "role": "user", "content": [
                        { "type": "text", "text": "Read the repository map.", "cache_control": { "type": "ephemeral", "ttl": "1h" } }
                    ]},
                    { "role": "assistant", "content": "Done." },
                    { "role": "user", "content": "Now fix the failing test." }
                ]
            }))
            .unwrap();

        let body = with_router_points(&req, "us.anthropic.claude-haiku-5-5");

        assert_eq!(
            body["system"],
            json!([{ "text": "You are a coding agent." }])
        );
        assert_eq!(body["messages"][2]["content"][1], cache_point());
    }

    #[test]
    fn the_router_adds_no_cache_point_without_the_flag_or_for_a_model_without_caching() {
        let off = BedrockConverseProvider::new(reqwest::Client::new(), "http://x".into());
        let claude = "us.anthropic.claude-haiku-5-5";
        assert_eq!(
            off.request_body(&plain_request(), &resolved(claude))
                .unwrap()
                .0,
            to_converse_request(&plain_request(), &resolved(claude))
                .unwrap()
                .0
        );

        let gpt = "us.openai.gpt-6-astra";
        assert_eq!(
            with_router_points(&plain_request(), gpt),
            to_converse_request(&plain_request(), &resolved(gpt))
                .unwrap()
                .0
        );
    }

    #[test]
    fn claude_without_prompt_caching_gets_no_router_cache_point_but_keeps_the_agents() {
        for model in [
            "anthropic.claude-3-haiku-20240307-v1:0",
            "us.anthropic.claude-3-5-sonnet-20241022-v2:0",
        ] {
            let sent = with_router_points(&plain_request(), model).to_string();
            assert!(!sent.contains("cachePoint"), "{model}");

            let marked = with_router_points(&marked_anthropic_request(), model);
            assert_eq!(
                marked,
                to_converse_request(&marked_anthropic_request(), &resolved(model))
                    .unwrap()
                    .0,
                "{model}"
            );
        }
        // Claude 3.5 Haiku and 3.7 Sonnet cache, so they still get the router's points.
        let sent = with_router_points(
            &plain_request(),
            "us.anthropic.claude-3-7-sonnet-20250219-v1:0",
        );
        assert!(sent.to_string().contains("cachePoint"));
    }

    #[test]
    fn glm_gets_no_cache_point_from_the_agent_or_the_router() {
        // Converse fails the whole GLM call with 403 when a request carries one.
        for model in ["zai.glm-5", "zai.glm-4.7", "us.zai.glm-5.3"] {
            for req in [marked_anthropic_request(), plain_request()] {
                let sent = with_router_points(&req, model).to_string();

                assert!(!sent.contains("cachePoint"), "{model}");
                assert!(!sent.contains("cache_control"), "{model}");
            }
        }
    }

    #[tokio::test]
    async fn the_default_gateway_config_sends_the_routers_cache_points() {
        let mut server = mockito::Server::new_async().await;
        let model = "us.anthropic.claude-haiku-5-5";
        let m = server
            .mock("POST", format!("/model/{model}/converse").as_str())
            .match_body(mockito::Matcher::PartialJson(json!({
                "system": [{ "text": "You are a coding agent." }, cache_point()],
                "messages": [{ "role": "user", "content": [
                    { "text": "Fix the failing test." }, cache_point()
                ]}]
            })))
            .with_status(200)
            .with_body(
                json!({
                    "output": { "message": { "role": "assistant", "content": [{ "text": "On it." }] } },
                    "stopReason": "end_turn",
                    "usage": { "inputTokens": 10, "outputTokens": 3, "totalTokens": 13 }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let endpoint = crate::resolver::CustomEndpoint {
            base_url: server.url(),
            dialect: crate::providers::dialect::ProviderDialect::BedrockConverse,
            public_only: false,
        };
        let provider = crate::providers::provider_for(
            "aws-bedrock",
            Some(&endpoint),
            &reqwest::Client::new(),
            &crate::config::GatewayConfig::default(),
        )
        .unwrap();

        provider
            .chat(&plain_request(), &resolved(model))
            .await
            .unwrap();

        m.assert_async().await;
    }
}
