//! Anthropic inbound parser — the agent speaks the Anthropic Messages API.
//!
//! This is the **inverse** of the outbound Anthropic spoke (`providers/anthropic.rs`):
//! - `parse_chat`: an Anthropic `/v1/messages` request body → canonical IR (OpenAI
//!   shape). System → a `system` message; `tool_use`/`tool_result` blocks → assistant
//!   `tool_calls[]` / `{role:"tool"}` messages; `input_schema` → `parameters`.
//! - `render_chat_response`: IR response → an Anthropic Messages response (text +
//!   `tool_use` content blocks, `finish_reason` → `stop_reason`).
//! - the streaming renderer: flat OpenAI-shaped IR chunks → Anthropic's **stateful** SSE
//!   event protocol (`message_start` → `content_block_*` → `message_delta`/`message_stop`).
//!
//! Building IR by constructing OpenAI-shaped JSON and deserializing keeps this in lock-
//! step with the IR's own serde rules.
//!
//! Text is flattened to strings, except where the agent set a `cache_control` marker: that
//! content stays as text parts, each keeping its marker, so the cached prefix the agent chose
//! ends where it put it (see [`crate::ir::CACHE_CONTROL`]).

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::{ChatStreamRenderer, InboundParser};
use crate::error::GatewayError;
use crate::ir::{
    CACHE_CONTROL, ChatChunk, ChatRequest, ChatResponse, ContentPart, DataSource,
    EmbeddingsRequest, EmbeddingsResponse, parts_to_content_value,
};

pub struct AnthropicInbound;

impl InboundParser for AnthropicInbound {
    fn parse_chat(&self, body: Value) -> Result<ChatRequest, GatewayError> {
        let mut oa_messages: Vec<Value> = Vec::new();

        // Anthropic carries the system prompt at the top level (string or block array).
        if let Some(system) = body.get("system") {
            let content = text_content_from_anthropic(system);
            if !is_blank(&content) {
                oa_messages.push(json!({ "role": "system", "content": content }));
            }
        }

        for m in body
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = m.get("content").unwrap_or(&Value::Null);
            match role {
                "assistant" => oa_messages.push(assistant_from_anthropic(content)),
                "user" => append_user_from_anthropic(content, &mut oa_messages)?,
                other => {
                    oa_messages.push(json!({ "role": other, "content": anthropic_text(content) }))
                }
            }
        }

        let mut oa = json!({ "messages": oa_messages });
        for key in ["model", "temperature", "max_tokens", "stream"] {
            if let Some(v) = body.get(key) {
                oa[key] = v.clone();
            }
        }
        if let Some(tools) = body.get("tools").and_then(Value::as_array) {
            oa["tools"] = json!(tools.iter().map(tool_from_anthropic).collect::<Vec<_>>());
        }
        if let Some(tc) = body.get("tool_choice").and_then(tool_choice_from_anthropic) {
            oa["tool_choice"] = tc;
        }

        serde_json::from_value(oa)
            .map_err(|e| GatewayError::BadRequest(format!("invalid anthropic request: {e}")))
    }

    fn render_chat_response(&self, resp: ChatResponse) -> Value {
        let id = anthropic_msg_id(&resp.id);
        let model = resp.model.clone();
        let usage = resp.usage.clone();
        let choice = resp.choices.into_iter().next();
        let finish = choice.as_ref().and_then(|c| c.finish_reason.clone());

        let mut blocks: Vec<Value> = Vec::new();
        if let Some(c) = &choice {
            if let Some(text) = c.message.text()
                && !text.is_empty()
            {
                blocks.push(json!({ "type": "text", "text": text }));
            }
            if let Some(tool_calls) = &c.message.tool_calls {
                for tc in tool_calls {
                    let input: Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.function.name,
                        "input": input,
                    }));
                }
            }
        }

        let stop_reason = finish
            .as_deref()
            .map(reverse_stop_reason)
            .unwrap_or("end_turn");
        let mut out = json!({
            "id": id,
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": blocks,
            "stop_reason": stop_reason,
            "stop_sequence": Value::Null,
        });
        if let Some(u) = usage {
            out["usage"] = anthropic_usage(u);
        }
        out
    }

    fn chat_stream_renderer(&self) -> Box<dyn ChatStreamRenderer> {
        Box::new(AnthropicStreamRenderer::default())
    }

    fn parse_embeddings(&self, _body: Value) -> Result<EmbeddingsRequest, GatewayError> {
        Err(GatewayError::BadRequest(
            "Anthropic inbound does not support embeddings".to_string(),
        ))
    }

    fn render_embeddings(&self, resp: EmbeddingsResponse) -> Value {
        // Unreachable: parse_embeddings always errors, so this never runs. Provide a
        // harmless fallback rather than panicking.
        serde_json::to_value(resp).unwrap_or(Value::Null)
    }
}

// ── Anthropic request → IR (OpenAI shape) ────────────────────────────────────

/// Best-effort plain text from an Anthropic content value: a string as-is, or the
/// concatenation of `text` fields across a content-block array.
fn anthropic_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

/// Anthropic text content → IR content: the joined text, unless a text block carries a cache
/// marker; then one text part per block, each keeping its marker.
fn text_content_from_anthropic(v: &Value) -> Value {
    let Value::Array(blocks) = v else {
        return json!(anthropic_text(v));
    };
    let text_blocks: Vec<&Value> = blocks.iter().filter(|b| is_text_block(b)).collect();
    if !text_blocks.iter().any(|b| b.get(CACHE_CONTROL).is_some()) {
        return json!(anthropic_text(v));
    }
    let parts: Vec<Value> = text_blocks
        .into_iter()
        .map(|b| with_cache_marker_of(b, json!({ "type": "text", "text": b["text"] })))
        .collect();
    json!(parts)
}

/// The blocks [`anthropic_text`] reads.
fn is_text_block(b: &Value) -> bool {
    b.get("text").and_then(Value::as_str).is_some()
}

fn is_blank(content: &Value) -> bool {
    match content {
        Value::String(s) => s.is_empty(),
        Value::Array(parts) => parts.is_empty(),
        _ => true,
    }
}

