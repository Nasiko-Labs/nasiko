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

use serde_json::{Value, json};

use super::{ChatStreamRenderer, InboundParser};
use crate::error::GatewayError;
use crate::ir::{
    CACHE_CONTROL, ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse,
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
                "user" => append_user_from_anthropic(content, &mut oa_messages),
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

/// An Anthropic user turn → one or more OpenAI messages. `tool_result` blocks become
/// `{role:"tool"}` messages (keyed by `tool_use_id`); text becomes a user message.
fn append_user_from_anthropic(content: &Value, out: &mut Vec<Value>) {
    match content {
        Value::String(s) => out.push(json!({ "role": "user", "content": s })),
        Value::Array(blocks) => {
            for b in blocks {
                if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let result = b.get("content").map(anthropic_text).unwrap_or_default();
                    let content = match tool_result_marker(b) {
                        Some(marker) => {
                            json!([{ "type": "text", "text": result, CACHE_CONTROL: marker }])
                        }
                        None => json!(result),
                    };
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": b.get("tool_use_id").and_then(Value::as_str).unwrap_or_default(),
                        "content": content,
                    }));
                }
            }
            let text = text_content_from_anthropic(content);
            if !is_blank(&text) {
                out.push(json!({ "role": "user", "content": text }));
            }
        }
        _ => {}
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
    format!("msg_{}", id.trim_start_matches("chatcmpl-"))
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
}
