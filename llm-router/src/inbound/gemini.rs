//! Gemini inbound parser — the agent speaks the Gemini `generateContent` API.
//!
//! The **inverse** of the outbound Gemini spoke (`providers/gemini.rs`):
//! - `parse_chat`: a Gemini `:generateContent` body → canonical IR. `systemInstruction`
//!   → a `system` message; `contents[role:model]` `functionCall` parts → assistant
//!   `tool_calls[]`; `contents[role:user]` `functionResponse` parts → `{role:"tool"}`
//!   messages. Gemini keys `functionResponse` by function **name** (no call id), so we
//!   synthesize ids for `functionCall`s and link responses by name.
//! - `render_chat_response`: IR → a Gemini `GenerateContentResponse` (text /
//!   `functionCall` parts, `finish_reason` → `finishReason`, usage → `usageMetadata`).
//! - the streaming renderer: flat IR chunks → Gemini's `data:`-only SSE (no `event:`
//!   lines, no `[DONE]`); tool-call argument fragments are accumulated and emitted as a
//!   single complete `functionCall` in the terminal chunk (Gemini never fragments args).
//!
//! Note: Gemini signals streaming by the *endpoint* (`:streamGenerateContent`), not a
//! body field, so the handler forces the stream flag — `parse_chat` carries no `stream`.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{ChatStreamRenderer, InboundParser};
use crate::error::GatewayError;
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, ContentPart, DataSource, EmbeddingsRequest,
    EmbeddingsResponse, Usage, parts_to_content_value,
};

pub struct GeminiInbound;