/// `target` with `source`'s cache marker copied onto it, if it has one.
fn with_cache_marker_of(source: &Value, mut target: Value) -> Value {
    if let Some(marker) = source.get(CACHE_CONTROL) {
        target[CACHE_CONTROL] = marker.clone();
    }
    target
}

/// A `tool_result` keeps one marker, on its whole result: the block's own, else the last one on
/// a block inside it. Both end the cached prefix at the end of the result.
fn tool_result_marker(b: &Value) -> Option<&Value> {
    b.get(CACHE_CONTROL).or_else(|| {
        b.get("content")
            .and_then(Value::as_array)?
            .iter()
            .rev()
            .find_map(|inner| inner.get(CACHE_CONTROL))
    })
}

/// Tool-result content with `marker` on its last part. A string becomes the one text part it
/// stands for, since only a part can hold a marker.
fn marked_at_end(content: Value, marker: &Value) -> Value {
    let mut parts = match content {
        Value::Array(parts) => parts,
        text => vec![json!({ "type": "text", "text": text })],
    };
    if let Some(last) = parts.last_mut() {
        last[CACHE_CONTROL] = marker.clone();
    }
    Value::Array(parts)
}

/// A user turn's IR content: unmarked text-only parts join into one string, so an unmarked
/// request reaches every provider byte-identical to before markers and media were kept; a
/// marker or a media part keeps the array, so each keeps its position.
fn user_content(parts: Vec<Value>) -> Value {
    let plain_text = parts
        .iter()
        .all(|p| p["type"] == "text" && p.get(CACHE_CONTROL).is_none());
    if plain_text {
        return json!(anthropic_text(&Value::Array(parts)));
    }
    Value::Array(parts)
}

/// An Anthropic assistant turn → an OpenAI assistant message. `text` blocks join into
/// `content`; `tool_use` blocks become `tool_calls[]` (arguments as a JSON string).
fn assistant_from_anthropic(content: &Value) -> Value {
    match content {
        Value::String(s) => json!({ "role": "assistant", "content": s }),
        Value::Array(blocks) => {
            let mut tool_calls: Vec<Value> = Vec::new();
            for b in blocks {
                if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let arguments = b
                        .get("input")
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "{}".to_string());
                    let call = json!({
                        "id": b.get("id").and_then(Value::as_str).unwrap_or_default(),
                        "type": "function",
                        "function": {
                            "name": b.get("name").and_then(Value::as_str).unwrap_or_default(),
                            "arguments": arguments,
                        }
                    });
                    tool_calls.push(with_cache_marker_of(b, call));
                }
            }
            let text = text_content_from_anthropic(content);
            let mut msg = json!({ "role": "assistant" });
            if tool_calls.is_empty() {
                msg["content"] = text;
            } else {
                msg["tool_calls"] = json!(tool_calls);
                // OpenAI: content is null on a pure tool-call turn.
                msg["content"] = if is_blank(&text) { Value::Null } else { text };
            }
            msg
        }
        _ => json!({ "role": "assistant", "content": "" }),
    }
}

/// Anthropic `source` object → canonical [`DataSource`].
///
/// `kind` ("image"/"document") only shapes the error text; the two block types carry an
/// identical source grammar, which is why they share this.
fn anthropic_source(
    b: &Value,
    kind: &str,
    default_media_type: &str,
) -> Result<DataSource, GatewayError> {
    let source = b
        .get("source")
        .ok_or_else(|| GatewayError::BadRequest(format!("{kind} block missing source")))?;
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => {
            let media_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or(default_media_type);
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest(format!("base64 {kind} missing data")))?;
            Ok(DataSource::from_inline(media_type, data))
        }
        "url" => {
            let url = source
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest(format!("url {kind} missing url")))?;
            Ok(DataSource::Url(url.to_string()))
        }
        other => Err(GatewayError::BadRequest(format!(
            "unsupported {kind} source type: '{other}'"
        ))),
    }
}

/// Anthropic content block → canonical part.
///
/// `Ok(None)` means "contributes nothing to this turn" — an empty text block, or a
/// `tool_result`, which the caller lifts into its own `{role:"tool"}` message instead.
fn anthropic_block_to_parts(b: &Value) -> Result<Vec<ContentPart>, GatewayError> {
    let block_type = b.get("type").and_then(Value::as_str).unwrap_or_default();
    match block_type {
        "text" => {
            let text = b.get("text").and_then(Value::as_str).unwrap_or_default();
            Ok(text_part(text).into_iter().collect())
        }
        "image" => Ok(vec![ContentPart::Image {
            source: anthropic_source(b, "image", "image/jpeg")?,
            detail: None,
        }]),
        "document" => document_parts(b),
        "search_result" => search_result_parts(b),
        "tool_result" => Ok(Vec::new()),
        "container_upload" => Err(GatewayError::BadRequest(
            "'container_upload' blocks hand a file to Anthropic's code execution sandbox, which \
             the router does not run; send the file as a document or image block instead"
                .to_string(),
        )),
        other => Err(GatewayError::BadRequest(format!(
            "unsupported content block type: '{other}' (the router reads text, image, document, \
             search_result and tool_result blocks)"
        ))),
    }
}

fn search_result_parts(b: &Value) -> Result<Vec<ContentPart>, GatewayError> {
    let field = |name: &str| {
        b.get(name).ok_or_else(|| {
            GatewayError::BadRequest(format!("search_result block missing '{name}'"))
        })
    };
    let source = field("source")?.as_str().unwrap_or_default();
    let title = field("title")?.as_str().unwrap_or_default();
    let Value::Array(blocks) = field("content")? else {
        return Err(GatewayError::BadRequest(
            "search_result block missing 'content'".to_string(),
        ));
    };
    let mut texts = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text" => texts.push(
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            other => {
                return Err(GatewayError::BadRequest(format!(
                    "a search_result can hold only text blocks, not '{other}'"
                )));
            }
        }
    }
    Ok(vec![ContentPart::Text(format!(
        "\n\nSearch result: {title}\nSource: {source}\n\n{}\n\n",
        texts.join("\n\n")
    ))])
}

