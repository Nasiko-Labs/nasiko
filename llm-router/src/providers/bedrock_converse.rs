//! AWS Bedrock Converse provider — OpenAI ⇄ Bedrock Converse API translation.
//!
//! The Bedrock Runtime Converse API (`/model/{id}/converse`) supports every Bedrock
//! model, including INFERENCE_PROFILE-only ones (Claude, GPT-6, Grok, …) that the
//! OpenAI-compatible Mantle surface cannot serve. The trade-off: the wire format is
//! Bedrock's own, so this spoke translates in both directions.
//!
//! Request: system messages → top-level `system`; user/assistant content → `[{text}]`
//! blocks; tool calls → `toolUse` blocks; tool results → `toolResult` in a following
//! user turn; tools → `toolConfig.tools[].toolSpec`; params → `inferenceConfig`.
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

use super::{ProviderClient, ProviderError, delta_chunk, finish_chunk, now_unix, usage_chunk};
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, Choice, Delta, EmbeddingsRequest, EmbeddingsResponse,
    FunctionCall, FunctionCallDelta, Message, ToolCall, ToolCallDelta, ToolDef, Usage,
};
use crate::resolver::ResolvedConfig;

use super::dialect::{bedrock_region, bedrock_region_prefix};

/// Bedrock requires `maxTokens` in `inferenceConfig`; used when neither config nor
/// request sets it.
const DEFAULT_MAX_TOKENS: i64 = 4096;

pub struct BedrockConverseProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://bedrock-runtime.us-west-2.amazonaws.com`.
    base: String,
}

