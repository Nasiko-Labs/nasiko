//! Anthropic provider — OpenAI ⇄ Anthropic Messages API translation.
//!
//! Request (C1/C3): pull `system` out to the top level; `tools[].function.parameters`
//! → `tools[].input_schema` (no `type:function` wrapper); assistant `tool_calls[]` →
//! `tool_use` blocks; `{role:"tool"}` results → a following user turn with
//! `tool_result` blocks (consecutive results merged into one turn). `max_tokens` is
//! required by Anthropic, so we always send one. An agent's `cache_control` markers go back
//! on the system block, message block, tool result or tool they came from. Brevity's directive
//! ends the last user turn rather than joining `system` ([`crate::brevity`]). With
//! `TOKEN_CACHE_MARKERS` on, the router fills slots the agent left free ([`add_cache_markers`]).
//!
//! Response (C2): `content[].tool_use` → OpenAI `tool_calls[]` (arguments as a JSON
//! string); text blocks → `message.content`; `stop_reason` → `finish_reason`;
//! `usage.{input,output}_tokens` → `{prompt,completion,total}_tokens`.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Map, Value, json};

use super::media::{self, DocumentKind};
use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError, delta_chunk, finish_chunk, now_unix, usage_chunk};
use crate::ir::{
    CACHE_CONTROL, ChatChunk, ChatRequest, ChatResponse, Choice, ContentPart, ContentView,
    DataSource, Delta, EmbeddingsRequest, EmbeddingsResponse, FunctionCall, FunctionCallDelta,
    Message, ToolCall, ToolCallDelta, ToolDef, UnsupportedPart, Usage,
};
use crate::resolver::{ResolvedConfig, ResolvedEmbeddingConfig};

/// Anthropic requires `max_tokens`; used when neither config nor request sets it.
const DEFAULT_MAX_TOKENS: i64 = 4096;
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic accepts at most this many `cache_control` breakpoints in one request.
const MAX_CACHE_MARKERS: usize = 4;

pub struct AnthropicProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://api.anthropic.com/v1` (overridable for tests).
    base: String,
    /// Add the router's own cache markers ([`add_cache_markers`]).
    cache_markers: bool,
}

impl AnthropicProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self {
            http,
            base,
            cache_markers: false,
        }
    }

    /// Fill the cache-marker slots an agent leaves free (`TOKEN_CACHE_MARKERS`).
    pub fn with_cache_markers(mut self, enabled: bool) -> Self {
        self.cache_markers = enabled;
        self
    }

    fn request_body(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<Value, ProviderError> {
        let mut body = to_anthropic_request(req, cfg)?;
        if self.cache_markers {
            add_cache_markers(&mut body);
        }
        Ok(body)
    }
}

pub(crate) async fn count_tokens(
    http: &reqwest::Client,
    base: &str,
    api_key: &str,
    body: &Value,
) -> Result<Value, ProviderError> {
    let resp = http
        .post(format!("{base}/messages/count_tokens"))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(body)
        .send()
        .await
        .map_err(|e| ProviderError::Transport(e.to_string()))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ProviderError::Transport(e.to_string()))?;
    if !status.is_success() {
        return Err(ProviderError::Status {
            status: status.as_u16(),
            message: text,
            retryable: false,
        });
    }
    serde_json::from_str(&text).map_err(|e| ProviderError::Parse(e.to_string()))
}

#[async_trait]
impl ProviderClient for AnthropicProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let body = self.request_body(req, cfg)?;

        let resp = self
            .http
            .post(format!("{}/messages", self.base))
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(status_error(status, text, req, cfg));
        }

        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        from_anthropic_response(&value, &cfg.model)
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let mut body = self.request_body(req, cfg)?;
        body["stream"] = json!(true);

        let resp = self
            .http
            .post(format!("{}/messages", self.base))
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(status_error(status, text, req, cfg));
        }

        let model = cfg.model.clone();
        let data = sse_data_stream(resp.bytes_stream());
        let stream = async_stream::stream! {
            futures::pin_mut!(data);
            let mut id = String::new();
            // Anthropic content-block index → OpenAI tool_call index (text blocks
            // don't get a tool index, so the two can differ).
            let mut block_to_tool: HashMap<i64, i64> = HashMap::new();
            let mut tool_blocks_with_arguments: HashSet<i64> = HashSet::new();
            let mut next_tool_index: i64 = 0;
            let mut input_tokens: Option<i64> = None;
            let mut output_tokens: Option<i64> = None;
            let mut cache_read: Option<i64> = None;
            let mut cache_creation: Option<i64> = None;
            let mut cache_details = None;
            let mut finish: Option<String> = None;
            let mut stopped = false;

            while let Some(item) = data.next().await {
                let payload = match item {
                    Ok(p) => p,
                    Err(e) => { yield Err(e); return; }
                };
                let event: Value = match serde_json::from_str(&payload) {
                    Ok(v) => v,
                    Err(error) => {
                        yield Err(ProviderError::Parse(format!("invalid Anthropic stream event: {error}")));
                        return;
                    }
                };
                match event["type"].as_str() {
                    Some("error") => {
                        let message = event["error"]["message"]
                            .as_str()
                            .unwrap_or("Anthropic stream error");
                        yield Err(ProviderError::Status {
                            status: 502,
                            message: message.to_string(),
                            retryable: false,
                        });
                        return;
                    }
                    Some("message_start") => {
                        id = event["message"]["id"].as_str().unwrap_or_default().to_string();
                        let u = &event["message"]["usage"];
                        input_tokens = u["input_tokens"].as_i64();
                        cache_read = u["cache_read_input_tokens"].as_i64();
                        cache_creation = u["cache_creation_input_tokens"].as_i64();
                        cache_details = serde_json::from_value(u["cache_creation"].clone()).ok();
                        yield Ok(delta_chunk(&id, &model, Delta {
                            role: Some("assistant".to_string()),
                            ..Delta::default()
                        }));
                    }
                    Some("content_block_start") => {
                        let block = &event["content_block"];
                        if block["type"].as_str() == Some("tool_use") {
                            let oa_index = next_tool_index;
                            next_tool_index += 1;
                            block_to_tool.insert(event["index"].as_i64().unwrap_or(0), oa_index);
                            yield Ok(delta_chunk(&id, &model, Delta {
                                tool_calls: Some(vec![ToolCallDelta {
                                    index: oa_index,
                                    id: block["id"].as_str().map(str::to_string),
                                    kind: Some("function".to_string()),
                                    function: Some(FunctionCallDelta {
                                        name: block["name"].as_str().map(str::to_string),
                                        arguments: Some(String::new()),
                                    }),
                                    ..ToolCallDelta::default()
                                }]),
                                ..Delta::default()
                            }));
                        }
                    }
                    Some("content_block_delta") => {
                        let delta = &event["delta"];
                        match delta["type"].as_str() {
                            Some("text_delta") => {
                                if let Some(text) = delta["text"].as_str() {
                                    yield Ok(delta_chunk(&id, &model, Delta {
                                        content: Some(text.to_string()),
                                        ..Delta::default()
                                    }));
                                }
                            }
                            Some("input_json_delta") => {
                                let block_index = event["index"].as_i64().unwrap_or(0);
                                if let (Some(partial), Some(&oa_index)) =
                                    (delta["partial_json"].as_str(), block_to_tool.get(&block_index))
                                {
                                    tool_blocks_with_arguments.insert(block_index);
                                    yield Ok(delta_chunk(&id, &model, Delta {
                                        tool_calls: Some(vec![ToolCallDelta {
                                            index: oa_index,
                                            id: None,
                                            kind: None,
                                            function: Some(FunctionCallDelta {
                                                name: None,
                                                arguments: Some(partial.to_string()),
                                            }),
                                            ..ToolCallDelta::default()
                                        }]),
                                        ..Delta::default()
                                    }));
                                }
                            }
                            _ => {}
                        }
                    }
                    Some("content_block_stop") => {
                        let block_index = event["index"].as_i64().unwrap_or(0);
                        if !tool_blocks_with_arguments.contains(&block_index)
                            && let Some(&oa_index) = block_to_tool.get(&block_index)
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
                    Some("message_delta") => {
                        if let Some(sr) = event["delta"]["stop_reason"].as_str() {
                            finish = Some(map_stop_reason(sr).to_string());
                        }
                        let u = &event["usage"];
                        input_tokens = u["input_tokens"].as_i64().or(input_tokens);
                        cache_read = u["cache_read_input_tokens"].as_i64().or(cache_read);
                        cache_creation = u["cache_creation_input_tokens"].as_i64().or(cache_creation);
                        cache_details = serde_json::from_value(u["cache_creation"].clone()).ok().or(cache_details);
                        if let Some(ot) = event["usage"]["output_tokens"].as_i64() {
                            output_tokens = Some(ot);
                        }
                    }
                    Some("message_stop") => {
                        stopped = true;
                        break;
                    }
                    _ => {}
                }
            }

            if !stopped {
                yield Err(ProviderError::Parse("Anthropic stream ended before message_stop".into()));
                return;
            }
            // Terminal: finish chunk, then an OpenAI-style usage chunk (empty choices).
            let Some(finish) = finish else {
                yield Err(ProviderError::Parse("Anthropic stream ended without stop_reason".into()));
                return;
            };
            yield Ok(finish_chunk(&id, &model, finish));
            yield Ok(usage_chunk(&id, &model, Usage {
                prompt_tokens: input_tokens,
                completion_tokens: output_tokens,
                total_tokens: match (input_tokens, output_tokens) {
                    (Some(i), Some(o)) => Some(i + o),
                    _ => None,
                },
                cache_read_input_tokens: cache_read,
                cache_creation_input_tokens: cache_creation,
                cache_creation: cache_details,
                prompt_tokens_details: None,
            }));
        };
        Ok(Box::pin(stream))
    }

    async fn embeddings(
        &self,
        _req: &EmbeddingsRequest,
        _cfg: &ResolvedEmbeddingConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        Err(ProviderError::Status {
            status: 501,
            message: "Anthropic has no embeddings API".to_string(),
            retryable: false,
        })
    }

    /// Anthropic reports a model/parameter mismatch as a 400 whose `error.message`
    /// names the field in backticks, e.g. ``` `temperature` is deprecated for this
    /// model. ``` — there's no structured `param` field like OpenAI. So we gate on the
    /// message clearly meaning "this param isn't accepted" (deprecated / unsupported /
    /// not supported) and extract the backtick-quoted name for the executor to drop and
    /// retry. The gate keeps us from stripping fields on structural 400s (missing
    /// `messages`, bad `max_tokens` value, …).
    fn droppable_param(&self, err: &ProviderError) -> Option<String> {
        let ProviderError::Status {
            status, message, ..
        } = err
        else {
            return None;
        };
        if *status != 400 {
            return None;
        }
        let body: serde_json::Value = serde_json::from_str(message).ok()?;
        let detail = body.get("error")?.get("message")?.as_str()?;
        super::param_rejection(detail)
    }
}