fn text_part(text: &str) -> Option<ContentPart> {
    (!text.is_empty()).then(|| ContentPart::Text(text.to_string()))
}

fn document_parts(b: &Value) -> Result<Vec<ContentPart>, GatewayError> {
    // `title` is Anthropic's only name for a document, and OpenAI requires one
    // alongside `file_data`; absent, the renderer synthesizes it from the media
    // type (`ir::parts::openai_filename`).
    let filename = b.get("title").and_then(Value::as_str).map(str::to_string);
    let source = b.get("source");
    match source.and_then(|s| s.get("type")).and_then(Value::as_str) {
        Some("text") => {
            let text = source
                .and_then(|s| s.get("data"))
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest("text document missing data".into()))?;
            Ok(vec![ContentPart::File {
                source: DataSource::from_inline("text/plain", &STANDARD.encode(text)),
                filename,
            }])
        }
        Some("content") => content_document_parts(source.and_then(|s| s.get("content"))),
        _ => Ok(vec![ContentPart::File {
            source: anthropic_source(b, "document", "application/pdf")?,
            filename,
        }]),
    }
}

fn content_document_parts(content: Option<&Value>) -> Result<Vec<ContentPart>, GatewayError> {
    match content {
        Some(Value::String(text)) => Ok(text_part(text).into_iter().collect()),
        Some(Value::Array(blocks)) => {
            let mut parts = Vec::new();
            for block in blocks {
                match block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "text" | "image" => parts.extend(anthropic_block_to_parts(block)?),
                    other => {
                        return Err(GatewayError::BadRequest(format!(
                            "content document cannot hold a '{other}' block"
                        )));
                    }
                }
            }
            Ok(parts)
        }
        _ => Err(GatewayError::BadRequest(
            "content document missing content".into(),
        )),
    }
}

/// An Anthropic user turn → one or more OpenAI messages. `tool_result` blocks become
/// `{role:"tool"}` messages (keyed by `tool_use_id`); text/multimodal parts become a user message.
///
/// Empty content is **forwarded, not rejected**. Emptiness is not a translation failure —
/// there is nothing to convert and nothing to lose — so it is not this edge's call to make.
/// Each provider spoke already enforces its own rule (Anthropic and Gemini both refuse an
/// empty user turn; OpenAI accepts one), which keeps the limit with the backend that has
/// it. Unconvertible *content* is still rejected here, because that is a real translation
/// failure and forwarding it would drop data silently.
fn append_user_from_anthropic(content: &Value, out: &mut Vec<Value>) -> Result<(), GatewayError> {
    match content {
        Value::String(s) => {
            // Empty or not, the turn is forwarded. Whether an empty message is legal is
            // the destination's rule, not ours — see the note on this function.
            out.push(json!({ "role": "user", "content": s }));
            Ok(())
        }
        Value::Array(blocks) => {
            let mut user_parts: Vec<Value> = Vec::new();
            let mut had_tool_result = false;
            for b in blocks {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    had_tool_result = true;
                    let id = b
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let tr_content = match b.get("content") {
                        Some(Value::String(s)) => Value::String(s.clone()),
                        Some(Value::Array(sub_blocks)) => {
                            let mut sub_parts = Vec::new();
                            for sb in sub_blocks {
                                sub_parts.extend(anthropic_block_to_parts(sb)?);
                            }
                            parts_to_content_value(&sub_parts)
                        }
                        _ => Value::String(String::new()),
                    };
                    let content = match tool_result_marker(b) {
                        Some(marker) => marked_at_end(tr_content, marker),
                        None => tr_content,
                    };
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": content,
                    }));
                } else {
                    let parts = anthropic_block_to_parts(b)?;
                    let last = parts.len().saturating_sub(1);
                    for (i, part) in parts.into_iter().enumerate() {
                        let rendered = part.to_openai_part();
                        user_parts.push(if i == last {
                            with_cache_marker_of(b, rendered)
                        } else {
                            rendered
                        });
                    }
                }
            }
            if !user_parts.is_empty() {
                out.push(json!({ "role": "user", "content": user_content(user_parts) }));
            } else if !had_tool_result {
                // Blocks that carried nothing, and no tool result to stand in for the
                // turn. Keep the turn rather than dropping it: losing a message silently
                // breaks user/assistant alternation downstream.
                out.push(json!({ "role": "user", "content": "" }));
            }
            Ok(())
        }
        // `null`, or a shape we do not model.
        _ => {
            out.push(json!({ "role": "user", "content": "" }));
            Ok(())
        }
    }
}

/// Anthropic tool def → OpenAI function-tool def (`input_schema` → `parameters`).
fn tool_from_anthropic(t: &Value) -> Value {
    let mut function = json!({ "name": t.get("name").and_then(Value::as_str).unwrap_or_default() });
    if let Some(d) = t.get("description") {
        function["description"] = d.clone();
    }
    if let Some(s) = t.get("input_schema") {
        function["parameters"] = s.clone();
    }
    with_cache_marker_of(t, json!({ "type": "function", "function": function }))
}

/// Anthropic `tool_choice` → OpenAI `tool_choice` (inverse of the outbound mapping).
fn tool_choice_from_anthropic(v: &Value) -> Option<Value> {
    match v.get("type").and_then(Value::as_str) {
        Some("auto") => Some(json!("auto")),
        Some("any") => Some(json!("required")),
        Some("none") => Some(json!("none")),
        Some("tool") => v
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({ "type": "function", "function": { "name": name } })),
        _ => None,
    }
}

// ── shared helpers ───────────────────────────────────────────────────────────

/// OpenAI `finish_reason` → Anthropic `stop_reason` (inverse of `map_stop_reason`).
fn reverse_stop_reason(finish_reason: &str) -> &'static str {
    match finish_reason {
        "tool_calls" => "tool_use",
        "length" => "max_tokens",
        _ => "end_turn",
    }
}

/// Present our completion id as an Anthropic-style `msg_…` id.
fn anthropic_msg_id(id: &str) -> String {
    let id = id.trim_start_matches("chatcmpl-");
    if id.starts_with("msg_") {
        id.to_string()
    } else {
        format!("msg_{id}")
    }
}