impl InboundParser for GeminiInbound {
    fn parse_chat(&self, body: Value) -> Result<ChatRequest, GatewayError> {
        let mut oa_messages: Vec<Value> = Vec::new();

        if let Some(si) = body
            .get("systemInstruction")
            .or_else(|| body.get("system_instruction"))
        {
            let text = parts_text(si.get("parts"));
            if !text.is_empty() {
                oa_messages.push(json!({ "role": "system", "content": text }));
            }
        }

        // Gemini gives function calls no id and keys responses by name; synthesize ids
        // on the way through so OpenAI `tool`/`tool_call_id` linkage is preserved.
        let mut name_to_id: HashMap<String, String> = HashMap::new();
        let mut call_counter = 0;

        for content in body
            .get("contents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let role = content
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            let parts = content.get("parts").and_then(Value::as_array);

            if role == "model" {
                let mut text = String::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                for part in parts.into_iter().flatten() {
                    if let Some(t) = part.get("text").and_then(Value::as_str) {
                        text.push_str(t);
                    } else if let Some(fc) = part.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let id = format!("call_{name}_{call_counter}");
                        call_counter += 1;
                        name_to_id.insert(name.clone(), id.clone());
                        let arguments = fc
                            .get("args")
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "{}".to_string());
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": arguments }
                        }));
                    }
                }
                let mut msg = json!({ "role": "assistant" });
                if tool_calls.is_empty() {
                    msg["content"] = json!(text);
                } else {
                    msg["tool_calls"] = json!(tool_calls);
                    msg["content"] = if text.is_empty() {
                        Value::Null
                    } else {
                        json!(text)
                    };
                }
                oa_messages.push(msg);
            } else {
                // user turn: text parts, inlineData parts, and/or functionResponse parts.
                let mut user_parts: Vec<ContentPart> = Vec::new();
                let mut had_fr = false;
                for part in parts.into_iter().flatten() {
                    if let Some(fr) = part.get("functionResponse") {
                        had_fr = true;
                        let name = fr.get("name").and_then(Value::as_str).unwrap_or_default();
                        let id = name_to_id
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| name.to_string());
                        oa_messages.push(function_response_message(fr, &id)?);
                    } else if let Some(t) = part.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            user_parts.push(ContentPart::Text(t.to_string()));
                        }
                    } else if let Some(inline) = part.get("inlineData") {
                        user_parts.push(inline_data_part(inline)?);
                    } else if part.get("fileData").is_some() {
                        return Err(file_data_refused());
                    } else {
                        return Err(GatewayError::BadRequest(format!(
                            "unsupported gemini content part: {part}"
                        )));
                    }
                }
                if !user_parts.is_empty() {
                    let content = parts_to_content_value(&user_parts);
                    oa_messages.push(json!({ "role": "user", "content": content }));
                } else if !had_fr {
                    oa_messages.push(json!({ "role": "user", "content": "" }));
                }
            }
        }

        let mut oa = json!({ "messages": oa_messages });
        if let Some(tools) = body.get("tools").and_then(Value::as_array) {
            let mut decls: Vec<Value> = Vec::new();
            for t in tools {
                for fd in t
                    .get("functionDeclarations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let mut function = json!({ "name": fd.get("name").and_then(Value::as_str).unwrap_or_default() });
                    if let Some(d) = fd.get("description") {
                        function["description"] = d.clone();
                    }
                    if let Some(p) = fd.get("parameters") {
                        function["parameters"] = p.clone();
                    }
                    decls.push(json!({ "type": "function", "function": function }));
                }
            }
            if !decls.is_empty() {
                oa["tools"] = json!(decls);
            }
        }
        if let Some(tc) = body.get("toolConfig").and_then(tool_choice_from_gemini) {
            oa["tool_choice"] = tc;
        }
        if let Some(gc) = body.get("generationConfig") {
            if let Some(t) = gc.get("temperature") {
                oa["temperature"] = t.clone();
            }
            if let Some(mt) = gc.get("maxOutputTokens") {
                oa["max_tokens"] = mt.clone();
            }
        }

        serde_json::from_value(oa)
            .map_err(|e| GatewayError::BadRequest(format!("invalid gemini request: {e}")))
    }

    fn render_chat_response(&self, resp: ChatResponse) -> Value {
        let usage = resp.usage.clone();
        let choice = resp.choices.into_iter().next();
        let finish = choice.as_ref().and_then(|c| c.finish_reason.clone());

        let mut parts: Vec<Value> = Vec::new();
        if let Some(c) = &choice {
            if let Some(text) = c.message.text()
                && !text.is_empty()
            {
                parts.push(json!({ "text": text }));
            }
            if let Some(tool_calls) = &c.message.tool_calls {
                for tc in tool_calls {
                    let args: Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
                    parts.push(
                        json!({ "functionCall": { "name": tc.function.name, "args": args } }),
                    );
                }
            }
        }

        let finish_reason = finish
            .as_deref()
            .map(gemini_finish_reason)
            .unwrap_or("STOP");
        let mut out = json!({
            "candidates": [{
                "content": { "role": "model", "parts": parts },
                "finishReason": finish_reason,
                "index": 0,
            }]
        });
        if let Some(u) = usage {
            out["usageMetadata"] = usage_metadata(&u);
        }
        with_identity(out, &resp.id, &resp.model)
    }

    fn chat_stream_renderer(&self) -> Box<dyn ChatStreamRenderer> {
        Box::new(GeminiStreamRenderer::default())
    }

    fn parse_embeddings(&self, _body: Value) -> Result<EmbeddingsRequest, GatewayError> {
        Err(GatewayError::BadRequest(
            "Gemini inbound embeddings are not supported on this route".to_string(),
        ))
    }

    fn render_embeddings(&self, resp: EmbeddingsResponse) -> Value {
        serde_json::to_value(resp).unwrap_or(Value::Null)
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Concatenate `text` fields across a Gemini `parts` array.
/// Gemini `inlineData` → canonical part, keyed on the declared mime type.
///
/// This used to accept only `image/*` and `application/pdf` and 400 everything else,
/// which made the router stricter than the service it fronts: Gemini itself takes audio,
/// video and a range of text/document types. The sharpest symptom was self-inflicted —
/// our *outbound* Gemini converter emits `inlineData` with `audio/…`, a shape this
/// parser then refused, so the same audio reached the same backend or not depending
/// only on which SDK the caller happened to use.
///
/// The document list is explicit rather than a blanket accept: an unknown type is still
/// worth a 400, because forwarding bytes Gemini will reject costs a round trip and
/// returns a worse error than ours.
fn gemini_inline_part(mime_type: &str, source: DataSource) -> Result<ContentPart, GatewayError> {
    /// Non-`text/*` types Gemini accepts as documents.
    const DOCUMENT_TYPES: &[&str] = &[
        "application/pdf",
        "application/json",
        "application/xml",
        "application/rtf",
        "application/x-javascript",
        "application/x-python",
    ];

    if mime_type.starts_with("image/") {
        Ok(ContentPart::Image {
            source,
            detail: None,
        })
    } else if let Some(format) = mime_type.strip_prefix("audio/") {
        // ContentPart::Audio carries the bare format token, matching OpenAI's
        // `input_audio.format`; the outbound converter reassembles `audio/{format}`.
        match source {
            DataSource::Inline { data, .. } => Ok(ContentPart::Audio {
                data,
                format: format.to_string(),
            }),
            DataSource::Url(_) => Err(GatewayError::BadRequest(
                "inlineData audio must carry inline bytes".to_string(),
            )),
        }
    } else if mime_type.starts_with("video/") {
        Ok(ContentPart::Video { source })
    } else if mime_type.starts_with("text/") || DOCUMENT_TYPES.contains(&mime_type) {
        Ok(ContentPart::File {
            source,
            filename: None,
        })
    } else {
        Err(GatewayError::BadRequest(format!(
            "unsupported inlineData mimeType: '{mime_type}'"
        )))
    }
}

fn inline_data_part(inline: &Value) -> Result<ContentPart, GatewayError> {
    let mime_type = inline
        .get("mimeType")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("inlineData missing mimeType".to_string()))?;
    let data = inline
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("inlineData missing data".to_string()))?;
    let source = DataSource::from_inline(mime_type, data);
    let mime_type = source.media_type().unwrap_or_default().to_owned();
    gemini_inline_part(&mime_type, source)
}