const FORCED_TOOL_CHOICE_REFUSED: &str = "tool_choice: type \"tool\" and \"any\" are not supported";

fn status_error(
    status: reqwest::StatusCode,
    text: String,
    req: &ChatRequest,
    cfg: &ResolvedConfig,
) -> ProviderError {
    let refuses_forced_choice = status == reqwest::StatusCode::BAD_REQUEST
        && serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|body| {
                body.pointer("/error/message")?
                    .as_str()
                    .map(|m| m.contains(FORCED_TOOL_CHOICE_REFUSED))
            })
            .unwrap_or(false);
    if refuses_forced_choice {
        let choice = req
            .tool_choice
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_else(|| "\"required\"".to_string());
        return ProviderError::Rejected(format!(
            "Anthropic model {} does not support tool_choice {choice}; use \"auto\" or a model that supports forced tool use",
            cfg.model
        ));
    }
    ProviderError::Status {
        status: status.as_u16(),
        message: text,
        retryable: status.as_u16() == 429 || status.is_server_error(),
    }
}

// ── OpenAI → Anthropic (request) ─────────────────────────────────────────────

/// A part the hub carries but Anthropic cannot, rendered onto this edge's error type.
fn unsupported(err: UnsupportedPart) -> ProviderError {
    ProviderError::Rejected(format!(
        "Anthropic does not accept this input: {}",
        err.into_message()
    ))
}

fn empty_user_content() -> ProviderError {
    ProviderError::Rejected("user message content cannot be empty".to_string())
}

/// Anthropic takes bytes inline or by URL, so both [`DataSource`] arms map directly.
fn anthropic_source(source: &DataSource) -> Value {
    match source {
        DataSource::Inline { media_type, data } => json!({
            "type": "base64",
            "media_type": media_type,
            "data": data,
        }),
        DataSource::Url(url) => json!({ "type": "url", "url": url }),
    }
}

/// Canonical part → Anthropic content block.
///
/// The Messages API has no audio input, so audio is refused by name here rather than
/// dropped — a model answering confidently about audio it never received is the worse
/// failure. `detail` and `filename` have no Anthropic equivalent and stop at this edge.
fn anthropic_block(part: &ContentPart) -> Result<Value, UnsupportedPart> {
    match part {
        ContentPart::Text(text) => Ok(json!({ "type": "text", "text": text })),
        ContentPart::Image { source, .. } => {
            if let Some((media_type, data)) = source.inline() {
                if !ANTHROPIC_IMAGE_TYPES.contains(&media_type) {
                    return Err(UnsupportedPart::new(format!(
                        "'{media_type}' images (it reads jpeg, png, gif and webp)"
                    )));
                }
                if data.len() > ANTHROPIC_IMAGE_MAX_ENCODED_BYTES {
                    return Err(UnsupportedPart::new("an image over 10 MB as base64"));
                }
            }
            Ok(json!({
                "type": "image",
                "source": anthropic_source(source),
            }))
        }
        ContentPart::File { source, filename } => anthropic_document(source, filename.as_deref()),
        ContentPart::Audio { .. } => Err(UnsupportedPart::new(
            "unsupported content part type: 'input_audio'",
        )),
        // The Messages API has no video input of any kind.
        ContentPart::Video { .. } => Err(UnsupportedPart::new(
            "unsupported content part type: 'video_url'",
        )),
    }
}

const ANTHROPIC_IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];
const ANTHROPIC_IMAGE_MAX_ENCODED_BYTES: usize = 10_000_000;

fn anthropic_document(
    source: &DataSource,
    filename: Option<&str>,
) -> Result<Value, UnsupportedPart> {
    let Some(media_type) = source.media_type() else {
        return Ok(json!({ "type": "document", "source": anthropic_source(source) }));
    };
    match media::document_kind(media_type) {
        DocumentKind::Pdf => Ok(json!({ "type": "document", "source": anthropic_source(source) })),
        DocumentKind::TextLike => {
            let mut block = json!({
                "type": "document",
                "source": {
                    "type": "text",
                    "media_type": "text/plain",
                    "data": media::decode_text(source)?,
                },
            });
            if let Some(name) = filename.map(str::trim).filter(|n| !n.is_empty()) {
                block["title"] = json!(name);
            }
            Ok(block)
        }
        DocumentKind::Other => Err(UnsupportedPart::new(format!(
            "'{media_type}' documents (it reads PDF and text documents)"
        ))),
    }
}

fn anthropic_blocks(parts: &[ContentPart]) -> Result<Vec<Value>, ProviderError> {
    parts
        .iter()
        .map(|p| anthropic_block(p).map_err(unsupported))
        .collect()
}

fn has_media(parts: &[ContentPart]) -> bool {
    parts.iter().any(|p| !matches!(p, ContentPart::Text(_)))
}

fn without_empty_text(mut blocks: Vec<Value>) -> Vec<Value> {
    blocks.retain(|block| block.get("text").and_then(Value::as_str) != Some(""));
    blocks
}

/// [`anthropic_blocks`] for `m`'s parts, each block keeping the cache marker its part carried.
fn marked_anthropic_blocks(
    m: &Message,
    parts: &[ContentPart],
) -> Result<Vec<Value>, ProviderError> {
    let mut blocks = anthropic_blocks(parts)?;
    if let Some(Value::Array(raw)) = &m.content {
        // `content_view` projects the raw array one part to one part, so the two line up.
        for (block, part) in blocks.iter_mut().zip(raw) {
            if let Some(marker) = part.get(CACHE_CONTROL) {
                block[CACHE_CONTROL] = marker.clone();
            }
        }
    }
    let blocks = without_empty_text(blocks);
    if blocks.is_empty() {
        return Err(empty_user_content());
    }
    Ok(blocks)
}

fn to_anthropic_request(req: &ChatRequest, cfg: &ResolvedConfig) -> Result<Value, ProviderError> {
    let mut system_messages: Vec<&Message> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    for m in &req.messages {
        match m.role.as_str() {
            "system" => {
                // Brevity's directive goes after every cache marker, so a turn that skips it still
                // matches this turn's cached prefix; in `system` it would sit ahead of them all.
                let placed = crate::brevity::is_directive(m)
                    && pending_tool_results.is_empty()
                    && end_user_turn_with(messages.last_mut(), crate::brevity::DIRECTIVE);
                if !placed {
                    system_messages.push(m);
                }
            }
            "tool" => {
                // Accumulate consecutive tool results into one following user turn. A result
                // keeps one marker, on the whole result, so its own blocks go unmarked.
                let content = match m.content_view().map_err(unsupported)? {
                    ContentView::Parts(parts) if has_media(&parts) => {
                        Value::Array(without_empty_text(anthropic_blocks(&parts)?))
                    }
                    _ => json!(m.text().unwrap_or_default()),
                };
                let mut result = json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": content,
                });
                if let Some(marker) = m.last_cache_marker() {
                    result[CACHE_CONTROL] = marker.clone();
                }
                pending_tool_results.push(result);
            }
            "assistant" => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                messages.push(assistant_to_anthropic(m));
            }
            _ => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                // Unmarked text goes as one string, byte-identical to before markers and media
                // were kept; a marker or a media part needs blocks.
                let content_val = match m.content_view().map_err(unsupported)? {
                    ContentView::Parts(parts)
                        if has_media(&parts) || m.last_cache_marker().is_some() =>
                    {
                        Value::Array(marked_anthropic_blocks(m, &parts)?)
                    }
                    _ => match m.text() {
                        Some(text) if !text.is_empty() => Value::String(text),
                        _ => return Err(empty_user_content()),
                    },
                };
                messages.push(json!({ "role": "user", "content": content_val }));
            }
        }
    }
    flush_tool_results(&mut pending_tool_results, &mut messages);

    let max_tokens = cfg
        .max_tokens
        .or(req.max_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    if let Some(t) = cfg.temperature.or(req.temperature) {
        body["temperature"] = json!(t);
    }
    if let Some(stop) = stop_sequences(req) {
        body["stop_sequences"] = json!(stop);
    }
    if let Some(system) = system_field(&system_messages) {
        body["system"] = system;
    }
    if let Some(tools) = &req.tools {
        let translated: Vec<Value> = tools.iter().map(tool_to_anthropic).collect();
        if !translated.is_empty() {
            body["tools"] = json!(translated);
        }
    }
    if let Some(choice) = req.tool_choice.as_ref().and_then(tool_choice_to_anthropic) {
        body["tool_choice"] = choice;
    }
    Ok(body)
}