/// Frame one Anthropic SSE event: an `event:` line plus a `data:` line whose payload
/// also carries `type` (Anthropic includes it in both places).
fn event(kind: &str, mut data: Value) -> String {
    if let Value::Object(map) = &mut data {
        map.insert("type".to_string(), json!(kind));
    }
    format!("event: {kind}\ndata: {data}\n\n")
}

// ── IR stream → Anthropic SSE (stateful) ─────────────────────────────────────

enum OpenBlock {
    Text(i64),
    Tool(i64),
}

fn anthropic_usage(mut usage: crate::ir::Usage) -> Value {
    usage.normalize_openai_details();
    let mut value = json!({});
    for (key, count) in [
        ("input_tokens", usage.prompt_tokens),
        ("output_tokens", usage.completion_tokens),
        ("cache_read_input_tokens", usage.cache_read_input_tokens),
        (
            "cache_creation_input_tokens",
            usage.cache_creation_input_tokens,
        ),
    ] {
        if let Some(count) = count {
            value[key] = json!(count);
        }
    }
    if let Some(cache) = usage.cache_creation {
        value["cache_creation"] = json!(cache);
    }
    value
}

/// Reconstructs Anthropic's stateful event sequence from flat OpenAI-shaped IR chunks.
///
/// Anthropic indexes content blocks sequentially across both text and tool blocks; the
/// IR carries tool deltas with their own (tool-only) index. We assign Anthropic block
/// indices as blocks open and map the IR tool index onto them. Usage arriving after
/// message_start is sent in message_delta so clients can update their final message
/// without delaying incremental text or tool delivery.
#[derive(Default)]
struct AnthropicStreamRenderer {
    started: bool,
    id: String,
    model: String,
    usage: crate::ir::Usage,
    stop_reason: Option<String>,
    open_block: Option<OpenBlock>,
    next_index: i64,
    tool_index_map: HashMap<i64, i64>,
}

impl AnthropicStreamRenderer {
    fn message_start_event(&self) -> String {
        event(
            "message_start",
            json!({
                "message": {
                    "id": self.id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    // Required start counts are provisional; terminal usage replaces them.
                    "usage": anthropic_usage(crate::ir::Usage {
                        prompt_tokens: Some(self.usage.prompt_tokens.unwrap_or(0)),
                        completion_tokens: Some(0),
                        ..self.usage.clone()
                    }),
                }
            }),
        )
    }

    fn ensure_text_block(&mut self, out: &mut Vec<String>) {
        if matches!(self.open_block, Some(OpenBlock::Text(_))) {
            return;
        }
        self.close_block(out);
        let index = self.next_index;
        self.next_index += 1;
        self.open_block = Some(OpenBlock::Text(index));
        out.push(event(
            "content_block_start",
            json!({ "index": index, "content_block": { "type": "text", "text": "" } }),
        ));
    }

    fn close_block(&mut self, out: &mut Vec<String>) {
        if let Some(block) = self.open_block.take() {
            let index = match block {
                OpenBlock::Text(i) | OpenBlock::Tool(i) => i,
            };
            out.push(event("content_block_stop", json!({ "index": index })));
        }
    }
}

impl ChatStreamRenderer for AnthropicStreamRenderer {
    fn render(&mut self, chunk: ChatChunk) -> Vec<String> {
        let mut out = Vec::new();

        if let Some(mut usage) = chunk.usage {
            usage.normalize_openai_details();
            self.usage.prompt_tokens = usage.prompt_tokens.or(self.usage.prompt_tokens);
            self.usage.completion_tokens = usage.completion_tokens.or(self.usage.completion_tokens);
            self.usage.cache_read_input_tokens = usage
                .cache_read_input_tokens
                .or(self.usage.cache_read_input_tokens);
            self.usage.cache_creation_input_tokens = usage
                .cache_creation_input_tokens
                .or(self.usage.cache_creation_input_tokens);
            if let Some(cache) = usage.cache_creation {
                let current = self.usage.cache_creation.get_or_insert_default();
                current.ephemeral_5m_input_tokens = cache
                    .ephemeral_5m_input_tokens
                    .or(current.ephemeral_5m_input_tokens);
                current.ephemeral_1h_input_tokens = cache
                    .ephemeral_1h_input_tokens
                    .or(current.ephemeral_1h_input_tokens);
            }
        }

        if !self.started {
            self.started = true;
            self.id = anthropic_msg_id(&chunk.id);
            self.model = chunk.model.clone();
            out.push(self.message_start_event());
        }

        if let Some(choice) = chunk.choices.first() {
            let delta = &choice.delta;

            if let Some(text) = &delta.content
                && !text.is_empty()
            {
                self.ensure_text_block(&mut out);
                if let Some(OpenBlock::Text(index)) = self.open_block {
                    out.push(event(
                        "content_block_delta",
                        json!({ "index": index, "delta": { "type": "text_delta", "text": text } }),
                    ));
                }
            }

            if let Some(tool_calls) = &delta.tool_calls {
                for tc in tool_calls {
                    let name = tc.function.as_ref().and_then(|f| f.name.clone());
                    let args = tc.function.as_ref().and_then(|f| f.arguments.clone());
                    let is_start = tc.id.is_some() || name.is_some();

                    if is_start {
                        self.close_block(&mut out);
                        let index = self.next_index;
                        self.next_index += 1;
                        self.tool_index_map.insert(tc.index, index);
                        self.open_block = Some(OpenBlock::Tool(index));
                        out.push(event(
                            "content_block_start",
                            json!({
                                "index": index,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": tc.id.clone().unwrap_or_default(),
                                    "name": name.unwrap_or_default(),
                                    "input": {},
                                }
                            }),
                        ));
                    }

                    // Argument fragments (empty on the opening delta) → input_json_delta.
                    if let Some(args) = args
                        && !args.is_empty()
                        && let Some(&index) = self.tool_index_map.get(&tc.index)
                    {
                        out.push(event(
                            "content_block_delta",
                            json!({ "index": index, "delta": { "type": "input_json_delta", "partial_json": args } }),
                        ));
                    }
                }
            }

            if let Some(fr) = &choice.finish_reason {
                self.stop_reason = Some(reverse_stop_reason(fr).to_string());
            }
        }