impl BedrockConverseProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self { http, base }
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
        let (body, name_map) = to_converse_request(req, cfg);
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
            if is_temperature_unsupported(status.as_u16(), &text) {
                return self.chat_no_temperature(req, cfg, &model_id).await;
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
        let (body, name_map) = to_converse_request(req, cfg);
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
            if is_temperature_unsupported(status.as_u16(), &text) {
                return self.stream_no_temperature(req, cfg, &model_id).await;
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
        _cfg: &ResolvedConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        Err(ProviderError::Status {
            status: 501,
            message: "Bedrock Converse has no embeddings endpoint".to_string(),
            retryable: false,
        })
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
        let (body, name_map) = to_converse_request(req, cfg);
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
            if is_temperature_unsupported(status.as_u16(), &text) {
                return self.chat_no_temperature(req, cfg, model_id).await;
            }
            return Err(Self::status_error(status, text));
        }

        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        from_converse_response(&value, &cfg.model, &name_map)
    }

    /// Retry chat without the temperature field (some models reject it).
    async fn chat_no_temperature(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
        model_id: &str,
    ) -> Result<ChatResponse, ProviderError> {
        let (body, name_map) = to_converse_request_no_temperature(req, cfg);
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

    /// Retry streaming without the temperature field (some models reject it).
    async fn stream_no_temperature(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
        model_id: &str,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let (body, _name_map) = to_converse_request_no_temperature(req, cfg);
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
            "bedrock converse-stream (no-temp retry): outgoing request stats"
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
            tool_count,
            "bedrock converse-stream (no-temp retry): response status"
        );
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            tracing::error!(
                target: "nasiko::llm_router::bedrock",
                %status,
                model = %model_id,
                body_preview = &text[..text.len().min(500)],
                "bedrock converse-stream (no-temp retry): error — this is the FINAL retry"
            );
            return Err(Self::status_error(status, text));
        }

        // `_name_map` is empty here because `to_converse_request_no_temperature`
        // discards it. Rebuild so tool names are properly unsanitized in the response.
        let name_map = req
            .tools
            .as_ref()
            .map(|t| build_tool_name_map(t))
            .unwrap_or_default();
        Ok(Box::pin(converse_event_stream(
            resp.bytes_stream(),
            cfg.model.clone(),
            name_map,
            "no-temp-retry",
        )))
    }

    /// Stream with an explicit model ID (used for prefix retry).
    async fn stream_with_model(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
        model_id: &str,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let (body, name_map) = to_converse_request(req, cfg);
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
            if is_temperature_unsupported(status.as_u16(), &text) {
                return self.stream_no_temperature(req, cfg, model_id).await;
            }
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

/// Whether Bedrock rejected the request because the model doesn't support temperature.
/// Shared event-stream decoder for all Bedrock Converse streaming paths (primary,
/// prefix-retry, no-temperature-retry). Handles text content, tool-use blocks,
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
                let preview = String::from_utf8_lossy(&chunk[..chunk.len().min(256)]);
                tracing::info!(
                    target: "nasiko::llm_router::bedrock",
                    chunk_len = chunk.len(),
                    label,
                    preview = %preview,
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

fn is_temperature_unsupported(status: u16, body: &str) -> bool {
    status == 400 && body.contains("doesn't support the temperature field")
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

/// Returns `(body, tool_name_map)` where `tool_name_map` maps sanitized names
/// back to the originals (only entries that changed).
fn to_converse_request(
    req: &ChatRequest,
    cfg: &ResolvedConfig,
) -> (Value, HashMap<String, String>) {
    to_converse_request_inner(req, cfg, true)
}

fn to_converse_request_no_temperature(
    req: &ChatRequest,
    cfg: &ResolvedConfig,
) -> (Value, HashMap<String, String>) {
    to_converse_request_inner(req, cfg, false)
}

fn to_converse_request_inner(
    req: &ChatRequest,
    cfg: &ResolvedConfig,
    allow_temperature: bool,
) -> (Value, HashMap<String, String>) {
    let mut system_parts: Vec<Value> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    for m in &req.messages {
        match m.role.as_str() {
            "system" => {
                if let Some(t) = m.text() {
                    system_parts.push(json!({ "text": t }));
                }
            }
            "tool" => {
                pending_tool_results.push(json!({
                    "toolResult": {
                        "toolUseId": m.tool_call_id.clone().unwrap_or_default(),
                        "content": [{ "text": m.text().unwrap_or_default() }]
                    }
                }));
            }
            "assistant" => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                messages.push(assistant_to_converse(m));
            }
            _ => {
                // "user" and any unexpected role → user turn.
                flush_tool_results(&mut pending_tool_results, &mut messages);
                messages.push(json!({
                    "role": "user",
                    "content": [{ "text": m.text().unwrap_or_default() }]
                }));
            }
        }
    }
    flush_tool_results(&mut pending_tool_results, &mut messages);

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

    if allow_temperature && let Some(t) = cfg.temperature.or(req.temperature) {
        body["inferenceConfig"]["temperature"] = json!(t);
    }
    if !system_parts.is_empty() {
        body["system"] = json!(system_parts);
    }
    let mut name_map = HashMap::new();
    if let Some(tools) = &req.tools {
        name_map = build_tool_name_map(tools);
        let translated: Vec<Value> = tools.iter().map(tool_to_converse).collect();
        if !translated.is_empty() {
            body["toolConfig"] = json!({ "tools": translated });
            if let Some(choice) = req.tool_choice.as_ref().and_then(tool_choice_to_converse) {
                body["toolConfig"]["toolChoice"] = choice;
            }
        }
    }
    (body, name_map)
}

fn flush_tool_results(pending: &mut Vec<Value>, messages: &mut Vec<Value>) {
    if !pending.is_empty() {
        messages.push(json!({ "role": "user", "content": std::mem::take(pending) }));
    }
}

fn assistant_to_converse(m: &Message) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(t) = m.text()
        && !t.is_empty()
    {
        blocks.push(json!({ "text": t }));
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

    fn resolved(model: &str) -> ResolvedConfig {
        ResolvedConfig {
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
            compress_enabled: false,
            tool_compact_enabled: false,
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
        let (body, names) = to_converse_request(&request, &config);
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
        let (body, _map) = to_converse_request(&req, &resolved("deepseek.v3.2"));

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
        let (body, _map) = to_converse_request(&req, &resolved("deepseek.v3.2"));
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
            },
            extra: Map::new(),
        }];
        let map = build_tool_name_map(&tools);
        assert_eq!(map.get("query_docs").unwrap(), "query-docs");
        assert_eq!(unsanitize_tool_name("query_docs", &map), "query-docs");
        // Names that don't need sanitizing have no map entry.
        assert_eq!(unsanitize_tool_name("search", &map), "search");
    }
}