fn flush_tool_results(pending: &mut Vec<Value>, messages: &mut Vec<Value>) {
    if !pending.is_empty() {
        messages.push(json!({ "role": "user", "content": std::mem::take(pending) }));
    }
}

/// Add `text` as an unmarked block at the end of `turn` when it is a user turn; `false` when it
/// is not, or there is none. A string content becomes the one text block it stands for.
fn end_user_turn_with(turn: Option<&mut Value>, text: &str) -> bool {
    let Some(turn) = turn.filter(|t| t["role"] == "user") else {
        return false;
    };
    let block = json!({ "type": "text", "text": text });
    turn["content"] = match turn["content"].take() {
        Value::Array(mut blocks) => {
            if let Some(last) = blocks.last_mut() {
                trim_text_end(last);
            }
            blocks.push(block);
            Value::Array(blocks)
        }
        // Anthropic rejects an empty text block.
        Value::String(s) if s.is_empty() => json!([block]),
        content => {
            let mut first = json!({ "type": "text", "text": content });
            trim_text_end(&mut first);
            json!([first, block])
        }
    };
    true
}

/// Drop a text block's trailing whitespace, unless that would empty it. The text ahead of brevity's
/// directive needs this: the next request ends the turn at that text, and with trailing whitespace
/// it missed the prefix this request cached there (checked live, Nov 2026).
fn trim_text_end(block: &mut Value) {
    if block["type"] == "text"
        && let Some(text) = block["text"].as_str()
        && text.trim_end().len() != text.len()
        && !text.trim_end().is_empty()
    {
        block["text"] = json!(text.trim_end());
    }
}

/// Anthropic's top-level `system`: one joined string, unless a system message carries a cache
/// marker; then a block array, so each marker keeps its position.
fn system_field(system_messages: &[&Message]) -> Option<Value> {
    if !system_messages
        .iter()
        .any(|m| m.marked_text_parts().is_some())
    {
        let parts: Vec<String> = system_messages.iter().filter_map(|m| m.text()).collect();
        return (!parts.is_empty()).then(|| json!(parts.join("\n")));
    }
    let blocks: Vec<Value> = system_messages
        .iter()
        .flat_map(|m| match marked_text_blocks(m) {
            Some(blocks) => blocks,
            // Anthropic rejects an empty text block.
            None => m
                .text()
                .filter(|t| !t.is_empty())
                .map(|t| json!({ "type": "text", "text": t }))
                .into_iter()
                .collect(),
        })
        .collect();
    (!blocks.is_empty()).then(|| json!(blocks))
}

/// One Anthropic text block per text part, each keeping its cache marker; `None` when no part
/// carries one.
fn marked_text_blocks(m: &Message) -> Option<Vec<Value>> {
    let blocks = m
        .marked_text_parts()?
        .into_iter()
        .map(|part| {
            let mut block = json!({ "type": "text", "text": part["text"] });
            if let Some(marker) = part.get(CACHE_CONTROL) {
                block[CACHE_CONTROL] = marker.clone();
            }
            block
        })
        .collect();
    Some(without_empty_text(blocks))
}

fn assistant_to_anthropic(m: &Message) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    match marked_text_blocks(m) {
        Some(text_blocks) => blocks.extend(text_blocks),
        None => {
            if let Some(t) = m.text()
                && !t.is_empty()
            {
                blocks.push(json!({ "type": "text", "text": t }));
            }
        }
    }
    if let Some(tool_calls) = &m.tool_calls {
        for tc in tool_calls {
            // arguments is a JSON string → Anthropic `input` must be an object.
            let input: Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
            let mut block = json!({
                "type": "tool_use",
                "id": tc.id,
                "name": tc.function.name,
                "input": input,
            });
            if let Some(marker) = tc.extra.get(CACHE_CONTROL) {
                block[CACHE_CONTROL] = marker.clone();
            }
            blocks.push(block);
        }
    }
    match blocks.as_slice() {
        [] => json!({ "role": "assistant", "content": "" }),
        // Single text block → string content (both forms are valid; string is tidier), unless
        // it carries a marker, which only a block can hold.
        [only] if only["type"].as_str() == Some("text") && only.get(CACHE_CONTROL).is_none() => {
            json!({ "role": "assistant", "content": only["text"].clone() })
        }
        _ => json!({ "role": "assistant", "content": blocks }),
    }
}

fn tool_to_anthropic(t: &ToolDef) -> Value {
    let schema = t
        .function
        .parameters
        .clone()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let mut out = json!({ "name": t.function.name, "input_schema": schema });
    if let Some(desc) = &t.function.description {
        out["description"] = json!(desc);
    }
    if let Some(marker) = t.extra.get(CACHE_CONTROL) {
        out[CACHE_CONTROL] = marker.clone();
    }
    out
}

/// Add the router's own cache markers in the slots the agent left free: first the end of the
/// conversation, then the end of `system` (tools render before it, so that marker covers them
/// too). Each is a 5-minute marker, the cheapest write.
///
/// Explicit markers, not Anthropic's top-level `cache_control`: that one marks the last block,
/// which is brevity's directive when it applied, so the next turn would miss; and it is a 400
/// when the agent already holds four markers or marked the last block with another TTL.
fn add_cache_markers(body: &mut Value) {
    let mut free = MAX_CACHE_MARKERS.saturating_sub(count_cache_markers(body));
    // A 5-minute marker may not come before a 1-hour one, and `system` comes before every turn.
    let turns_hold_long_marker = turn_blocks(body).any(|block| block[CACHE_CONTROL]["ttl"] == "1h");

    if free > 0
        && let Some(block) = last_cacheable_block(&mut body["messages"])
        && block.get(CACHE_CONTROL).is_none()
    {
        block[CACHE_CONTROL] = json!({ "type": "ephemeral" });
        free -= 1;
    }
    if free > 0
        && !turns_hold_long_marker
        && let Some(block) = last_system_block(body)
        && block.get(CACHE_CONTROL).is_none()
    {
        block[CACHE_CONTROL] = json!({ "type": "ephemeral" });
    }
}