        out
    }

    fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.started {
            self.started = true;
            out.push(self.message_start_event());
        }
        self.close_block(&mut out);
        let stop_reason = self
            .stop_reason
            .clone()
            .unwrap_or_else(|| "end_turn".to_string());
        out.push(event(
            "message_delta",
            json!({
                "delta": { "stop_reason": stop_reason, "stop_sequence": Value::Null },
                "usage": anthropic_usage(self.usage.clone()),
            }),
        ));
        out.push(event("message_stop", json!({})));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ChunkChoice, Delta, FunctionCallDelta, ToolCallDelta};
    use serde_json::Map;

    // ── parse_chat (Anthropic request → IR) ──────────────────────────────────

    #[test]
    fn parse_extracts_system_and_translates_tools() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "temperature": 0.1,
                "system": "You are a Translation agent.",
                "messages": [{ "role": "user", "content": "translate to hindi: my name is csg" }],
                "tools": [{
                    "name": "translate_text",
                    "description": "Translate plain text",
                    "input_schema": { "type": "object", "properties": { "text": { "type": "string" } } }
                }],
                "tool_choice": { "type": "auto" }
            }))
            .unwrap();

        // system became the first message; user follows.
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(
            req.messages[0].text().as_deref(),
            Some("You are a Translation agent.")
        );
        assert_eq!(req.messages[1].role, "user");
        assert_eq!(req.max_tokens, Some(1024));
        assert_eq!(req.temperature, Some(0.1));
        // input_schema → parameters; no Anthropic-only fields leak.
        let tool = &req.tools.as_ref().unwrap()[0];
        assert_eq!(tool.function.name, "translate_text");
        assert_eq!(
            tool.function.parameters.as_ref().unwrap()["properties"]["text"]["type"],
            "string"
        );
        assert_eq!(req.tool_choice, Some(json!("auto")));
    }

    #[test]
    fn parse_multi_turn_tool_use_and_tool_result() {
        // Inverse of the outbound C3 case: tool_use → assistant tool_calls;
        // tool_result → a {role:"tool"} message keyed by tool_use_id.
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [
                    { "role": "user", "content": "translate to hindi: my name is csg" },
                    { "role": "assistant", "content": [
                        { "type": "tool_use", "id": "toolu_01", "name": "translate_text",
                          "input": { "text": "my name is csg", "target_language": "hi" } }
                    ]},
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_01", "content": "मेरा नाम csg है" }
                    ]}
                ]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 3);
        let assistant = &req.messages[1];
        assert_eq!(assistant.role, "assistant");
        assert!(assistant.text().is_none()); // pure tool call → no text content
        let tc = &assistant.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.id, "toolu_01");
        assert_eq!(tc.function.name, "translate_text");
        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["target_language"], "hi");
        let tool_msg = &req.messages[2];
        assert_eq!(tool_msg.role, "tool");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("toolu_01"));
        assert_eq!(tool_msg.text().as_deref(), Some("मेरा नाम csg है"));
    }

    #[test]
    fn parse_round_trips_through_outbound_anthropic() {
        // The inbound parse should reproduce an OpenAI IR that the outbound spoke turns
        // back into an equivalent Anthropic request (system, tool, tool_choice).
        let anthropic_req = json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 4000,
            "system": "You are helpful.",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{ "name": "f", "description": "d", "input_schema": { "type": "object" } }],
            "tool_choice": { "type": "any" }
        });
        let req = AnthropicInbound.parse_chat(anthropic_req).unwrap();
        // any → required in OpenAI shape
        assert_eq!(req.tool_choice, Some(json!("required")));
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.tools.as_ref().unwrap()[0].function.name, "f");
    }

    #[test]
    fn parse_keeps_tool_choice_none() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [{ "role": "user", "content": "hi" }],
                "tools": [{ "name": "f", "input_schema": { "type": "object" } }],
                "tool_choice": { "type": "none" }
            }))
            .unwrap();
        assert_eq!(req.tool_choice, Some(json!("none")));
    }

    #[test]
    fn parse_keeps_cache_markers_on_content_parts_where_openrouter_reads_them() {
        let ephemeral = json!({ "type": "ephemeral" });
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "system": [
                    { "type": "text", "text": "You are a coding agent." },
                    { "type": "text", "text": "Repository map.", "cache_control": ephemeral }
                ],
                "tools": [{ "name": "read", "input_schema": { "type": "object" }, "cache_control": ephemeral }],
                "messages": [
                    { "role": "user", "content": "Fix the test." },
                    { "role": "assistant", "content": [
                        { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {}, "cache_control": ephemeral }
                    ]},
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}", "cache_control": ephemeral }
                    ]}
                ]
            }))
            .unwrap();

        let ir = serde_json::to_value(&req).unwrap();
        assert_eq!(
            ir["messages"][0]["content"],
            json!([
                { "type": "text", "text": "You are a coding agent." },
                { "type": "text", "text": "Repository map.", "cache_control": ephemeral }
            ])
        );
        assert_eq!(ir["tools"][0]["cache_control"], ephemeral);
        assert_eq!(
            ir["messages"][2]["tool_calls"][0]["cache_control"],
            ephemeral
        );
        assert_eq!(
            ir["messages"][3]["content"],
            json!([{ "type": "text", "text": "fn main() {}", "cache_control": ephemeral }])
        );
    }

    #[test]
    fn parse_without_cache_markers_still_flattens_content_to_strings() {
        // Unmarked requests must reach every provider byte-identical to before markers were kept.
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "system": [
                    { "type": "text", "text": "You are a coding agent. " },
                    { "type": "text", "text": "Repository map." }
                ],
                "messages": [{ "role": "user", "content": [
                    { "type": "text", "text": "Fix the test. " },
                    { "type": "text", "text": "Then stop." }
                ]}]
            }))
            .unwrap();

        let ir = serde_json::to_value(&req).unwrap();
        assert_eq!(
            ir["messages"][0]["content"],
            "You are a coding agent. Repository map."
        );
        assert_eq!(ir["messages"][1]["content"], "Fix the test. Then stop.");
    }

    // ── render_chat_response (IR → Anthropic response) ────────────────────────

    #[test]
    fn render_text_response() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": "claude-3-5-sonnet-20241022",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Here is the translation." }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 531, "completion_tokens": 42, "total_tokens": 573 }
        }))
        .unwrap();
        let v = AnthropicInbound.render_chat_response(resp);
        assert_eq!(v["type"], "message");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["id"], "msg_1");
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "Here is the translation.");
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["usage"]["input_tokens"], 531);
        assert_eq!(v["usage"]["output_tokens"], 42);
    }

    #[test]
    fn render_tool_use_response() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl-2", "object": "chat.completion", "model": "claude-3-5-sonnet-20241022",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": null, "tool_calls": [{
                "id": "toolu_9", "type": "function",
                "function": { "name": "translate_text", "arguments": "{\"text\":\"hi\",\"target_language\":\"hi\"}" }
            }]}, "finish_reason": "tool_calls" }]
        }))
        .unwrap();
        let v = AnthropicInbound.render_chat_response(resp);
        assert_eq!(v["stop_reason"], "tool_use");
        let block = &v["content"][0];
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["id"], "toolu_9");
        assert_eq!(block["name"], "translate_text");
        // arguments JSON string → input object
        assert_eq!(block["input"]["target_language"], "hi");
    }

    // ── streaming renderer (IR chunks → Anthropic SSE) ────────────────────────

    fn text_chunk(text: &str) -> ChatChunk {
        ChatChunk {
            id: "chatcmpl-s".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "claude-3-5-sonnet-20241022".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    content: Some(text.into()),
                    ..Delta::default()
                },
                finish_reason: None,
            }],
            usage: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn terminal_usage_preserves_input_cache_and_ttl_without_buffering_text() {
        let mut renderer = AnthropicInbound.chat_stream_renderer();
        assert!(
            renderer
                .render(text_chunk("hello"))
                .join("")
                .contains("text_delta")
        );
        let usage: crate::ir::Usage = serde_json::from_value(json!({
            "prompt_tokens":2,"completion_tokens":10,"cache_read_input_tokens":1000,
            "cache_creation_input_tokens":300,
            "cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":200}
        }))
        .unwrap();
        let mut chunk = text_chunk("");
        chunk.usage = Some(usage.clone());
        renderer.render(chunk.clone());
        renderer.render(chunk);
        let mut partial = text_chunk("");
        partial.usage = Some(crate::ir::Usage {
            completion_tokens: Some(0),
            ..Default::default()
        });
        renderer.render(partial);
        let frames = renderer.finish();
        let delta: Value = serde_json::from_str(
            frames
                .iter()
                .find(|f| f.contains("event: message_delta"))
                .unwrap()
                .lines()
                .find_map(|l| l.strip_prefix("data: "))
                .unwrap(),
        )
        .unwrap();
        let mut expected = anthropic_usage(usage);
        expected["output_tokens"] = json!(0);
        assert_eq!(delta["usage"], expected);
        assert!(
            !delta["usage"]
                .as_object()
                .unwrap()
                .contains_key("total_tokens")
        );
    }

    #[test]
    fn buffered_usage_normalizes_nested_cache_and_retains_missing_fields() {
        let usage = serde_json::from_value(json!({"prompt_tokens":110,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":100}})).unwrap();
        let result = anthropic_usage(usage);
        assert_eq!(result["input_tokens"], 10);
        assert_eq!(result["cache_read_input_tokens"], 100);
        assert!(result.get("cache_creation_input_tokens").is_none());
        assert_eq!(anthropic_usage(crate::ir::Usage::default()), json!({}));
    }

    #[test]
    fn stream_text_emits_message_start_block_and_stop() {
        let mut r = AnthropicInbound.chat_stream_renderer();
        let start: Vec<String> = r.render(text_chunk("Hel"));
        let joined = start.join("");
        assert!(joined.contains("event: message_start"));
        assert!(joined.contains("event: content_block_start"));
        assert!(joined.contains("\"type\":\"text\""));
        assert!(joined.contains("event: content_block_delta"));
        assert!(joined.contains("\"text_delta\""));
        assert!(joined.contains("Hel"));

        // Second text delta must NOT reopen the block.
        let more = r.render(text_chunk("lo")).join("");
        assert!(!more.contains("content_block_start"));
        assert!(more.contains("lo"));

        let fin = r.finish().join("");
        assert!(fin.contains("event: content_block_stop"));
        assert!(fin.contains("event: message_delta"));
        assert!(fin.contains("\"end_turn\""));
        assert!(fin.contains("event: message_stop"));
    }

    #[test]
    fn stream_tool_call_reassembles_into_anthropic_events() {
        let mut r = AnthropicInbound.chat_stream_renderer();

        // Opening delta: id + name + empty args.
        let open = ChatChunk {
            id: "chatcmpl-t".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "claude-3-5-sonnet-20241022".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: Some(vec![ToolCallDelta {
                        index: 0,
                        id: Some("toolu_1".into()),
                        kind: Some("function".into()),
                        function: Some(FunctionCallDelta {
                            name: Some("translate_text".into()),
                            arguments: Some(String::new()),
                        }),
                        ..ToolCallDelta::default()
                    }]),
                    ..Delta::default()
                },
                finish_reason: None,
            }],
            usage: None,
            extra: Map::new(),
        };
        let frag = |partial: &str| ChatChunk {
            id: "chatcmpl-t".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "claude-3-5-sonnet-20241022".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: Some(vec![ToolCallDelta {
                        index: 0,
                        id: None,
                        kind: None,
                        function: Some(FunctionCallDelta {
                            name: None,
                            arguments: Some(partial.into()),
                        }),
                        ..ToolCallDelta::default()
                    }]),
                    ..Delta::default()
                },
                finish_reason: None,
            }],
            usage: None,
            extra: Map::new(),
        };

        let started = r.render(open).join("");
        assert!(started.contains("event: content_block_start"));
        assert!(started.contains("\"tool_use\""));
        assert!(started.contains("toolu_1"));
        assert!(started.contains("translate_text"));

        let a = r.render(frag("{\"text\":")).join("");
        let b = r.render(frag("\"hi\"}")).join("");
        assert!(a.contains("input_json_delta"));
        assert!(a.contains("partial_json"));
        assert!(b.contains("\\\"hi\\\"}"));

        // finish chunk sets the stop reason.
        let mut finish_chunk = text_chunk("");
        finish_chunk.choices[0].delta = Delta::default();
        finish_chunk.choices[0].finish_reason = Some("tool_calls".into());
        let _ = r.render(finish_chunk);

        let fin = r.finish().join("");
        assert!(fin.contains("event: content_block_stop"));
        assert!(fin.contains("event: message_delta"));
        assert!(fin.contains("\"stop_reason\":\"tool_use\""));
        assert!(fin.contains("event: message_stop"));
    }

    #[test]
    fn parse_chat_with_image_url_source() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "What is this?" },
                        { "type": "image", "source": { "type": "url", "url": "https://example.com/cat.png" } }
                    ]
                }]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 1);
        let msg = &req.messages[0];
        let parts = msg.content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "What is this?");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "https://example.com/cat.png");
    }

    #[test]
    fn a_reply_id_carries_the_msg_prefix_exactly_once() {
        for (id, expected) in [
            ("chatcmpl-msg_011CghUGgzcD", "msg_011CghUGgzcD"),
            ("msg_011CghUGgzcD", "msg_011CghUGgzcD"),
            ("chatcmpl-abc123", "msg_abc123"),
            ("abc123", "msg_abc123"),
        ] {
            assert_eq!(anthropic_msg_id(id), expected, "{id}");
        }
    }

    #[test]
    fn a_base64_source_media_type_is_normalized() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "image", "source": { "type": "base64", "media_type": "Image/PNG", "data": "iVBORw0KGgo=" } },
                        { "type": "document", "source": { "type": "base64", "media_type": "application/pdf;name=q3.pdf", "data": "JVBERi0xLjQK" } }
                    ]
                }]
            }))
            .unwrap();

        let parts = req.messages[0]
            .content
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(
            parts[0]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
        assert_eq!(
            parts[1]["file"]["file_data"],
            "data:application/pdf;base64,JVBERi0xLjQK"
        );
    }

    #[test]
    fn parse_chat_with_image_base64_source() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } }
                    ]
                }]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 1);
        let msg = &req.messages[0];
        let parts = msg.content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "image_url");
        assert_eq!(
            parts[0]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
    }

    #[test]
    fn parse_chat_with_document_base64_source() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQK" } }
                    ]
                }]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 1);
        let msg = &req.messages[0];
        let parts = msg.content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "file");
        assert_eq!(
            parts[0]["file"]["file_data"],
            "data:application/pdf;base64,JVBERi0xLjQK"
        );
        // An untitled document still needs a name: OpenAI refuses a `file` object that
        // carries neither `filename` nor `file_id`.
        assert_eq!(parts[0]["file"]["filename"], "document.pdf");
    }

    #[test]
    fn a_documents_title_becomes_the_filename() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [{
                    "role": "user",
                    "content": [
                        {
                            "type": "document",
                            "title": "q3-earnings.pdf",
                            "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQK" }
                        }
                    ]
                }]
            }))
            .unwrap();

        let parts = req.messages[0]
            .content
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(parts[0]["file"]["filename"], "q3-earnings.pdf");
    }

    #[test]
    fn parse_chat_with_tool_result_carrying_image() {
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "max_tokens": 1024,
                "messages": [{
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_chart_01",
                            "content": [
                                { "type": "text", "text": "Generated chart:" },
                                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } }
                            ]
                        }
                    ]
                }]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 1);
        let msg = &req.messages[0];
        assert_eq!(msg.role, "tool");
        assert_eq!(msg.tool_call_id.as_deref(), Some("toolu_chart_01"));
        let parts = msg.content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
    }

    #[test]
    fn parse_chat_empty_or_unsupported_part_rejected() {
        // An empty user message is NOT a translation failure — nothing to convert and
        // nothing to lose — so it is forwarded and the destination spoke decides. The
        // Anthropic and Gemini spokes still refuse it; OpenAI accepts it.
        let req = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "messages": [{ "role": "user", "content": "" }]
            }))
            .unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].content, Some(json!("")));

        // A turn whose blocks carry nothing keeps its place rather than vanishing —
        // dropping it would break user/assistant alternation downstream.
        let empty_blocks = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "messages": [{ "role": "user", "content": [] }]
            }))
            .unwrap();
        assert_eq!(empty_blocks.messages.len(), 1);
        assert_eq!(empty_blocks.messages[0].content, Some(json!("")));

        // Unconvertible *content*, by contrast, is still a real translation failure.
        // unsupported block type
        let err_block = AnthropicInbound
            .parse_chat(json!({
                "model": "claude-3-5-sonnet-20241022",
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "unknown_future_block", "foo": "bar" }]
                }]
            }))
            .unwrap_err();
        assert!(matches!(err_block, GatewayError::BadRequest(_)));
    }

    fn user_turn(blocks: Value) -> Result<ChatRequest, GatewayError> {
        AnthropicInbound.parse_chat(json!({
            "model": "claude-haiku-5-5",
            "max_tokens": 64,
            "messages": [{ "role": "user", "content": blocks }]
        }))
    }

    fn parts_of(req: &ChatRequest, index: usize) -> Vec<Value> {
        req.messages[index]
            .content
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    fn decoded_file_text(part: &Value) -> String {
        use base64::Engine;
        let data_url = part["file"]["file_data"].as_str().unwrap();
        let encoded = data_url.strip_prefix("data:text/plain;base64,").unwrap();
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_text_document_becomes_a_text_file_with_its_title() {
        let req = user_turn(json!([
            { "type": "text", "text": "What was EMEA in Q2?" },
            { "type": "document", "title": "sales.csv",
              "source": { "type": "text", "media_type": "text/plain", "data": "region,q2\nEMEA,913" } }
        ]))
        .unwrap();
        let parts = parts_of(&req, 0);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["type"], "file");
        assert_eq!(parts[1]["file"]["filename"], "sales.csv");
        assert_eq!(decoded_file_text(&parts[1]), "region,q2\nEMEA,913");
    }

    #[test]
    fn a_text_document_without_a_media_type_is_plain_text() {
        let req = user_turn(json!([
            { "type": "document", "source": { "type": "text", "data": "ünïcode ✓" } }
        ]))
        .unwrap();
        assert_eq!(decoded_file_text(&parts_of(&req, 0)[0]), "ünïcode ✓");
    }

    #[test]
    fn a_content_document_becomes_its_own_blocks_with_the_marker_on_the_last() {
        let req = user_turn(json!([
            { "type": "document", "title": "brief",
              "cache_control": { "type": "ephemeral" },
              "source": { "type": "content", "content": [
                  { "type": "text", "text": "Page one." },
                  { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } }
              ] } }
        ]))
        .unwrap();
        let parts = parts_of(&req, 0);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "Page one.");
        assert!(parts[0].get(CACHE_CONTROL).is_none());
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1][CACHE_CONTROL], json!({ "type": "ephemeral" }));
    }

    #[test]
    fn a_content_document_given_as_a_string_is_one_text_part() {
        let req = user_turn(json!([
            { "type": "document", "source": { "type": "content", "content": "Just words." } }
        ]))
        .unwrap();
        assert_eq!(req.messages[0].content, Some(json!("Just words.")));
    }

    #[test]
    fn a_document_inside_a_tool_result_is_kept() {
        let req = user_turn(json!([
            { "type": "tool_result", "tool_use_id": "toolu_1", "content": [
                { "type": "document", "title": "out.txt", "source": { "type": "text", "data": "done: 7781" } }
            ] }
        ]))
        .unwrap();
        assert_eq!(req.messages[0].role, "tool");
        let parts = parts_of(&req, 0);
        assert_eq!(parts[0]["file"]["filename"], "out.txt");
        assert_eq!(decoded_file_text(&parts[0]), "done: 7781");
    }

    #[test]
    fn a_broken_text_or_content_document_is_refused_by_name() {
        let cases = [
            (json!({ "type": "text" }), "text document missing data"),
            (
                json!({ "type": "text", "data": 42 }),
                "text document missing data",
            ),
            (
                json!({ "type": "content" }),
                "content document missing content",
            ),
            (
                json!({ "type": "content", "content": [{ "type": "document", "source": { "type": "text", "data": "x" } }] }),
                "content document cannot hold a 'document' block",
            ),
            (
                json!({ "type": "file", "file_id": "file_1" }),
                "unsupported document source type: 'file'",
            ),
        ];
        for (source, expected) in cases {
            let err = user_turn(json!([{ "type": "document", "source": source }])).unwrap_err();
            match err {
                GatewayError::BadRequest(detail) => {
                    assert!(detail.contains(expected), "{detail} should name {expected}")
                }
                other => panic!("expected BadRequest, got {other:?}"),
            }
        }
    }

    fn search_result(content: Value) -> Value {
        json!({ "type": "search_result", "source": "https://docs.example.com/api",
                "title": "API Reference", "content": content,
                "citations": { "enabled": true } })
    }

    const RENDERED: &str = "\n\nSearch result: API Reference\nSource: https://docs.example.com/api\n\nTimeouts default to 30 s.\n\nSet `timeout_ms` to change it.\n\n";

    fn refusal(blocks: Value) -> String {
        match user_turn(blocks).unwrap_err() {
            GatewayError::BadRequest(detail) => detail,
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn a_search_result_reaches_the_model_as_text_with_its_title_and_source() {
        let req = user_turn(json!([
            { "type": "text", "text": "What is the timeout?" },
            search_result(json!([
                { "type": "text", "text": "Timeouts default to 30 s." },
                { "type": "text", "text": "Set `timeout_ms` to change it." }
            ]))
        ]))
        .unwrap();
        assert_eq!(
            req.messages[0].content,
            Some(json!(format!("What is the timeout?{RENDERED}")))
        );
    }

    #[test]
    fn a_search_result_inside_a_tool_result_is_kept() {
        let req = user_turn(json!([
            { "type": "tool_result", "tool_use_id": "toolu_1", "content": [
                search_result(json!([
                    { "type": "text", "text": "Timeouts default to 30 s." },
                    { "type": "text", "text": "Set `timeout_ms` to change it." }
                ]))
            ] }
        ]))
        .unwrap();
        assert_eq!(req.messages[0].role, "tool");
        assert_eq!(req.messages[0].content, Some(json!(RENDERED)));
    }

    #[test]
    fn a_malformed_search_result_is_refused_by_name() {
        let cases = [
            (
                json!({ "type": "search_result", "title": "t", "content": [{ "type": "text", "text": "x" }] }),
                "search_result block missing 'source'",
            ),
            (
                json!({ "type": "search_result", "source": "s", "content": [{ "type": "text", "text": "x" }] }),
                "search_result block missing 'title'",
            ),
            (
                json!({ "type": "search_result", "source": "s", "title": "t" }),
                "search_result block missing 'content'",
            ),
            (
                search_result(json!([
                    { "type": "image", "source": { "type": "url", "url": "https://x.test/a.png" } }
                ])),
                "a search_result can hold only text blocks, not 'image'",
            ),
        ];
        for (block, expected) in cases {
            let detail = refusal(json!([block]));
            assert!(detail.contains(expected), "{detail} should say {expected}");
        }
    }

    #[test]
    fn a_container_upload_is_refused_with_the_reason() {
        let detail = refusal(json!([
            { "type": "text", "text": "Analyse this." },
            { "type": "container_upload", "file_id": "file_011" }
        ]));
        assert_eq!(
            detail,
            "'container_upload' blocks hand a file to Anthropic's code execution sandbox, \
             which the router does not run; send the file as a document or image block instead"
        );
    }

    #[test]
    fn an_unknown_block_type_is_refused_naming_what_the_router_reads() {
        let detail = refusal(json!([{ "type": "hologram", "data": "?" }]));
        assert_eq!(
            detail,
            "unsupported content block type: 'hologram' (the router reads text, image, \
             document, search_result and tool_result blocks)"
        );
    }
}