fn file_data_refused() -> GatewayError {
    GatewayError::BadRequest(
        "unsupported content part type: 'fileData'; send the file's bytes as inlineData"
            .to_string(),
    )
}

fn function_response_message(fr: &Value, tool_call_id: &str) -> Result<Value, GatewayError> {
    let text = function_response_text(fr.get("response"));
    let media = fr
        .get("parts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if media.is_empty() {
        return Ok(json!({ "role": "tool", "tool_call_id": tool_call_id, "content": text }));
    }
    let mut parts = Vec::with_capacity(media.len() + 1);
    if !text.is_empty() {
        parts.push(ContentPart::Text(text));
    }
    for part in media {
        if let Some(inline) = part.get("inlineData") {
            parts.push(inline_data_part(inline)?);
        } else if part.get("fileData").is_some() {
            return Err(file_data_refused());
        } else {
            return Err(GatewayError::BadRequest(format!(
                "unsupported gemini functionResponse part: {part}"
            )));
        }
    }
    Ok(json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": parts_to_content_value(&parts),
    }))
}

fn parts_text(parts: Option<&Value>) -> String {
    parts
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Best-effort plain text from a Gemini `functionResponse.response` object. The outbound
/// spoke wraps results as `{ "result": <text> }`; honor that, else stringify the object.
fn function_response_text(response: Option<&Value>) -> String {
    match response {
        Some(Value::Object(map)) => match map.get("result") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => Value::Object(map.clone()).to_string(),
        },
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Gemini `toolConfig` → OpenAI `tool_choice` (inverse of the outbound mapping).
fn tool_choice_from_gemini(tool_config: &Value) -> Option<Value> {
    let fcc = tool_config.get("functionCallingConfig")?;
    match fcc.get("mode").and_then(Value::as_str).unwrap_or("AUTO") {
        "ANY" => fcc
            .get("allowedFunctionNames")
            .and_then(Value::as_array)
            .and_then(|names| names.first())
            .and_then(Value::as_str)
            .map(|name| json!({ "type": "function", "function": { "name": name } }))
            .or_else(|| Some(json!("required"))),
        "NONE" => Some(json!("none")),
        _ => Some(json!("auto")),
    }
}

/// OpenAI `finish_reason` → Gemini `finishReason`. Gemini reports `STOP` even when the
/// model emits a tool call, so `tool_calls` maps to `STOP` too.
fn gemini_finish_reason(finish_reason: &str) -> &'static str {
    match finish_reason {
        "length" => "MAX_TOKENS",
        _ => "STOP",
    }
}

fn usage_metadata(u: &Usage) -> Value {
    let u = u.clone().into_cache_inclusive();
    let mut metadata = json!({
        "promptTokenCount": u.prompt_tokens.unwrap_or(0),
        "candidatesTokenCount": u.completion_tokens.unwrap_or(0),
        "totalTokenCount": u.total_tokens.unwrap_or(0),
    });
    if let Some(cached) = u
        .prompt_tokens_details
        .and_then(|d| d.cached_tokens)
        .filter(|&n| n > 0)
    {
        metadata["cachedContentTokenCount"] = json!(cached);
    }
    metadata
}

fn with_identity(mut event: Value, id: &str, model: &str) -> Value {
    if !model.is_empty() {
        event["modelVersion"] = json!(model);
    }
    if !id.is_empty() {
        event["responseId"] = json!(id);
    }
    event
}

// ── IR stream → Gemini SSE (stateful for tool accumulation) ──────────────────

struct ToolAccum {
    name: String,
    args: String,
}

/// Renders IR chunks into Gemini's `data:`-only SSE. Text deltas stream immediately;
/// tool-call argument fragments accumulate and emit as one complete `functionCall` in
/// the terminal chunk (Gemini sends complete function args, never fragments). No
/// `[DONE]` terminator (Gemini just ends the stream).
#[derive(Default)]
struct GeminiStreamRenderer {
    tools: Vec<ToolAccum>,
    tool_pos: HashMap<i64, usize>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
    id: String,
    model: String,
}

impl ChatStreamRenderer for GeminiStreamRenderer {
    fn render(&mut self, chunk: ChatChunk) -> Vec<String> {
        let mut out = Vec::new();

        if let Some(u) = &chunk.usage {
            self.usage = Some(u.clone());
        }
        if !chunk.id.is_empty() {
            self.id = chunk.id;
        }
        if !chunk.model.is_empty() {
            self.model = chunk.model;
        }

        if let Some(choice) = chunk.choices.first() {
            let delta = &choice.delta;

            if let Some(text) = &delta.content
                && !text.is_empty()
            {
                let event = with_identity(
                    json!({
                        "candidates": [{ "content": { "role": "model", "parts": [{ "text": text }] } }]
                    }),
                    &self.id,
                    &self.model,
                );
                out.push(format!("data: {event}\n\n"));
            }

            if let Some(tool_calls) = &delta.tool_calls {
                for tc in tool_calls {
                    let pos = *self.tool_pos.entry(tc.index).or_insert_with(|| {
                        self.tools.push(ToolAccum {
                            name: String::new(),
                            args: String::new(),
                        });
                        self.tools.len() - 1
                    });
                    if let Some(f) = &tc.function {
                        if let Some(name) = &f.name {
                            self.tools[pos].name = name.clone();
                        }
                        if let Some(args) = &f.arguments {
                            self.tools[pos].args.push_str(args);
                        }
                    }
                }
            }

            if let Some(fr) = &choice.finish_reason {
                self.finish_reason = Some(gemini_finish_reason(fr).to_string());
            }
        }

        out
    }

    fn finish(&mut self) -> Vec<String> {
        let parts: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                let args: Value = serde_json::from_str(&t.args).unwrap_or_else(|_| json!({}));
                json!({ "functionCall": { "name": t.name, "args": args } })
            })
            .collect();

        let finish_reason = self
            .finish_reason
            .clone()
            .unwrap_or_else(|| "STOP".to_string());
        let mut event = json!({
            "candidates": [{
                "content": { "role": "model", "parts": parts },
                "finishReason": finish_reason,
                "index": 0,
            }]
        });
        if let Some(u) = &self.usage {
            event["usageMetadata"] = usage_metadata(u);
        }
        let event = with_identity(event, &self.id, &self.model);
        vec![format!("data: {event}\n\n")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ChunkChoice, Delta, FunctionCallDelta, ToolCallDelta};
    use serde_json::Map;

    // ── parse_chat (Gemini request → IR) ──────────────────────────────────────

    #[test]
    fn parse_system_tools_and_generation_config() {
        let req = GeminiInbound
            .parse_chat(json!({
                "systemInstruction": { "parts": [{ "text": "You are helpful." }] },
                "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
                "tools": [{ "functionDeclarations": [
                    { "name": "translate_text", "description": "Translate",
                      "parameters": { "type": "object", "properties": { "text": { "type": "string" } } } }
                ]}],
                "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
                "generationConfig": { "temperature": 0.2, "maxOutputTokens": 512 }
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[0].text().as_deref(), Some("You are helpful."));
        assert_eq!(req.temperature, Some(0.2));
        assert_eq!(req.max_tokens, Some(512));
        let tool = &req.tools.as_ref().unwrap()[0];
        assert_eq!(tool.function.name, "translate_text");
        assert_eq!(
            tool.function.parameters.as_ref().unwrap()["properties"]["text"]["type"],
            "string"
        );
        assert_eq!(req.tool_choice, Some(json!("auto")));
    }

    #[test]
    fn parse_function_call_and_response_link_by_name() {
        // model functionCall (no id) → assistant tool_calls with a synthesized id;
        // the following user functionResponse (keyed by name) → a {role:"tool"} message
        // carrying that same synthesized id.
        let req = GeminiInbound
            .parse_chat(json!({
                "contents": [
                    { "role": "user", "parts": [{ "text": "translate" }] },
                    { "role": "model", "parts": [
                        { "functionCall": { "name": "translate_text", "args": { "text": "hi" } } }
                    ]},
                    { "role": "user", "parts": [
                        { "functionResponse": { "name": "translate_text", "response": { "result": "नमस्ते" } } }
                    ]}
                ]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 3);
        let assistant = &req.messages[1];
        assert_eq!(assistant.role, "assistant");
        let tc = &assistant.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.function.name, "translate_text");
        let synthesized = tc.id.clone();
        assert!(!synthesized.is_empty());
        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["text"], "hi");

        let tool_msg = &req.messages[2];
        assert_eq!(tool_msg.role, "tool");
        assert_eq!(tool_msg.tool_call_id.as_ref(), Some(&synthesized)); // linked by name → id
        assert_eq!(tool_msg.text().as_deref(), Some("नमस्ते"));
    }

    #[test]
    fn parse_tool_choice_any_with_allowed_name() {
        let req = GeminiInbound
            .parse_chat(json!({
                "contents": [{ "role": "user", "parts": [{ "text": "x" }] }],
                "toolConfig": { "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": ["f"] } }
            }))
            .unwrap();
        assert_eq!(
            req.tool_choice,
            Some(json!({ "type": "function", "function": { "name": "f" } }))
        );
    }

    // ── render_chat_response (IR → Gemini response) ───────────────────────────

    #[test]
    fn render_text_response() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": "gemini-1.5-pro",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Hello there" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
        }))
        .unwrap();
        let v = GeminiInbound.render_chat_response(resp);
        assert_eq!(v["candidates"][0]["content"]["role"], "model");
        assert_eq!(
            v["candidates"][0]["content"]["parts"][0]["text"],
            "Hello there"
        );
        assert_eq!(v["candidates"][0]["finishReason"], "STOP");
        assert_eq!(v["usageMetadata"]["promptTokenCount"], 5);
        assert_eq!(v["usageMetadata"]["totalTokenCount"], 7);
        assert!(v["usageMetadata"].get("cachedContentTokenCount").is_none());
    }

    #[test]
    fn a_reply_served_from_cache_reports_gemini_usage_with_the_cached_count() {
        for usage in [
            json!({ "prompt_tokens": 4, "completion_tokens": 75, "total_tokens": 79,
                    "cache_read_input_tokens": 10431 }),
            json!({ "prompt_tokens": 10435, "completion_tokens": 75, "total_tokens": 10510,
                    "prompt_tokens_details": { "cached_tokens": 10431 } }),
        ] {
            let resp: ChatResponse = serde_json::from_value(json!({
                "id": "chatcmpl-1", "object": "chat.completion", "model": "m",
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                "usage": usage.clone()
            }))
            .unwrap();

            let v = GeminiInbound.render_chat_response(resp);

            assert_eq!(
                v["usageMetadata"],
                json!({ "promptTokenCount": 10435, "candidatesTokenCount": 75,
                        "totalTokenCount": 10510, "cachedContentTokenCount": 10431 }),
                "{usage}"
            );
        }
    }

    #[test]
    fn render_tool_call_response() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl-2", "object": "chat.completion", "model": "gemini-1.5-pro",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": null, "tool_calls": [{
                "id": "call_x", "type": "function",
                "function": { "name": "translate_text", "arguments": "{\"text\":\"hi\"}" }
            }]}, "finish_reason": "tool_calls" }]
        }))
        .unwrap();
        let v = GeminiInbound.render_chat_response(resp);
        let fc = &v["candidates"][0]["content"]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "translate_text");
        assert_eq!(fc["args"]["text"], "hi");
        // Gemini reports STOP even for tool calls.
        assert_eq!(v["candidates"][0]["finishReason"], "STOP");
    }

    #[test]
    fn a_reply_names_the_model_that_answered_and_its_response_id() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "msg_01abc", "object": "chat.completion", "model": "claude-haiku-5-5",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" }, "finish_reason": "stop" }]
        }))
        .unwrap();

        let v = GeminiInbound.render_chat_response(resp);

        assert_eq!(v["modelVersion"], "claude-haiku-5-5");
        assert_eq!(v["responseId"], "msg_01abc");
    }

    #[test]
    fn a_reply_without_an_id_or_model_carries_neither_field() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "", "object": "chat.completion", "model": "",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" }, "finish_reason": "stop" }]
        }))
        .unwrap();

        let v = GeminiInbound.render_chat_response(resp);

        assert!(v.get("modelVersion").is_none(), "{v}");
        assert!(v.get("responseId").is_none(), "{v}");
    }

    #[test]
    fn every_stream_event_names_the_model_and_response_id() {
        let mut r = GeminiInbound.chat_stream_renderer();
        let mut blank = text_chunk("");
        blank.id = String::new();
        blank.model = String::new();

        let events: Vec<Value> = [
            r.render(text_chunk("Hel")),
            r.render(blank.clone()),
            r.render({
                let mut later = blank;
                later.choices[0].delta.content = Some("lo".into());
                later
            }),
            r.finish(),
        ]
        .concat()
        .iter()
        .map(|frame| serde_json::from_str(frame.trim().strip_prefix("data: ").unwrap()).unwrap())
        .collect();

        assert_eq!(events.len(), 3, "{events:?}");
        for event in &events {
            assert_eq!(event["modelVersion"], "gemini-1.5-pro", "{event}");
            assert_eq!(event["responseId"], "chatcmpl-s", "{event}");
        }
    }

    // ── streaming renderer (IR chunks → Gemini SSE) ───────────────────────────

    fn text_chunk(text: &str) -> ChatChunk {
        ChatChunk {
            id: "chatcmpl-s".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "gemini-1.5-pro".into(),
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
    fn stream_text_emits_candidates_chunks_no_done() {
        let mut r = GeminiInbound.chat_stream_renderer();
        let first = r.render(text_chunk("Hel")).join("");
        assert!(first.starts_with("data: "));
        assert!(!first.contains("event:")); // Gemini SSE has no event: lines
        assert!(first.contains("\"role\":\"model\""));
        assert!(first.contains("Hel"));

        let mut finish_chunk = text_chunk("");
        finish_chunk.choices[0].delta = Delta::default();
        finish_chunk.choices[0].finish_reason = Some("stop".into());
        finish_chunk.usage = Some(Usage {
            prompt_tokens: Some(4),
            completion_tokens: Some(2),
            total_tokens: Some(6),
            ..Default::default()
        });
        let _ = r.render(finish_chunk);

        let fin = r.finish().join("");
        assert!(fin.contains("\"finishReason\":\"STOP\""));
        assert!(fin.contains("\"totalTokenCount\":6"));
        assert!(!fin.contains("[DONE]"));
    }

    #[test]
    fn stream_tool_call_accumulates_into_single_function_call() {
        let mut r = GeminiInbound.chat_stream_renderer();

        let open = ChatChunk {
            id: "chatcmpl-t".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "gemini-1.5-pro".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: Some(vec![ToolCallDelta {
                        index: 0,
                        id: Some("call_1".into()),
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
            model: "gemini-1.5-pro".into(),
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

        // Tool fragments produce nothing inline — they accumulate.
        assert!(r.render(open).is_empty());
        assert!(r.render(frag("{\"text\":")).is_empty());
        assert!(r.render(frag("\"hi\"}")).is_empty());

        let fin = r.finish().join("");
        let v: Value = serde_json::from_str(fin.trim_start_matches("data: ").trim()).unwrap();
        let fc = &v["candidates"][0]["content"]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "translate_text");
        assert_eq!(fc["args"]["text"], "hi"); // reassembled from fragments
    }

    #[test]
    fn parse_chat_with_inline_data_image() {
        let req = GeminiInbound
            .parse_chat(json!({
                "contents": [{
                    "role": "user",
                    "parts": [
                        { "text": "Describe image" },
                        { "inlineData": { "mimeType": "image/jpeg", "data": "abc123base64" } }
                    ]
                }]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 1);
        let msg = &req.messages[0];
        assert_eq!(msg.role, "user");
        let content_parts = msg.content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(content_parts.len(), 2);
        assert_eq!(content_parts[0]["type"], "text");
        assert_eq!(content_parts[0]["text"], "Describe image");
        assert_eq!(content_parts[1]["type"], "image_url");
        assert_eq!(
            content_parts[1]["image_url"]["url"],
            "data:image/jpeg;base64,abc123base64"
        );
    }

    #[test]
    fn an_empty_user_turn_is_forwarded_for_the_provider_to_judge() {
        for turn in [
            json!({ "role": "user", "parts": [] }),
            json!({ "role": "user" }),
            json!({ "role": "user", "parts": [{ "text": "" }] }),
        ] {
            let req = GeminiInbound
                .parse_chat(json!({ "contents": [turn.clone()] }))
                .unwrap();

            assert_eq!(req.messages.len(), 1, "{turn}");
            assert_eq!(req.messages[0].role, "user", "{turn}");
            assert_eq!(req.messages[0].content, Some(json!("")), "{turn}");
        }
    }

    #[test]
    fn a_turn_of_only_function_responses_adds_no_empty_user_message() {
        let req = GeminiInbound
            .parse_chat(json!({ "contents": [
                { "role": "user", "parts": [{ "text": "Read it." }] },
                { "role": "model", "parts": [{ "functionCall": { "name": "read_file", "args": {} } }] },
                { "role": "user", "parts": [{ "functionResponse": { "name": "read_file", "response": { "text": "7781" } } }] }
            ]}))
            .unwrap();

        let roles: Vec<&str> = req.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "tool"]);
    }

    #[test]
    fn an_inline_data_mime_type_is_normalized_before_it_is_mapped() {
        let req = GeminiInbound
            .parse_chat(json!({
                "contents": [{
                    "role": "user",
                    "parts": [
                        { "inlineData": { "mimeType": "Image/PNG", "data": "QUJD" } },
                        { "inlineData": { "mimeType": "application/pdf; name=q3.pdf", "data": "QUJD" } }
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
        assert_eq!(parts[0]["image_url"]["url"], "data:image/png;base64,QUJD");
        assert_eq!(
            parts[1]["file"]["file_data"],
            "data:application/pdf;base64,QUJD"
        );
    }

    #[test]
    fn parse_chat_with_inline_data_pdf() {
        let req = GeminiInbound
            .parse_chat(json!({
                "contents": [{
                    "role": "user",
                    "parts": [
                        { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0xLjQK" } }
                    ]
                }]
            }))
            .unwrap();

        assert_eq!(req.messages.len(), 1);
        let msg = &req.messages[0];
        assert_eq!(msg.role, "user");
        let content_parts = msg.content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(content_parts.len(), 1);
        assert_eq!(content_parts[0]["type"], "file");
        assert_eq!(
            content_parts[0]["file"]["file_data"],
            "data:application/pdf;base64,JVBERi0xLjQK"
        );
        // `inlineData` never carries a name, and OpenAI refuses a `file` object with
        // neither `filename` nor `file_id` — so one is synthesized from the mime type.
        assert_eq!(content_parts[0]["file"]["filename"], "document.pdf");
    }

    #[test]
    fn audio_video_and_text_inline_data_are_accepted() {
        // All four were a 400 before; Gemini itself takes every one of them.
        for (mime, expected_type) in [
            ("audio/wav", "input_audio"),
            ("video/mp4", "video_url"),
            ("text/csv", "file"),
            ("application/json", "file"),
        ] {
            let req = GeminiInbound
                .parse_chat(json!({
                    "contents": [{ "role": "user", "parts": [
                        { "text": "what is this?" },
                        { "inlineData": { "mimeType": mime, "data": "QUJD" } }
                    ]}]
                }))
                .unwrap_or_else(|e| panic!("{mime} rejected: {e}"));
            let parts = req.messages[0]
                .content
                .as_ref()
                .unwrap()
                .as_array()
                .unwrap();
            assert_eq!(parts.len(), 2, "{mime}");
            assert_eq!(parts[1]["type"], expected_type, "{mime}");
        }
    }

    /// The asymmetry this change exists to kill: our own outbound Gemini converter
    /// emits `inlineData` with `audio/…`, and this parser used to refuse that exact
    /// shape — so the same audio reached the same backend or not depending only on
    /// which SDK the caller used.
    #[test]
    fn audio_the_outbound_converter_emits_is_accepted_by_the_inbound_parser() {
        let req = GeminiInbound
            .parse_chat(json!({
                "contents": [{ "role": "user", "parts": [
                    { "inlineData": { "mimeType": "audio/wav", "data": "UklGRg==" } }
                ]}]
            }))
            .unwrap();
        let parts = req.messages[0]
            .content
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap();
        // Round-trips to the canonical audio shape, format token split off the mime type.
        assert_eq!(parts[0]["type"], "input_audio");
        assert_eq!(parts[0]["input_audio"]["format"], "wav");
        assert_eq!(parts[0]["input_audio"]["data"], "UklGRg==");
    }

    #[test]
    fn genuinely_unknown_mime_types_are_still_rejected() {
        // Widening the rule must not make the parser permissive.
        let err = GeminiInbound
            .parse_chat(json!({
                "contents": [{ "role": "user", "parts": [
                    { "inlineData": { "mimeType": "application/x-tar", "data": "QUJD" } }
                ]}]
            }))
            .unwrap_err();
        assert!(err.to_string().contains("application/x-tar"), "{err}");
    }

    #[test]
    fn parse_chat_with_unsupported_parts_rejected() {
        // fileData not supported
        let err = GeminiInbound
            .parse_chat(json!({
                "contents": [{
                    "role": "user",
                    "parts": [
                        { "fileData": { "fileUri": "https://example.com/file.pdf", "mimeType": "application/pdf" } }
                    ]
                }]
            }))
            .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
        assert!(err.to_string().contains("inlineData"), "{err}");
    }

    fn tool_turn(function_response: Value) -> Value {
        json!({ "contents": [
            { "role": "user", "parts": [{ "text": "What colours are in the picture?" }] },
            { "role": "model", "parts": [{ "functionCall": { "name": "read_file", "args": { "path": "img/fact.png" } } }] },
            { "role": "user", "parts": [{ "functionResponse": function_response }] }
        ]})
    }

    #[test]
    fn media_in_a_function_response_reaches_the_tool_message() {
        let req = GeminiInbound
            .parse_chat(tool_turn(json!({
                "name": "read_file",
                "response": { "result": "image img/fact.png" },
                "parts": [
                    { "inlineData": { "mimeType": "image/png", "data": "QUJD", "displayName": "fact.png" } },
                    { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0=" } }
                ]
            })))
            .unwrap();

        let roles: Vec<&str> = req.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "tool"]);
        let tool = &req.messages[2];
        assert_eq!(
            tool.tool_call_id.as_deref(),
            req.messages[1]
                .tool_calls
                .as_ref()
                .map(|c| c[0].id.as_str())
        );
        let parts = tool.content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["text"], "image img/fact.png");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,QUJD");
        assert_eq!(
            parts[2]["file"]["file_data"],
            "data:application/pdf;base64,JVBERi0="
        );
    }

    #[test]
    fn a_function_response_of_only_media_carries_no_empty_text() {
        let req = GeminiInbound
            .parse_chat(tool_turn(json!({
                "name": "read_file",
                "parts": [{ "inlineData": { "mimeType": "image/png", "data": "QUJD" } }]
            })))
            .unwrap();

        let tool = req.messages[2]
            .content
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(tool.len(), 1);
        assert_eq!(tool[0]["image_url"]["url"], "data:image/png;base64,QUJD");
    }

    #[test]
    fn a_function_response_without_parts_stays_plain_text() {
        let req = GeminiInbound
            .parse_chat(tool_turn(json!({
                "name": "read_file",
                "response": { "result": "7781" },
                "parts": []
            })))
            .unwrap();

        assert_eq!(req.messages[2].content, Some(json!("7781")));
    }

    #[test]
    fn a_function_response_part_the_router_cannot_carry_is_refused_by_name() {
        for (part, named) in [
            (
                json!({ "fileData": { "fileUri": "https://generativelanguage.googleapis.com/v1beta/files/abc", "mimeType": "image/png" } }),
                "fileData",
            ),
            (
                json!({ "inlineData": { "mimeType": "application/x-tar", "data": "QUJD" } }),
                "application/x-tar",
            ),
            (json!({ "inlineData": { "data": "QUJD" } }), "mimeType"),
            (json!({ "text": "a caption" }), "functionResponse part"),
        ] {
            let err = GeminiInbound
                .parse_chat(tool_turn(json!({
                    "name": "read_file",
                    "response": { "result": "x" },
                    "parts": [part.clone()]
                })))
                .unwrap_err();
            assert!(matches!(err, GatewayError::BadRequest(_)), "{part}");
            assert!(err.to_string().contains(named), "{part}: {err}");
        }
    }
}