fn count_cache_markers(body: &Value) -> usize {
    ["tools", "system"]
        .into_iter()
        .filter_map(|field| body[field].as_array())
        .flatten()
        .chain(turn_blocks(body))
        .filter(|block| block.get(CACHE_CONTROL).is_some())
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

/// The last block of the last turn that can carry a marker, walking back past brevity's
/// directive and anything Anthropic won't mark. A string content becomes its one text block.
fn last_cacheable_block(messages: &mut Value) -> Option<&mut Value> {
    let turn = messages.as_array_mut()?.last_mut()?;
    as_blocks(turn.get_mut("content")?)?
        .iter_mut()
        .rev()
        .find(|block| match block["type"].as_str() {
            Some("text") => block["text"]
                .as_str()
                .is_some_and(|t| !t.is_empty() && t != crate::brevity::DIRECTIVE),
            Some("tool_use" | "tool_result" | "image" | "document") => true,
            _ => false,
        })
}

/// The last `system` block, unless it carries brevity's directive (it falls back there when the
/// last turn is not the user's): a marker after it would cache a prefix the next turn drops.
fn last_system_block(body: &mut Value) -> Option<&mut Value> {
    let last = as_blocks(body.get_mut("system")?)?.last_mut()?;
    let text = last["text"].as_str()?;
    (!text.is_empty() && !text.ends_with(crate::brevity::DIRECTIVE)).then_some(last)
}

/// `content` or `system` as a block array, turning a non-empty string into its one text block
/// (Anthropic reads the two forms alike). `None` for anything else.
fn as_blocks(field: &mut Value) -> Option<&mut Vec<Value>> {
    if let Some(text) = field.as_str().filter(|t| !t.is_empty()) {
        *field = json!([{ "type": "text", "text": text }]);
    }
    field.as_array_mut()
}

/// OpenAI `stop` (a string or an array of strings; not a named IR field, so it sits in
/// `extra`) as Anthropic `stop_sequences`. `None` when absent or empty.
fn stop_sequences(req: &ChatRequest) -> Option<Vec<&str>> {
    let stop: Vec<&str> = match req.extra.get("stop")? {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    (!stop.is_empty()).then_some(stop)
}

fn tool_choice_to_anthropic(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(s) => match s.as_str() {
            "required" => Some(json!({ "type": "any" })),
            "none" => Some(json!({ "type": "none" })),
            _ => Some(json!({ "type": "auto" })),
        },
        Value::Object(o) => o
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .map(|name| json!({ "type": "tool", "name": name })),
        _ => None,
    }
}

// ── Anthropic → OpenAI (response) ────────────────────────────────────────────

fn from_anthropic_response(body: &Value, model: &str) -> Result<ChatResponse, ProviderError> {
    let id = body["id"].as_str().unwrap_or_default();

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    if let Some(blocks) = body["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
                Some("tool_use") => tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_string(),
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        // input (object) → arguments (JSON string)
                        arguments: block
                            .get("input")
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                    extra: Map::new(),
                }),
                _ => {} // ignore other block types (e.g. thinking)
            }
        }
    }

    // Match OpenAI: content is null when the turn is purely tool calls.
    let content = if tool_calls.is_empty() {
        Some(Value::String(text))
    } else if text.is_empty() {
        Some(Value::Null)
    } else {
        Some(Value::String(text))
    };

    let stop_reason = body["stop_reason"]
        .as_str()
        .ok_or_else(|| ProviderError::Parse("Anthropic response has no stop_reason".into()))?;
    let finish_reason = map_stop_reason(stop_reason);

    let usage = body.get("usage").map(|u| {
        let input = u["input_tokens"].as_i64();
        let output = u["output_tokens"].as_i64();
        Usage {
            prompt_tokens: input,
            completion_tokens: output,
            total_tokens: match (input, output) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            },
            cache_read_input_tokens: u["cache_read_input_tokens"].as_i64(),
            cache_creation_input_tokens: u["cache_creation_input_tokens"].as_i64(),
            cache_creation: serde_json::from_value(u["cache_creation"].clone()).ok(),
            prompt_tokens_details: None,
        }
    });

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
        // Refusal/content-filter terminals and future unknown reasons fail closed.
        _ => "content_filter",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tools_strict_flag_is_never_sent_to_anthropic() {
        let tool: ToolDef = serde_json::from_value(serde_json::json!({"type": "function",
            "function": {"name": "read_log", "parameters": {"type": "object"}, "strict": true}}))
        .unwrap();
        assert!(tool_to_anthropic(&tool).get("strict").is_none());
    }

    fn resolved() -> ResolvedConfig {
        ResolvedConfig {
            agent_id: String::new(),
            compress_enabled: false,
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet-20241022".into(),
            litellm_model: "anthropic/claude-3-5-sonnet-20241022".into(),
            api_key: "sk-ant-test".into(),
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
        }
    }

    #[test]
    fn droppable_param_extracts_backticked_field_from_anthropic_400() {
        let provider = AnthropicProvider::new(reqwest::Client::new(), "http://x".into());
        // The exact error shape claude-opus-4-8 returns for a deprecated temperature.
        let deprecated = ProviderError::Status {
            status: 400,
            message: json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "`temperature` is deprecated for this model."
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(
            provider.droppable_param(&deprecated).as_deref(),
            Some("temperature")
        );

        // An "unsupported" phrasing is also droppable.
        let unsupported = ProviderError::Status {
            status: 400,
            message: json!({
                "error": { "message": "`top_p` is not supported for this model." }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(
            provider.droppable_param(&unsupported).as_deref(),
            Some("top_p")
        );

        // A structural 400 (no "deprecated/unsupported" wording) must NOT be treated as a
        // droppable param, even though it mentions a field in backticks.
        let structural = ProviderError::Status {
            status: 400,
            message: json!({
                "error": { "message": "`max_tokens`: must be greater than 0" }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(provider.droppable_param(&structural), None);

        // 5xx / transport / unparseable bodies are never droppable.
        assert_eq!(
            provider.droppable_param(&ProviderError::Status {
                status: 529,
                message: "overloaded".into(),
                retryable: true
            }),
            None
        );
        assert_eq!(
            provider.droppable_param(&ProviderError::Transport("timeout".into())),
            None
        );
    }

    #[test]
    fn request_extracts_system_and_translates_tools() {
        // Mirrors REQUEST_JOURNEY steps 2 → 5.
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "temperature": 0.1,
            "max_tokens": 4000,
            "tool_choice": "auto",
            "messages": [
                { "role": "system", "content": "You are a Translation agent." },
                { "role": "user", "content": "translate to hindi: my name is csg" }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "translate_text",
                    "description": "Translate plain text",
                    "parameters": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }
                }
            }]
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved()).unwrap();

        assert_eq!(body["model"], "claude-3-5-sonnet-20241022");
        assert_eq!(body["max_tokens"], 4000); // from request (config didn't set it)
        assert_eq!(body["temperature"], 0.1);
        assert_eq!(body["system"], "You are a Translation agent.");
        // system removed from messages; only the user turn remains
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
        // tool: no "type":"function" wrapper; parameters → input_schema
        let tool = &body["tools"][0];
        assert_eq!(tool["name"], "translate_text");
        assert!(tool.get("type").is_none());
        assert_eq!(tool["input_schema"]["properties"]["text"]["type"], "string");
        assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
    }

    #[test]
    fn request_sends_tool_choice_none_and_stop_sequences() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "count to 10" }],
            "tools": [{ "type": "function", "function": { "name": "f", "parameters": { "type": "object" } } }],
            "tool_choice": "none",
            "stop": ["6"]
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved()).unwrap();
        assert_eq!(body["tool_choice"], json!({ "type": "none" }));
        assert_eq!(body["stop_sequences"], json!(["6"]));
    }

    #[test]
    fn request_accepts_a_single_stop_string() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "hi" }],
            "stop": "END"
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved()).unwrap();
        assert_eq!(body["stop_sequences"], json!(["END"]));
        assert!(body.get("stop").is_none());
    }

    #[test]
    fn drops_unsupported_openai_params() {
        // drop_params: OpenAI-only params (top_p, frequency_penalty, …) live in `extra`
        // and must NOT be forwarded to Anthropic — the translator only emits mapped fields.
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "hi" }],
            "top_p": 0.9,
            "frequency_penalty": 0.5,
            "logit_bias": { "50256": -100 }
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved()).unwrap();
        assert!(body.get("top_p").is_none());
        assert!(body.get("frequency_penalty").is_none());
        assert!(body.get("logit_bias").is_none());
    }

    #[test]
    fn max_tokens_defaults_when_unset() {
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let body = to_anthropic_request(&req, &resolved()).unwrap();
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn multi_turn_tool_history_becomes_tool_use_and_tool_result() {
        // Mirrors REQUEST_JOURNEY step 9 (C3).
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "user", "content": "translate to hindi: my name is csg" },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "toolu_01", "type": "function",
                    "function": { "name": "translate_text", "arguments": "{\"text\":\"my name is csg\",\"target_language\":\"hi\"}" }
                }]},
                { "role": "tool", "tool_call_id": "toolu_01", "content": "मेरा नाम csg है" }
            ]
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved()).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        // assistant turn carries a tool_use block with the parsed input object
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["id"], "toolu_01");
        assert_eq!(msgs[1]["content"][0]["input"]["target_language"], "hi");
        // tool result becomes a user turn with a tool_result block + matching id
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_01");
        assert_eq!(msgs[2]["content"][0]["content"], "मेरा नाम csg है");
    }

    #[test]
    fn mixed_assistant_text_and_multiple_calls_round_trip_to_anthropic() {
        let req: ChatRequest = serde_json::from_value(json!({
            "messages":[
                {"role":"user","content":"go"},
                {"role":"assistant","content":"Calling both.","tool_calls":[
                    {"id":"c1","type":"function","function":{"name":"one","arguments":"{\"x\":1}"}},
                    {"id":"c2","type":"function","function":{"name":"two","arguments":"{\"y\":2}"}}
                ]},
                {"role":"tool","tool_call_id":"c1","content":"one done"},
                {"role":"tool","tool_call_id":"c2","content":"two done"}
            ]
        }))
        .unwrap();
        let body = to_anthropic_request(&req, &resolved()).unwrap();
        assert_eq!(body["messages"][1]["content"][0]["text"], "Calling both.");
        assert_eq!(body["messages"][1]["content"][1]["name"], "one");
        assert_eq!(body["messages"][1]["content"][2]["name"], "two");
        assert_eq!(body["messages"][2]["content"].as_array().unwrap().len(), 2);
    }

    /// An Anthropic-SDK agent's request through the router and back out to Anthropic.
    fn round_trip(agent_request: &Value) -> Value {
        use crate::inbound::InboundParser;
        let ir = crate::inbound::AnthropicInbound
            .parse_chat(agent_request.clone())
            .unwrap();
        to_anthropic_request(&ir, &resolved()).unwrap()
    }

    #[test]
    fn cache_markers_on_system_tools_tool_results_and_user_text_survive_the_round_trip() {
        // Four markers, Anthropic's limit, one in each place an agent commonly puts them.
        let ephemeral = json!({ "type": "ephemeral" });
        let agent_request = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "system": [
                { "type": "text", "text": "You are a coding agent." },
                { "type": "text", "text": "Repository map.", "cache_control": { "type": "ephemeral", "ttl": "1h" } }
            ],
            "tools": [
                { "name": "read", "description": "Read a file", "input_schema": { "type": "object" } },
                { "name": "edit", "description": "Edit a file", "input_schema": { "type": "object" }, "cache_control": ephemeral }
            ],
            "messages": [
                { "role": "user", "content": "Fix the failing test." },
                { "role": "assistant", "content": [
                    { "type": "text", "text": "Reading it first." },
                    { "type": "tool_use", "id": "toolu_1", "name": "read", "input": { "path": "lib.rs" } }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}", "cache_control": ephemeral }
                ]},
                { "role": "user", "content": [
                    { "type": "text", "text": "Also run clippy." },
                    { "type": "text", "text": "Then stop.", "cache_control": ephemeral }
                ]}
            ]
        });

        let sent = round_trip(&agent_request);

        assert_eq!(sent["system"], agent_request["system"]);
        assert_eq!(sent["tools"], agent_request["tools"]);
        assert_eq!(sent["messages"], agent_request["messages"]);
    }

    #[test]
    fn cache_markers_on_an_assistant_turn_survive_the_round_trip() {
        let ephemeral = json!({ "type": "ephemeral" });
        let agent_request = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [
                { "role": "user", "content": "Fix the failing test." },
                { "role": "assistant", "content": [
                    { "type": "text", "text": "Reading it first.", "cache_control": ephemeral },
                    { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {}, "cache_control": ephemeral }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}" }
                ]}
            ]
        });

        let sent = round_trip(&agent_request);

        assert_eq!(sent["messages"], agent_request["messages"]);
    }

    #[test]
    fn a_marker_inside_a_tool_result_ends_the_cached_prefix_at_the_end_of_the_result() {
        let ephemeral = json!({ "type": "ephemeral" });
        let sent = round_trip(&json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [
                { "role": "user", "content": "Fix the failing test." },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {} }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": [
                        { "type": "text", "text": "fn main() {}\n" },
                        { "type": "text", "text": "fn helper() {}", "cache_control": ephemeral }
                    ]}
                ]}
            ]
        }));

        assert_eq!(
            sent["messages"][2]["content"][0],
            json!({
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": "fn main() {}\nfn helper() {}",
                "cache_control": ephemeral
            })
        );
    }

    #[test]
    fn an_openai_agents_content_part_markers_reach_anthropic() {
        // OpenRouter's convention for marking Claude prompts from an OpenAI SDK: `cache_control`
        // on a text content part. Now that the IR keeps it, Anthropic receives it too.
        use crate::inbound::{InboundParser, OpenAiInbound};
        let ephemeral = json!({ "type": "ephemeral" });
        let req = OpenAiInbound
            .parse_chat(json!({
                "model": "claude-sonnet-4-5",
                "messages": [
                    { "role": "system", "content": [
                        { "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral }
                    ]},
                    { "role": "user", "content": [
                        { "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }
                    ]}
                ]
            }))
            .unwrap();

        let sent = to_anthropic_request(&req, &resolved()).unwrap();

        assert_eq!(
            sent["system"],
            json!([{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral }])
        );
        assert_eq!(
            sent["messages"][0]["content"],
            json!([{ "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }])
        );
    }

    #[test]
    fn a_request_without_cache_markers_is_sent_as_before() {
        let sent = round_trip(&json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "system": [
                { "type": "text", "text": "You are a coding agent. " },
                { "type": "text", "text": "Repository map." }
            ],
            "tools": [{ "name": "read", "input_schema": { "type": "object" } }],
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "Fix the test. " },
                { "type": "text", "text": "Then stop." }
            ]}]
        }));

        assert_eq!(sent["system"], "You are a coding agent. Repository map.");
        assert_eq!(sent["messages"][0]["content"], "Fix the test. Then stop.");
        assert!(sent["tools"][0].get("cache_control").is_none());
    }

    #[test]
    fn cache_markers_beside_media_survive_the_round_trip() {
        let ephemeral = json!({ "type": "ephemeral" });
        let image = json!({ "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" });
        let agent_request = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [
                { "role": "user", "content": [
                    { "type": "image", "source": image, "cache_control": ephemeral },
                    { "type": "text", "text": "What is this?" }
                ]},
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_1", "name": "zoom", "input": {} }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "cache_control": ephemeral, "content": [
                        { "type": "text", "text": "Zoomed." },
                        { "type": "image", "source": image }
                    ]}
                ]}
            ]
        });

        let sent = round_trip(&agent_request);

        assert_eq!(sent["messages"], agent_request["messages"]);
    }

    /// Brevity's seam, run as the handler runs it: `Ok` when it added the directive.
    fn brevity(ir: &mut ChatRequest) -> Result<(), crate::brevity::Skipped> {
        let cfg = crate::config::GatewayConfig {
            brevity_enabled: true,
            brevity_min_bytes: 0,
            ..Default::default()
        };
        let resolved = ResolvedConfig {
            compress_enabled: true,
            ..resolved()
        };
        crate::brevity::apply(ir, &cfg, &resolved, None)
    }

    fn directive_block() -> Value {
        json!({ "type": "text", "text": crate::brevity::DIRECTIVE })
    }

    #[test]
    fn the_text_ahead_of_brevitys_directive_loses_its_trailing_whitespace() {
        // The next turn ends at that text; with the whitespace it missed this turn's cache live.
        let ephemeral = json!({ "type": "ephemeral" });
        for (content, trimmed) in [
            (
                json!("Fix the failing test.\n"),
                json!({ "type": "text", "text": "Fix the failing test." }),
            ),
            // The agent's marker keeps the turn a block array.
            (
                json!([{ "type": "text", "text": "Fix the failing test.  \n", "cache_control": ephemeral }]),
                json!({ "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }),
            ),
        ] {
            let mut ir: ChatRequest = serde_json::from_value(json!({
                "messages": [{ "role": "user", "content": content }]
            }))
            .unwrap();
            brevity(&mut ir).unwrap();

            let sent = to_anthropic_request(&ir, &resolved()).unwrap();

            assert_eq!(
                sent["messages"][0]["content"],
                json!([trimmed, directive_block()])
            );
        }

        // Whitespace alone stays: Anthropic rejects an empty text block.
        let mut ir: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "\n" }]
        }))
        .unwrap();
        brevity(&mut ir).unwrap();
        let sent = to_anthropic_request(&ir, &resolved()).unwrap();
        assert_eq!(sent["messages"][0]["content"][0]["text"], "\n");
    }

    #[test]
    fn a_turn_that_skips_brevity_still_matches_the_cached_prefix_of_one_that_applied_it() {
        use crate::inbound::InboundParser;
        let ephemeral = json!({ "type": "ephemeral" });
        let mut agent_request = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "system": [{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral }],
            "tools": [{ "name": "read", "input_schema": { "type": "object" } }],
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }
            ]}]
        });
        let mut first = crate::inbound::AnthropicInbound
            .parse_chat(agent_request.clone())
            .unwrap();
        assert_eq!(brevity(&mut first), Ok(()));
        let first = to_anthropic_request(&first, &resolved()).unwrap();

        // The next turn is mid tool-loop, where brevity stands down.
        let history = agent_request["messages"].as_array_mut().unwrap();
        history.push(json!({ "role": "assistant", "content": [
            { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {} }
        ]}));
        history.push(json!({ "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}", "cache_control": ephemeral }
        ]}));
        let mut second = crate::inbound::AnthropicInbound
            .parse_chat(agent_request)
            .unwrap();
        assert_eq!(
            brevity(&mut second),
            Err(crate::brevity::Skipped::ToolContinuation)
        );
        let second = to_anthropic_request(&second, &resolved()).unwrap();

        // Tools, system, then the first user turn up to its marker: the cached prefix, unchanged.
        assert_eq!(first["tools"], second["tools"]);
        assert_eq!(first["system"], second["system"]);
        let mut first_turn = second["messages"][0]["content"].clone();
        first_turn.as_array_mut().unwrap().push(directive_block());
        assert_eq!(first["messages"][0]["content"], first_turn);
    }

    #[test]
    fn the_directive_follows_a_plain_user_turn_as_its_own_block() {
        let mut ir: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "why did the deploy fail?" }
            ]
        }))
        .unwrap();
        brevity(&mut ir).unwrap();

        let body = to_anthropic_request(&ir, &resolved()).unwrap();

        assert_eq!(body["system"], "You are helpful.");
        assert_eq!(
            body["messages"][0]["content"],
            json!([{ "type": "text", "text": "why did the deploy fail?" }, directive_block()])
        );
    }

    #[test]
    fn the_directive_stays_in_system_when_the_last_turn_is_not_the_users() {
        // No user turn can end the request after an assistant prefill or bare tool results, so
        // the directive goes where it went before.
        let joined = format!("You are helpful.\n{}", crate::brevity::DIRECTIVE);
        for last in [
            json!({ "role": "assistant", "content": "The deploy failed because" }),
            json!({ "role": "tool", "tool_call_id": "toolu_1", "content": "exit 1" }),
        ] {
            let mut ir: ChatRequest = serde_json::from_value(json!({
                "messages": [
                    { "role": "system", "content": "You are helpful." },
                    { "role": "user", "content": "why did the deploy fail?" },
                    last
                ]
            }))
            .unwrap();
            brevity(&mut ir).unwrap();

            let body = to_anthropic_request(&ir, &resolved()).unwrap();

            assert_eq!(body["system"], joined.as_str());
            assert_eq!(body["messages"][0]["content"], "why did the deploy fail?");
        }
    }

    #[test]
    fn response_tool_use_becomes_openai_tool_calls() {
        // Mirrors REQUEST_JOURNEY steps 6 → 7 (C2).
        let anthropic = json!({
            "id": "msg_01", "type": "message", "role": "assistant",
            "model": "claude-3-5-sonnet-20241022",
            "content": [{
                "type": "tool_use", "id": "toolu_01", "name": "translate_text",
                "input": { "text": "my name is csg", "target_language": "hi" }
            }],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 463, "output_tokens": 58 }
        });
        let resp = from_anthropic_response(&anthropic, "claude-3-5-sonnet-20241022").unwrap();
        assert_eq!(resp.model, "claude-3-5-sonnet-20241022");
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let msg = &resp.choices[0].message;
        assert_eq!(msg.content, Some(Value::Null)); // pure tool call → null content
        let tc = &msg.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.id, "toolu_01");
        assert_eq!(tc.function.name, "translate_text");
        // arguments is a JSON string round-tripping the input object
        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["target_language"], "hi");
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(463));
        assert_eq!(usage.completion_tokens, Some(58));
        assert_eq!(usage.total_tokens, Some(521));
    }

    #[test]
    fn non_streaming_captures_cache_read_and_creation_tokens() {
        let anthropic = json!({
            "id": "msg_02", "type": "message", "role": "assistant",
            "model": "claude-3-5-sonnet-20241022",
            "content": [{ "type": "text", "text": "hi" }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100, "output_tokens": 20,
                "cache_read_input_tokens": 1500,
                "cache_creation_input_tokens": 300
            }
        });
        let resp = from_anthropic_response(&anthropic, "claude-3-5-sonnet-20241022").unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.cache_read_input_tokens, Some(1500));
        assert_eq!(usage.cache_creation_input_tokens, Some(300));
    }

    #[tokio::test]
    async fn streaming_captures_cache_tokens_from_message_start() {
        // Cache counts arrive on the message_start usage block.
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_3\",\"usage\":{\"input_tokens\":50,\"cache_read_input_tokens\":700,\"cache_creation_input_tokens\":80}}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        server
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({ "stream": true })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o", "stream": true,
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let stream = provider.chat_stream(&req, &resolved()).await.unwrap();
        let chunks: Vec<ChatChunk> = stream.filter_map(|r| async { r.ok() }).collect().await;
        let usage = chunks.into_iter().find_map(|c| c.usage).unwrap();
        assert_eq!(usage.cache_read_input_tokens, Some(700));
        assert_eq!(usage.cache_creation_input_tokens, Some(80));
    }

    #[test]
    fn response_text_becomes_content_and_stop() {
        // Mirrors REQUEST_JOURNEY steps 10 → 11.
        let anthropic = json!({
            "id": "msg_02", "type": "message", "role": "assistant",
            "content": [{ "type": "text", "text": "Here is the translation." }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 531, "output_tokens": 42 }
        });
        let resp = from_anthropic_response(&anthropic, "claude-3-5-sonnet-20241022").unwrap();
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("Here is the translation.".into()))
        );
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn stop_reasons_are_explicit_and_unknown_reasons_fail_closed() {
        for (reason, expected) in [
            ("end_turn", "stop"),
            ("stop_sequence", "stop"),
            ("tool_use", "tool_calls"),
            ("max_tokens", "length"),
            ("refusal", "content_filter"),
            ("content_filter", "content_filter"),
            ("future_reason", "content_filter"),
        ] {
            assert_eq!(map_stop_reason(reason), expected);
        }
        assert!(from_anthropic_response(&json!({"content":[]}), "m").is_err());
    }

    #[tokio::test]
    async fn stream_reassembles_tool_call_deltas() {
        // Anthropic streams a tool call as content_block_start + input_json_delta
        // fragments; we must re-emit OpenAI delta.tool_calls[] chunks (C2 streaming).
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":50}}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"translate_text\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"text\\\":\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"hi\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":12}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        server
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({ "stream": true })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o", "stream": true,
            "messages": [{ "role": "user", "content": "translate" }]
        }))
        .unwrap();
        let stream = provider.chat_stream(&req, &resolved()).await.unwrap();
        let chunks: Vec<ChatChunk> = stream.filter_map(|r| async { r.ok() }).collect().await;

        // Reassemble the streamed tool-call arguments across deltas.
        let mut name = String::new();
        let mut args = String::new();
        let mut finish = None;
        let mut usage = None;
        for c in &chunks {
            if let Some(choice) = c.choices.first() {
                if let Some(tcs) = &choice.delta.tool_calls {
                    for tc in tcs {
                        if let Some(f) = &tc.function {
                            if let Some(n) = &f.name {
                                name = n.clone();
                            }
                            if let Some(a) = &f.arguments {
                                args.push_str(a);
                            }
                        }
                    }
                }
                if choice.finish_reason.is_some() {
                    finish = choice.finish_reason.clone();
                }
            }
            if c.usage.is_some() {
                usage = c.usage.clone();
            }
        }
        assert_eq!(name, "translate_text");
        assert_eq!(serde_json::from_str::<Value>(&args).unwrap()["text"], "hi");
        assert_eq!(finish.as_deref(), Some("tool_calls"));
        let usage = usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(50));
        assert_eq!(usage.completion_tokens, Some(12));
        assert_eq!(usage.total_tokens, Some(62));
    }

    #[tokio::test]
    async fn stream_text_deltas_become_content() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_2\",\"usage\":{\"input_tokens\":5}}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        server
            .mock("POST", "/messages")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;
        let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "stream": true, "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let chunks: Vec<ChatChunk> = provider
            .chat_stream(&req, &resolved())
            .await
            .unwrap()
            .filter_map(|r| async { r.ok() })
            .collect()
            .await;
        let text: String = chunks
            .iter()
            .filter_map(|c| c.choices.first().and_then(|ch| ch.delta.content.clone()))
            .collect();
        assert_eq!(text, "Hello");
        let finish = chunks
            .iter()
            .find_map(|c| c.choices.first().and_then(|ch| ch.finish_reason.clone()));
        assert_eq!(finish.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn stream_rejects_malformed_error_and_premature_eof() {
        for payload in [
            "data: {not-json}\n\n",
            "data: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\"}}\n\n",
        ] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", "/messages")
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body(payload)
                .create_async()
                .await;
            let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
            let req: ChatRequest =
                serde_json::from_value(json!({"messages":[{"role":"user","content":"hi"}]}))
                    .unwrap();
            let results: Vec<_> = provider
                .chat_stream(&req, &resolved())
                .await
                .unwrap()
                .collect()
                .await;
            assert!(results.iter().any(Result::is_err), "accepted {payload}");
            assert!(
                !results
                    .iter()
                    .filter_map(|result| result.as_ref().ok())
                    .any(|chunk| chunk
                        .choices
                        .first()
                        .is_some_and(|choice| choice.finish_reason.is_some()))
            );
        }
    }

    #[tokio::test]
    async fn chat_calls_messages_endpoint_and_translates() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/messages")
            .match_header("x-api-key", "sk-ant-test")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "claude-3-5-sonnet-20241022", "system": "You are helpful."
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_9", "type": "message", "role": "assistant",
                    "content": [{ "type": "text", "text": "नमस्ते" }],
                    "stop_reason": "end_turn",
                    "usage": { "input_tokens": 10, "output_tokens": 3 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "You are helpful." },
                { "role": "user", "content": "hi" }
            ]
        }))
        .unwrap();
        let resp = provider.chat(&req, &resolved()).await.unwrap();
        m.assert_async().await;
        assert_eq!(resp.model, "claude-3-5-sonnet-20241022");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("नमस्ते"));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(13));
    }

    #[tokio::test]
    async fn a_model_that_refuses_forced_tool_choice_is_named_with_the_choice() {
        for stream in [false, true] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", "/messages")
                .with_status(400)
                .with_header("content-type", "application/json")
                .with_body(
                    json!({
                        "type": "error",
                        "error": {
                            "type": "invalid_request_error",
                            "message": "tool_choice: type \"tool\" and \"any\" are not supported for this model"
                        }
                    })
                    .to_string(),
                )
                .create_async()
                .await;
            let provider = AnthropicProvider::new(reqwest::Client::new(), server.url());
            let req: ChatRequest = serde_json::from_value(json!({
                "messages": [{ "role": "user", "content": "hi" }],
                "tool_choice": "required",
                "tools": [{ "type": "function", "function": { "name": "lookup", "parameters": { "type": "object" } } }]
            }))
            .unwrap();

            let err = if stream {
                provider.chat_stream(&req, &resolved()).await.err().unwrap()
            } else {
                provider.chat(&req, &resolved()).await.unwrap_err()
            };

            let ProviderError::Rejected(message) = err else {
                panic!("stream={stream}: expected a by-name refusal, got {err:?}");
            };
            assert!(message.contains("claude-3-5-sonnet-20241022"), "{message}");
            assert!(message.contains("tool_choice \"required\""), "{message}");
        }
    }

    /// The body the provider sends with `TOKEN_CACHE_MARKERS` on.
    fn with_router_markers(ir: &ChatRequest) -> Value {
        AnthropicProvider::new(reqwest::Client::new(), "http://x".into())
            .with_cache_markers(true)
            .request_body(ir, &resolved())
            .unwrap()
    }

    fn anthropic_agent(request: Value) -> ChatRequest {
        use crate::inbound::InboundParser;
        crate::inbound::AnthropicInbound
            .parse_chat(request)
            .unwrap()
    }

    #[test]
    fn the_router_marks_the_end_of_the_conversation_then_of_system() {
        let ir: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": "Fix the failing test." }
            ]
        }))
        .unwrap();

        let body = with_router_markers(&ir);

        let ephemeral = json!({ "type": "ephemeral" });
        assert_eq!(
            body["system"],
            json!([{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral }])
        );
        assert_eq!(
            body["messages"][0]["content"],
            json!([{ "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }])
        );
    }

    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    fn anthropic_body(request: Value) -> Result<Value, ProviderError> {
        let ir: ChatRequest = serde_json::from_value(request).unwrap();
        to_anthropic_request(&ir, &resolved())
    }

    fn image_block() -> Value {
        json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } })
    }

    #[test]
    fn an_anthropic_text_document_goes_back_to_anthropic_as_a_text_document() {
        use crate::inbound::InboundParser;
        let ir = crate::inbound::anthropic::AnthropicInbound
            .parse_chat(json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 64,
                "messages": [{ "role": "user", "content": [
                    { "type": "document", "title": "sales.csv",
                      "source": { "type": "text", "media_type": "text/plain", "data": "region,q2\nEMEA,913" } }
                ] }]
            }))
            .unwrap();
        let body = to_anthropic_request(&ir, &resolved()).unwrap();
        let block = &body["messages"][0]["content"][0];
        assert_eq!(block["type"], "document");
        assert_eq!(
            block["source"],
            json!({ "type": "text", "media_type": "text/plain", "data": "region,q2\nEMEA,913" })
        );
        assert_eq!(block["title"], "sales.csv");
    }

    #[test]
    fn an_empty_text_part_beside_an_image_is_left_out() {
        let body = anthropic_body(json!({ "messages": [{ "role": "user", "content": [
            { "type": "text", "text": "" },
            { "type": "image_url", "image_url": { "url": PNG } }
        ]}]}))
        .unwrap();

        assert_eq!(body["messages"][0]["content"], json!([image_block()]));
    }

    #[test]
    fn an_empty_text_part_leaves_with_its_cache_marker_and_real_text_keeps_its_own() {
        let ephemeral = json!({ "type": "ephemeral" });
        let body = anthropic_body(json!({ "messages": [{ "role": "user", "content": [
            { "type": "text", "text": "What colours?", "cache_control": ephemeral },
            { "type": "text", "text": "", "cache_control": ephemeral },
            { "type": "image_url", "image_url": { "url": PNG } },
            { "type": "text", "text": "Be brief." }
        ]}]}))
        .unwrap();

        assert_eq!(
            body["messages"][0]["content"],
            json!([
                { "type": "text", "text": "What colours?", "cache_control": ephemeral },
                image_block(),
                { "type": "text", "text": "Be brief." }
            ])
        );
    }

    #[test]
    fn an_empty_text_part_inside_a_tool_result_with_an_image_is_left_out() {
        let body = anthropic_body(json!({ "messages": [
            { "role": "user", "content": "Read the chart." },
            { "role": "assistant", "content": null, "tool_calls": [{
                "id": "toolu_1", "type": "function",
                "function": { "name": "read_file", "arguments": "{}" }
            }]},
            { "role": "tool", "tool_call_id": "toolu_1", "content": [
                { "type": "text", "text": "" },
                { "type": "image_url", "image_url": { "url": PNG } }
            ]}
        ]}))
        .unwrap();

        assert_eq!(
            body["messages"][2]["content"][0]["content"],
            json!([image_block()])
        );
    }

    #[test]
    fn a_user_turn_of_only_empty_text_is_refused_by_name() {
        let err = anthropic_body(json!({ "messages": [{ "role": "user", "content": [
            { "type": "text", "text": "", "cache_control": { "type": "ephemeral" } }
        ]}]}))
        .unwrap_err();

        assert!(
            matches!(&err, ProviderError::Rejected(msg) if msg.contains("empty")),
            "{err:?}"
        );
    }

    #[test]
    fn a_marked_system_prompt_drops_empty_text_and_an_all_empty_one_is_not_sent() {
        let ephemeral = json!({ "type": "ephemeral" });
        let kept = anthropic_body(json!({ "messages": [
            { "role": "system", "content": [
                { "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral },
                { "type": "text", "text": "" }
            ]},
            { "role": "user", "content": "hi" }
        ]}))
        .unwrap();
        let none = anthropic_body(json!({ "messages": [
            { "role": "system", "content": [
                { "type": "text", "text": "", "cache_control": ephemeral }
            ]},
            { "role": "user", "content": "hi" }
        ]}))
        .unwrap();

        assert_eq!(
            kept["system"],
            json!([{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral }])
        );
        assert!(none.get("system").is_none(), "{none}");
    }

    #[test]
    fn a_request_without_a_system_prompt_is_sent_without_a_system_field() {
        let image = "data:image/png;base64,iVBORw0KGgo=";
        let requests = [
            json!({ "messages": [{ "role": "user", "content": "hi" }] }),
            json!({ "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "What colours?" },
                { "type": "image_url", "image_url": { "url": image } }
            ]}]}),
            json!({
                "tools": [{ "type": "function", "function": {
                    "name": "read", "parameters": { "type": "object" }
                }}],
                "messages": [
                    { "role": "user", "content": "Fix the failing test." },
                    { "role": "assistant", "content": null, "tool_calls": [{
                        "id": "toolu_1", "type": "function",
                        "function": { "name": "read", "arguments": "{}" }
                    }]},
                    { "role": "tool", "tool_call_id": "toolu_1", "content": "fn main() {}" }
                ]
            }),
        ];

        for request in requests {
            let ir: ChatRequest = serde_json::from_value(request.clone()).unwrap();

            let body = with_router_markers(&ir);

            assert!(body.get("system").is_none(), "{request}");
            assert_eq!(count_cache_markers(&body), 1, "{request}");
        }
    }

    #[test]
    fn the_routers_last_marker_goes_ahead_of_brevitys_directive() {
        let mut ir: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "why did the deploy fail?" }]
        }))
        .unwrap();
        assert_eq!(brevity(&mut ir), Ok(()));

        let body = with_router_markers(&ir);

        assert_eq!(
            body["messages"][0]["content"],
            json!([
                { "type": "text", "text": "why did the deploy fail?", "cache_control": { "type": "ephemeral" } },
                directive_block()
            ])
        );
    }

    #[test]
    fn an_agent_holding_all_four_markers_gets_none_from_the_router() {
        let ephemeral = json!({ "type": "ephemeral" });
        let ir = anthropic_agent(json!({
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
        }));

        // Neither the end of the conversation nor `system` is marked, but a fifth marker is a 400.
        let body = with_router_markers(&ir);

        assert_eq!(count_cache_markers(&body), MAX_CACHE_MARKERS);
        assert_eq!(body, to_anthropic_request(&ir, &resolved()).unwrap());
    }

    #[test]
    fn the_router_fills_only_the_slots_the_agent_left_free() {
        let ephemeral = json!({ "type": "ephemeral" });
        let ir = anthropic_agent(json!({
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
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}" }
                ]}
            ]
        }));

        let body = with_router_markers(&ir);

        // One slot was free: it goes to the end of the conversation, the most valuable place.
        assert_eq!(count_cache_markers(&body), MAX_CACHE_MARKERS);
        assert_eq!(body["messages"][2]["content"][0][CACHE_CONTROL], ephemeral);
        assert_eq!(body["system"], "You are a coding agent.");
    }

    #[test]
    fn no_five_minute_system_marker_goes_ahead_of_an_agents_one_hour_marker() {
        // Anthropic rejects a shorter TTL ahead of a longer one, and `system` precedes every turn.
        let ir = anthropic_agent(json!({
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
        }));

        let body = with_router_markers(&ir);

        assert_eq!(body["system"], "You are a coding agent.");
        assert_eq!(
            body["messages"][2]["content"][0][CACHE_CONTROL],
            json!({ "type": "ephemeral" })
        );
    }

    #[test]
    fn without_the_flag_the_router_adds_no_marker() {
        let ir: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": "Fix the failing test." }
            ]
        }))
        .unwrap();
        let provider = AnthropicProvider::new(reqwest::Client::new(), "http://x".into());

        assert_eq!(
            provider.request_body(&ir, &resolved()).unwrap(),
            to_anthropic_request(&ir, &resolved()).unwrap()
        );
    }

    #[tokio::test]
    async fn the_default_gateway_config_sends_the_routers_markers() {
        let mut server = mockito::Server::new_async().await;
        let ephemeral = json!({ "type": "ephemeral" });
        let m = server
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({
                "system": [{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral }],
                "messages": [{ "role": "user", "content": [
                    { "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }
                ]}]
            })))
            .with_status(200)
            .with_body(
                json!({
                    "id": "msg_1", "type": "message", "role": "assistant",
                    "content": [{ "type": "text", "text": "On it." }],
                    "stop_reason": "end_turn",
                    "usage": { "input_tokens": 10, "output_tokens": 3 }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let cfg = crate::config::GatewayConfig {
            anthropic_api_base: server.url(),
            ..Default::default()
        };
        let provider =
            crate::providers::provider_for("anthropic", None, &reqwest::Client::new(), &cfg)
                .unwrap();
        let ir: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": "Fix the failing test." }
            ]
        }))
        .unwrap();

        provider.chat(&ir, &resolved()).await.unwrap();

        m.assert_async().await;
    }

    #[test]
    fn to_anthropic_request_with_image_url() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "What is this image?" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/test.png" } }
                ]
            }]
        }))
        .unwrap();

        let body = to_anthropic_request(&req, &resolved()).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        let blocks = msgs[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "What is this image?");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "url");
        assert_eq!(blocks[1]["source"]["url"], "https://example.com/test.png");
    }

    #[test]
    fn to_anthropic_request_with_image_data_url() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }
                ]
            }]
        })).unwrap();

        let body = to_anthropic_request(&req, &resolved()).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        let blocks = msgs[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["type"], "base64");
        assert_eq!(blocks[0]["source"]["media_type"], "image/png");
        assert_eq!(blocks[0]["source"]["data"], "iVBORw0KGgo=");
    }

    /// Emptiness is the destination's rule, not the front door's. The Anthropic inbound
    /// parser forwards an empty turn; this spoke is what refuses it, because the Messages
    /// API does. Routed to OpenAI instead, the same request would go through.
    #[test]
    fn an_empty_turn_is_refused_by_this_spoke_not_by_the_inbound_parser() {
        use crate::inbound::{AnthropicInbound, InboundParser};

        // The front door lets it past.
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 64,
                "messages": [{ "role": "user", "content": "" }]
            }))
            .unwrap();
        assert_eq!(req.messages[0].content, Some(json!("")));

        // The spoke is where it stops.
        match to_anthropic_request(&req, &resolved()).unwrap_err() {
            ProviderError::Rejected(message) => {
                assert!(message.contains("empty"), "{message}");
            }
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    #[test]
    fn video_is_refused_by_name_rather_than_sent_empty() {
        // The Messages API has no video input; a silent drop would have the model
        // answering about a video it never received.
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet",
            "messages": [{ "role": "user", "content": [
                { "type": "video_url", "video_url": { "url": "data:video/mp4;base64,AAAA" } }
            ]}]
        }))
        .unwrap();
        match to_anthropic_request(&req, &resolved()).unwrap_err() {
            ProviderError::Rejected(message) => {
                assert!(message.contains("video_url"), "{message}");
            }
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    #[test]
    fn to_anthropic_request_with_file_data_url() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "file", "file": { "file_data": "data:application/pdf;base64,JVBERi0xLjQK" } }
                ]
            }]
        })).unwrap();

        let body = to_anthropic_request(&req, &resolved()).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        let blocks = msgs[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "document");
        assert_eq!(blocks[0]["source"]["type"], "base64");
        assert_eq!(blocks[0]["source"]["media_type"], "application/pdf");
        assert_eq!(blocks[0]["source"]["data"], "JVBERi0xLjQK");
    }

    fn one_part(part: Value) -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "what does it say?" },
                part
            ]}]
        }))
        .unwrap()
    }

    fn file_part(media_type: &str, bytes: &[u8], filename: &str) -> Value {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        json!({ "type": "file", "file": {
            "file_data": format!("data:{media_type};base64,{data}"),
            "filename": filename
        }})
    }

    fn rejection(req: &ChatRequest) -> String {
        match to_anthropic_request(req, &resolved()) {
            Err(ProviderError::Rejected(message)) => message,
            other => panic!("expected a by-name refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_text_document_reaches_anthropic_as_a_text_source() {
        let req = one_part(file_part("text/csv", b"region,q2\nEMEA,913", "data.csv"));

        let body = to_anthropic_request(&req, &resolved()).unwrap();

        let block = &body["messages"][0]["content"][1];
        assert_eq!(block["type"], "document");
        assert_eq!(block["source"]["type"], "text");
        assert_eq!(block["source"]["media_type"], "text/plain");
        assert_eq!(block["source"]["data"], "region,q2\nEMEA,913");
        assert_eq!(block["title"], "data.csv");
    }

    #[test]
    fn a_document_anthropic_cannot_read_is_refused_by_name() {
        let doc = "application/msword";
        let message = rejection(&one_part(file_part(doc, b"\xd0\xcf\x11\xe0", "report.doc")));
        assert!(
            message.starts_with("Anthropic does not accept this input"),
            "{message}"
        );
        assert!(message.contains(doc), "{message}");
    }

    #[test]
    fn a_text_document_that_is_not_utf8_or_over_1_mib_is_refused_by_name() {
        let message = rejection(&one_part(file_part(
            "text/plain",
            &[0xff, 0xfe, 0x00],
            "x.txt",
        )));
        assert!(message.contains("UTF-8"), "{message}");

        let big = vec![b'a'; 1024 * 1024 + 1];
        let message = rejection(&one_part(file_part("text/plain", &big, "big.txt")));
        assert!(message.contains("1 MiB"), "{message}");
    }

    #[test]
    fn an_image_type_anthropic_cannot_read_is_refused_by_name() {
        let message = rejection(&one_part(json!({ "type": "image_url", "image_url": {
            "url": "data:image/heic;base64,AAAA"
        }})));
        assert!(message.contains("image/heic"), "{message}");
    }

    #[test]
    fn an_image_over_anthropics_10_mb_encoded_limit_is_refused_by_name() {
        let data = "A".repeat(10_000_004);
        let message = rejection(&one_part(json!({ "type": "image_url", "image_url": {
            "url": format!("data:image/png;base64,{data}")
        }})));
        assert!(message.contains("10 MB"), "{message}");
    }

    #[test]
    fn to_anthropic_request_with_unsupported_parts_rejected() {
        // file.file_id not supported
        let req_file_id: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "file", "file": { "file_id": "file-123" } }
                ]
            }]
        }))
        .unwrap();
        let err = to_anthropic_request(&req_file_id, &resolved()).unwrap_err();
        match err {
            ProviderError::Rejected(message) => {
                assert!(message.contains("file.file_id"));
            }
            _ => panic!("Expected ProviderError::Rejected"),
        }

        // input_audio not supported
        let req_audio: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "input_audio", "input_audio": { "data": "abc" } }
                ]
            }]
        }))
        .unwrap();
        let err = to_anthropic_request(&req_audio, &resolved()).unwrap_err();
        match err {
            ProviderError::Rejected(message) => {
                assert!(message.contains("input_audio"));
            }
            _ => panic!("Expected ProviderError::Rejected"),
        }

        // Empty message content
        let req_empty: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [{
                "role": "user",
                "content": []
            }]
        }))
        .unwrap();
        let err = to_anthropic_request(&req_empty, &resolved()).unwrap_err();
        match err {
            ProviderError::Rejected(message) => {
                assert!(message.contains("empty"));
            }
            _ => panic!("Expected ProviderError::Rejected"),
        }
    }

    #[test]
    fn to_anthropic_request_tool_result_preserves_images() {
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [
                {
                    "role": "tool",
                    "tool_call_id": "call_123",
                    "content": [
                        { "type": "text", "text": "Here is the chart:" },
                        { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" } }
                    ]
                }
            ]
        })).unwrap();

        let body = to_anthropic_request(&req, &resolved()).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        let tr = &msgs[0]["content"][0];
        assert_eq!(tr["type"], "tool_result");
        assert_eq!(tr["tool_use_id"], "call_123");
        let content_blocks = tr["content"].as_array().unwrap();
        assert_eq!(content_blocks.len(), 2);
        assert_eq!(content_blocks[0]["type"], "text");
        assert_eq!(content_blocks[1]["type"], "image");
    }
}
