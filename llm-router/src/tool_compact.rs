//! Opt-in compact tool schemas at the egress seam.
//!
//! Off by default. When disabled, [`apply`] is a no-op and the request is untouched
//! (byte-identical to pre-feature behaviour). When enabled, native `tools` are replaced
//! with a system message carrying the compact definitions + call-format instructions;
//! the model must emit `<<…>>` markers which the router converts back to native
//! OpenAI `tool_calls` via [`decode_chat_response`] (non-streaming).
//!
//! ## `tool_choice`
//!
//! - `"auto"` / absent — compact when schemas supported; soft choice cleared.
//! - `"required"` — compact; a short MUST-call line is appended so the model is still
//!   instructed to emit a call after native tools are stripped (providers cannot enforce
//!   `tool_choice` without a `tools` array). Works for both non-streaming and streaming
//!   (stream path uses [`CompactStreamSession`] + [`nasiko_tool_compact::StreamDecoder`]).
//! - Specific function force (`{"type":"function","function":{"name":…}}`) — **bypass**
//!   compaction (native tools kept); compact markers cannot name-force a single tool
//!   through the provider API.
//!
//! Streaming response decode reuses [`nasiko_tool_compact::StreamDecoder`] and emits
//! native OpenAI-shaped [`ToolCallDelta`] chunks after alias reverse-mapping.

use std::collections::HashMap;

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall as CompactToolCall, ToolDef as CompactToolDef,
    decode_calls, encode_tools,
};
use serde_json::{Map, Value};

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::ir::chat::{
    ChatChunk, ChatResponse, ChunkChoice, Delta, FunctionCall, FunctionCallDelta, Message,
    ToolCall, ToolCallDelta, ToolDef,
};

/// Why compaction was skipped.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    NoTools,
    /// Specific function `tool_choice` (cannot name-force a single tool via markers).
    ForcedToolChoice,
    UnsupportedSchema,
}

impl Skipped {
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoTools => "no_tools",
            Self::ForcedToolChoice => "forced_tool_choice",
            Self::UnsupportedSchema => "unsupported_schema",
        }
    }
}

/// Successful compaction — tool defs retained for response decode / validation.
#[derive(Debug, Clone)]
pub(crate) struct Applied {
    /// Model-facing tool defs (compact aliases as `name`).
    pub tools: Vec<CompactToolDef>,
    /// Exact `compact_alias → original_tool_name` for MCP / OpenAI execution identity.
    pub original_by_alias: HashMap<String, String>,
    /// True when the client asked for `tool_choice="required"`.
    pub require_call: bool,
}

/// Appended when compacting under `tool_choice="required"` (native enforcement gone).
const REQUIRE_CALL_LINE: &str = "MUST emit <<name {json}>>.";

/// Metadata for `token_usage` / tests.
pub(crate) fn to_metadata(outcome: &Result<Applied, Skipped>) -> Value {
    match outcome {
        Ok(applied) => serde_json::json!({
            "applied": true,
            "require_call": applied.require_call,
        }),
        Err(reason) => serde_json::json!({
            "applied": false,
            "skipped": reason.as_label(),
        }),
    }
}

/// Replace native tools with a compact system message when the flag is on.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Result<Applied, Skipped> {
    if !cfg.tool_compact_enabled {
        return Err(Skipped::Disabled);
    }
    let Some(tools) = req.tools.as_ref() else {
        return Err(Skipped::NoTools);
    };
    if tools.is_empty() {
        return Err(Skipped::NoTools);
    }

    let require_call = is_required_tool_choice(req.tool_choice.as_ref());
    // Specific function force cannot be expressed via compact markers at the provider.
    if forces_specific_function(req.tool_choice.as_ref()) {
        return Err(Skipped::ForcedToolChoice);
    }

    let native_defs: Vec<CompactToolDef> = tools.iter().map(to_compact_def).collect();
    let mut encoded = match encode_tools(&native_defs) {
        Ok(c) => c,
        Err(CompactError::UnsupportedSchema(_)) => return Err(Skipped::UnsupportedSchema),
        // InvalidToolName (incl. unsafe gateway aliasing) and other encode failures → bypass.
        Err(_) => return Err(Skipped::UnsupportedSchema),
    };

    if require_call {
        if !encoded.prompt.is_empty() {
            encoded.prompt.push('\n');
        }
        encoded.prompt.push_str(REQUIRE_CALL_LINE);
    }

    let tools = encoded.tools().to_vec();
    let original_by_alias = encoded.original_by_alias().clone();
    let prompt = encoded.prompt;

    req.messages.push(Message {
        role: "system".into(),
        content: Some(Value::String(prompt)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    req.tools = None;
    // Native tool_choice is meaningless without a tools array.
    req.tool_choice = None;

    Ok(Applied {
        tools,
        original_by_alias,
        require_call,
    })
}

/// Convert compact `<<name {json}>>` assistant text into native OpenAI `tool_calls`.
///
/// No-op when the message already has native `tool_calls`, has no text, or decodes to
/// zero calls (plain answer). Invalid/malformed markers leave the message unchanged
/// (fail closed at the agent: no invented calls).
pub(crate) fn decode_chat_response(resp: &mut ChatResponse, applied: &Applied) {
    for choice in &mut resp.choices {
        rewrite_message_tool_calls(
            &mut choice.message,
            &applied.tools,
            &applied.original_by_alias,
        );
        if choice
            .message
            .tool_calls
            .as_ref()
            .is_some_and(|t| !t.is_empty())
        {
            choice.finish_reason = Some("tool_calls".into());
        }
    }
}

fn rewrite_message_tool_calls(
    message: &mut Message,
    tools: &[CompactToolDef],
    original_by_alias: &HashMap<String, String>,
) {
    if message.tool_calls.as_ref().is_some_and(|t| !t.is_empty()) {
        return;
    }
    let Some(text) = message.text() else {
        return;
    };
    if !text.contains("<<") {
        return;
    }
    let Ok(calls) = decode_calls(&text, tools) else {
        tracing::warn!(
            target: "nasiko::llm_router::tool_compact",
            "compact response decode failed; leaving assistant content unchanged"
        );
        return;
    };
    if calls.is_empty() {
        return;
    }
    // Resolve every alias to the exact original tool name — never guess.
    let Ok(resolved) = resolve_calls(&calls, original_by_alias) else {
        tracing::warn!(
            target: "nasiko::llm_router::tool_compact",
            "compact alias resolution failed; leaving assistant content unchanged"
        );
        return;
    };
    message.tool_calls = Some(to_openai_tool_calls(&resolved));
    // Markers were the tool payload — clear content like a native tool-call turn.
    message.content = None;
}

/// Map decoded compact aliases → original tool names. Fail closed on unknown alias.
fn resolve_calls(
    calls: &[CompactToolCall],
    original_by_alias: &HashMap<String, String>,
) -> Result<Vec<CompactToolCall>, ()> {
    let mut out = Vec::with_capacity(calls.len());
    for c in calls {
        let Some(original) = original_by_alias.get(&c.name) else {
            return Err(());
        };
        out.push(CompactToolCall {
            name: original.clone(),
            arguments: c.arguments.clone(),
        });
    }
    Ok(out)
}

fn to_openai_tool_calls(calls: &[CompactToolCall]) -> Vec<ToolCall> {
    calls
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let arguments =
                serde_json::to_string(&c.arguments).unwrap_or_else(|_| "{}".to_string());
            ToolCall {
                id: format!("call_compact_{i}"),
                kind: "function".into(),
                function: FunctionCall {
                    name: c.name.clone(),
                    arguments,
                },
                extra: Map::new(),
            }
        })
        .collect()
}

/// Incremental compact→native converter for OpenAI-shaped chat streams.
///
/// Feeds assistant `delta.content` into [`StreamDecoder`]. Completed calls are
/// alias-resolved and emitted as native [`ToolCallDelta`] chunks. Marker text is
/// never forwarded to the agent after a successful decode. On failure, buffered
/// content is flushed unchanged (fail closed — no invented ToolCalls).
pub(crate) struct CompactStreamSession {
    decoder: StreamDecoder,
    original_by_alias: HashMap<String, String>,
    /// All provider content seen while still attempting compact decode.
    content_buf: String,
    /// True once any native tool_call delta has been emitted.
    emitted_tools: bool,
    /// Decode/alias failure — forward remaining content as-is.
    failed: bool,
    /// Provider already sent native tool_calls — pass through, skip compact.
    passthrough_native: bool,
    next_index: i64,
    finished: bool,
}

impl CompactStreamSession {
    pub(crate) fn new(applied: &Applied) -> Self {
        Self {
            decoder: StreamDecoder::new(applied.tools.clone()),
            original_by_alias: applied.original_by_alias.clone(),
            content_buf: String::new(),
            emitted_tools: false,
            failed: false,
            passthrough_native: false,
            next_index: 0,
            finished: false,
        }
    }

    /// Transform one provider chunk into zero or more agent-facing chunks.
    pub(crate) fn push_chunk(&mut self, chunk: ChatChunk) -> Vec<ChatChunk> {
        if self.passthrough_native || self.failed {
            return vec![chunk];
        }

        // Native tool_calls from the provider — never mix with compact decode.
        if chunk
            .choices
            .iter()
            .any(|c| c.delta.tool_calls.as_ref().is_some_and(|t| !t.is_empty()))
        {
            self.passthrough_native = true;
            return vec![chunk];
        }

        let mut out = Vec::new();
        let mut content_pieces = String::new();
        for choice in &chunk.choices {
            if let Some(text) = choice.delta.content.as_deref() {
                content_pieces.push_str(text);
            }
        }

        if !content_pieces.is_empty() {
            self.content_buf.push_str(&content_pieces);
            match self.decoder.push(&content_pieces) {
                Ok(new_calls) => {
                    if let Some(chunks) = self.emit_resolved_calls(&chunk, &new_calls) {
                        out.extend(chunks);
                    } else if self.failed {
                        // Alias resolution failed — surface buffered content once.
                        out.push(content_only_chunk(&chunk, &self.content_buf));
                    }
                    // else: still buffering a partial marker — suppress content.
                }
                Err(e) => {
                    tracing::warn!(
                        target: "nasiko::llm_router::tool_compact",
                        error = %e,
                        "compact stream decode failed; forwarding buffered content"
                    );
                    self.failed = true;
                    out.push(content_only_chunk(&chunk, &self.content_buf));
                }
            }
        }

        // Preserve role-only / finish / usage signaling without leaking marker text.
        let mut passthrough = chunk.clone();
        let mut keep = false;
        for choice in &mut passthrough.choices {
            if choice.delta.content.is_some() {
                choice.delta.content = None;
            }
            if choice.delta.role.is_some() {
                keep = true;
            }
            if let Some(fr) = choice.finish_reason.as_deref() {
                keep = true;
                if self.emitted_tools && (fr == "stop" || fr == "end_turn") {
                    choice.finish_reason = Some("tool_calls".into());
                }
            }
        }
        if passthrough.usage.is_some() {
            keep = true;
        }
        if keep
            && (passthrough.usage.is_some()
                || passthrough
                    .choices
                    .iter()
                    .any(|c| c.delta.role.is_some() || c.finish_reason.is_some()))
        {
            // Drop empty choice shells that only had content we suppressed.
            passthrough.choices.retain(|c| {
                c.delta.role.is_some()
                    || c.finish_reason.is_some()
                    || c.delta.tool_calls.is_some()
                    || c.delta.content.is_some()
            });
            if !passthrough.choices.is_empty() || passthrough.usage.is_some() {
                out.push(passthrough);
            }
        }

        out
    }

    /// Finish the stream: flush incomplete-marker failure or leftover plain text.
    pub(crate) fn finish(&mut self, template: &ChatChunk) -> Vec<ChatChunk> {
        if self.finished || self.passthrough_native {
            self.finished = true;
            return Vec::new();
        }
        self.finished = true;

        if self.failed {
            return Vec::new();
        }

        // Take the decoder so we can call finish(self).
        let decoder = std::mem::replace(&mut self.decoder, StreamDecoder::new(Vec::new()));
        match decoder.finish() {
            Ok(all_calls) => {
                // Emit any calls not already emitted via push (normally none).
                let already = self.next_index as usize;
                if already < all_calls.len() {
                    if let Some(chunks) = self.emit_resolved_calls(template, &all_calls[already..])
                    {
                        let mut out = chunks;
                        if self.emitted_tools {
                            out.push(finish_reason_chunk(template, "tool_calls"));
                        }
                        return out;
                    }
                }
                if self.emitted_tools {
                    return Vec::new();
                }
                // Plain answer — no markers / zero calls.
                if !self.content_buf.is_empty() {
                    return vec![content_only_chunk(template, &self.content_buf)];
                }
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::tool_compact",
                    error = %e,
                    "compact stream finish failed; forwarding buffered content if no tools emitted"
                );
                self.failed = true;
                if self.emitted_tools {
                    // Prior complete calls already emitted; do not invent more.
                    Vec::new()
                } else if !self.content_buf.is_empty() {
                    vec![content_only_chunk(template, &self.content_buf)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Returns `Some(chunks)` on success, `None` if alias resolution failed (`self.failed` set).
    fn emit_resolved_calls(
        &mut self,
        template: &ChatChunk,
        calls: &[CompactToolCall],
    ) -> Option<Vec<ChatChunk>> {
        if calls.is_empty() {
            return Some(Vec::new());
        }
        let Ok(resolved) = resolve_calls(calls, &self.original_by_alias) else {
            tracing::warn!(
                target: "nasiko::llm_router::tool_compact",
                "compact stream alias resolution failed; leaving content unchanged"
            );
            self.failed = true;
            return None;
        };
        let mut out = Vec::with_capacity(resolved.len());
        for call in resolved {
            let args = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string());
            let index = self.next_index;
            self.next_index += 1;
            out.push(tool_call_delta_chunk(
                template,
                index,
                &format!("call_compact_{index}"),
                &call.name,
                &args,
            ));
            self.emitted_tools = true;
        }
        Some(out)
    }
}

fn content_only_chunk(template: &ChatChunk, content: &str) -> ChatChunk {
    let mut chunk = template.clone();
    chunk.usage = None;
    chunk.choices = vec![ChunkChoice {
        index: 0,
        delta: Delta {
            role: None,
            content: Some(content.to_string()),
            tool_calls: None,
        },
        finish_reason: None,
    }];
    chunk
}

fn tool_call_delta_chunk(
    template: &ChatChunk,
    index: i64,
    id: &str,
    name: &str,
    arguments: &str,
) -> ChatChunk {
    let mut chunk = template.clone();
    chunk.usage = None;
    chunk.choices = vec![ChunkChoice {
        index: 0,
        delta: Delta {
            role: None,
            content: None,
            tool_calls: Some(vec![ToolCallDelta {
                index,
                id: Some(id.into()),
                kind: Some("function".into()),
                function: Some(FunctionCallDelta {
                    name: Some(name.into()),
                    arguments: Some(arguments.into()),
                }),
            }]),
        },
        finish_reason: None,
    }];
    chunk
}

fn finish_reason_chunk(template: &ChatChunk, reason: &str) -> ChatChunk {
    let mut chunk = template.clone();
    chunk.usage = None;
    chunk.choices = vec![ChunkChoice {
        index: 0,
        delta: Delta::default(),
        finish_reason: Some(reason.into()),
    }];
    chunk
}

fn is_required_tool_choice(tool_choice: Option<&Value>) -> bool {
    matches!(tool_choice, Some(Value::String(s)) if s == "required")
}

/// True only for a forced *named* function — not for `"required"` / `"auto"` / `"none"`.
fn forces_specific_function(tool_choice: Option<&Value>) -> bool {
    match tool_choice {
        Some(Value::Object(m)) => {
            m.get("type").and_then(Value::as_str) == Some("function") || m.contains_key("function")
        }
        _ => false,
    }
}

fn to_compact_def(t: &ToolDef) -> CompactToolDef {
    CompactToolDef {
        name: t.function.name.clone(),
        description: t.function.description.clone(),
        parameters: t.function.parameters.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Choice, FunctionDef};
    use serde_json::json;

    fn sample_req() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("hi".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "ping".into(),
                    description: Some("Ping the host.".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "host": {"type": "string", "description": "Hostname"}
                        },
                        "required": ["host"]
                    })),
                },
                extra: Default::default(),
            }]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_is_byte_identical() {
        let cfg = GatewayConfig {
            tool_compact_enabled: false,
            ..Default::default()
        };
        let original = sample_req();
        let mut req = original.clone();
        let outcome = apply(&mut req, &cfg);
        assert!(matches!(outcome, Err(Skipped::Disabled)));
        let before = serde_json::to_string(&original).unwrap();
        let after = serde_json::to_string(&req).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn enabled_strips_tools_and_injects_system() {
        let cfg = GatewayConfig {
            tool_compact_enabled: true,
            ..Default::default()
        };
        let mut req = sample_req();
        let applied = apply(&mut req, &cfg).unwrap();
        assert!(!applied.require_call);
        assert!(req.tools.is_none());
        assert!(
            req.messages
                .iter()
                .any(|m| { m.role == "system" && m.text().is_some_and(|t| t.contains("CALL <<")) })
        );
    }

    #[test]
    fn required_tool_choice_compacts_non_streaming() {
        let cfg = GatewayConfig {
            tool_compact_enabled: true,
            ..Default::default()
        };
        let mut req = sample_req();
        req.tool_choice = Some(json!("required"));
        let applied = apply(&mut req, &cfg).unwrap();
        assert!(applied.require_call);
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        let sys = req
            .messages
            .iter()
            .find(|m| m.role == "system")
            .unwrap()
            .text()
            .unwrap();
        assert!(sys.contains("CALL <<"));
        assert!(
            sys.contains(REQUIRE_CALL_LINE),
            "must instruct model to emit a call; sys={sys}"
        );
    }

    #[test]
    fn required_tool_choice_streaming_compacts() {
        let cfg = GatewayConfig {
            tool_compact_enabled: true,
            ..Default::default()
        };
        let mut req = sample_req();
        req.tool_choice = Some(json!("required"));
        req.stream = Some(true);
        let applied = apply(&mut req, &cfg).expect("required+stream must compact");
        assert!(applied.require_call);
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        assert!(
            req.messages
                .iter()
                .any(|m| m.role == "system" && m.text().is_some_and(|t| t.contains("CALL <<")))
        );
    }

    #[test]
    fn forced_tool_choice_keeps_native_tools() {
        let cfg = GatewayConfig {
            tool_compact_enabled: true,
            ..Default::default()
        };
        let mut req = sample_req();
        req.tool_choice = Some(json!({
            "type": "function",
            "function": {"name": "ping"}
        }));
        let before = serde_json::to_value(&req).unwrap();
        let outcome = apply(&mut req, &cfg);
        assert_eq!(outcome.err(), Some(Skipped::ForcedToolChoice));
        assert!(
            req.tools.is_some(),
            "tools must remain for forced tool_choice"
        );
        assert_eq!(req.tools.as_ref().unwrap()[0].function.name, "ping");
        assert_eq!(
            req.tool_choice,
            Some(json!({
                "type": "function",
                "function": {"name": "ping"}
            }))
        );
        assert!(
            !req.messages
                .iter()
                .any(|m| m.role == "system" && m.text().is_some_and(|t| t.contains("CALL <<"))),
            "must not inject compact instructions when bypassing"
        );
        let after = serde_json::to_value(&req).unwrap();
        assert_eq!(
            before, after,
            "forced tool_choice path must leave request unchanged"
        );
    }

    #[test]
    fn unsupported_schema_keeps_native_tools() {
        let cfg = GatewayConfig {
            tool_compact_enabled: true,
            ..Default::default()
        };
        let mut req = sample_req();
        req.tools = Some(vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "lookup".into(),
                description: Some("Lookup".into()),
                parameters: Some(json!({
                    "anyOf": [
                        {"type": "string"},
                        {"type": "integer"}
                    ]
                })),
            },
            extra: Default::default(),
        }]);
        let before_tools = serde_json::to_value(&req.tools).unwrap();
        let outcome = apply(&mut req, &cfg);
        assert_eq!(outcome.err(), Some(Skipped::UnsupportedSchema));
        let after_tools = serde_json::to_value(&req.tools).unwrap();
        assert_eq!(after_tools, before_tools, "native tools must remain");
        assert!(
            after_tools
                .pointer("/0/function/parameters/anyOf")
                .is_some(),
            "must not partially rewrite unsupported schema"
        );
        assert!(
            !req.messages
                .iter()
                .any(|m| m.role == "system" && m.text().is_some_and(|t| t.contains("CALL <<"))),
            "must not inject compact instructions on unsupported bypass"
        );
    }

    #[test]
    fn decode_chat_response_converts_markers_to_native_tool_calls() {
        let tools = vec![CompactToolDef {
            name: "ping".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"host": {"type": "string"}},
                "required": ["host"]
            })),
        }];
        let mut original_by_alias = HashMap::new();
        original_by_alias.insert("ping".into(), "ping".into());
        let applied = Applied {
            tools,
            original_by_alias,
            require_call: true,
        };
        let mut resp = ChatResponse {
            id: "chatcmpl-test".into(),
            object: "chat.completion".into(),
            created: None,
            model: "gpt-4o-mini".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(Value::String(r#"<<ping {"host":"example.com"}>>"#.into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        };
        decode_chat_response(&mut resp, &applied);
        let msg = &resp.choices[0].message;
        assert!(msg.content.is_none());
        let tcs = msg.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "ping");
        assert_eq!(tcs[0].function.arguments, r#"{"host":"example.com"}"#);
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn decode_chat_response_plain_text_unchanged() {
        let mut original_by_alias = HashMap::new();
        original_by_alias.insert("ping".into(), "ping".into());
        let applied = Applied {
            tools: vec![CompactToolDef {
                name: "ping".into(),
                description: None,
                parameters: Some(json!({"type":"object","properties":{}})),
            }],
            original_by_alias,
            require_call: false,
        };
        let mut resp = assistant_text_response("hello");
        decode_chat_response(&mut resp, &applied);
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("hello".into()))
        );
        assert!(resp.choices[0].message.tool_calls.is_none());
    }

    // ── End-to-end hardening: required + compact request/response path ─────────

    fn calendar_like_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "ask_human".into(),
                    description: Some(
                        "Ask the human user a clarifying question whenever a value you need \
                         identifies a specific real-world thing."
                            .into(),
                    ),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "question": {"type": "string", "description": "The exact question to show the user."},
                            "header": {"type": "string"},
                            "multi_select": {"type": "boolean"},
                            "allow_custom_input": {"type": "boolean"},
                        },
                        "required": ["question"]
                    })),
                },
                extra: Default::default(),
            },
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "finish_task".into(),
                    description: Some("Give the user your final message and end this turn.".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "message": {"type": "string", "description": "The final message to show the user."}
                        },
                        "required": ["message"]
                    })),
                },
                extra: Default::default(),
            },
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "create_calendar_event".into(),
                    description: Some("Create an event in the user's calendar.".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "start": {"type": "string", "format": "date-time"},
                            "visibility": {"type": "string", "enum": ["public", "private"]}
                        },
                        "required": ["title", "start"]
                    })),
                },
                extra: Default::default(),
            },
        ]
    }

    fn agent_shaped_req() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![
                Message {
                    role: "system".into(),
                    content: Some(Value::String("You are Nasiko's calendar assistant.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                Message {
                    role: "user".into(),
                    content: Some(Value::String("Book a design review Monday 3pm IST".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
            ],
            tools: Some(calendar_like_tools()),
            tool_choice: Some(json!("required")),
            temperature: Some(0.0),
            max_tokens: None,
            stream: Some(false),
            extra: Default::default(),
        }
    }

    fn cfg_on() -> GatewayConfig {
        GatewayConfig {
            tool_compact_enabled: true,
            ..Default::default()
        }
    }

    fn assistant_text_response(text: &str) -> ChatResponse {
        ChatResponse {
            id: "chatcmpl-e2e".into(),
            object: "chat.completion".into(),
            created: None,
            model: "gpt-4o-mini".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(Value::String(text.into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        }
    }

    fn apply_required_agent() -> Applied {
        let mut req = agent_shaped_req();
        apply(&mut req, &cfg_on()).expect("agent-shaped required request must compact")
    }

    /// Full path: apply(required) → simulated provider content → decode → native tool_calls.
    #[test]
    fn e2e_required_successful_tool_call() {
        let mut req = agent_shaped_req();
        let before_tools = req.tools.clone();
        let applied = apply(&mut req, &cfg_on()).unwrap();
        assert!(applied.require_call);
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        assert!(before_tools.is_some());

        let marker = r#"<<create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let mut resp = assistant_text_response(marker);
        decode_chat_response(&mut resp, &applied);

        let msg = &resp.choices[0].message;
        assert!(
            msg.content.is_none(),
            "compact markers must not remain in content"
        );
        let tcs = msg.tool_calls.as_ref().expect("native tool_calls");
        assert_eq!(tcs.len(), 1, "no duplicate calls");
        assert_eq!(tcs[0].kind, "function");
        assert_eq!(tcs[0].function.name, "create_calendar_event");
        let args: Value = serde_json::from_str(&tcs[0].function.arguments).unwrap();
        assert_eq!(args["title"], "Design review");
        assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        // Same OpenAI shape a non-compact provider tool_call would have.
        assert!(tcs[0].id.starts_with("call_"));
        assert!(!tcs[0].function.arguments.contains("<<"));
    }

    #[test]
    fn e2e_required_multiple_tool_calls_preserve_order() {
        let applied = apply_required_agent();
        let text = concat!(
            r#"<<ask_human {"question":"Which calendar?"}>>"#,
            "\n",
            r#"<<finish_task {"message":"Done."}>>"#,
        );
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0].function.name, "ask_human");
        assert_eq!(tcs[1].function.name, "finish_task");
        let a0: Value = serde_json::from_str(&tcs[0].function.arguments).unwrap();
        let a1: Value = serde_json::from_str(&tcs[1].function.arguments).unwrap();
        assert_eq!(a0["question"], "Which calendar?");
        assert_eq!(a1["message"], "Done.");
        assert_eq!(tcs[0].id, "call_compact_0");
        assert_eq!(tcs[1].id, "call_compact_1");
    }

    #[test]
    fn e2e_required_normal_text_no_fabricated_call() {
        let applied = apply_required_agent();
        let mut resp = assistant_text_response("I need more information.");
        decode_chat_response(&mut resp, &applied);
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("I need more information.".into()))
        );
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn e2e_required_model_ignores_must_instruction() {
        // Provider no longer enforces tool_choice="required". If the model returns
        // plain text, the router does NOT invent a call — same as normal text:
        // assistant content unchanged, no tool_calls, finish_reason stays "stop".
        let applied = apply_required_agent();
        assert!(applied.require_call);
        let mut resp = assistant_text_response("I cannot perform this operation.");
        decode_chat_response(&mut resp, &applied);
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("I cannot perform this operation.".into()))
        );
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn e2e_unknown_tool_no_native_call() {
        let applied = apply_required_agent();
        let text = r#"<<unknown_tool {"x":"y"}>>"#;
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        assert!(
            resp.choices[0].message.tool_calls.is_none(),
            "must not map unknown tools"
        );
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String(text.into())),
            "fail-closed: leave content unchanged (explicit; not a fabricated call)"
        );
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn e2e_invalid_arguments_fail_closed() {
        let applied = apply_required_agent();
        let cases = [
            // missing required title
            r#"<<create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#,
            // wrong type
            r#"<<create_calendar_event {"title":1,"start":"2026-10-05T15:00:00+05:30"}>>"#,
            // invalid enum
            r#"<<create_calendar_event {"title":"t","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
            // malformed JSON
            r#"<<create_calendar_event {"title":>>"#,
            // unexpected structure (array instead of object)
            r#"<<create_calendar_event ["title"]>>"#,
        ];
        for text in cases {
            let mut resp = assistant_text_response(text);
            decode_chat_response(&mut resp, &applied);
            assert!(
                resp.choices[0].message.tool_calls.is_none(),
                "invalid args must not produce ToolCall; text={text}"
            );
            assert_eq!(
                resp.choices[0].message.content,
                Some(Value::String(text.into())),
                "content left unchanged on fail-closed decode; text={text}"
            );
        }
    }

    #[test]
    fn e2e_gtgt_inside_string_and_escapes() {
        let applied = apply_required_agent();
        let text = r#"<<ask_human {"question":"pick a >> b","header":"say \"hello\""}>>"#;
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        let tcs = resp.choices[0]
            .message
            .tool_calls
            .as_ref()
            .expect(">> inside string must not terminate call");
        assert_eq!(tcs.len(), 1);
        let args: Value = serde_json::from_str(&tcs[0].function.arguments).unwrap();
        assert_eq!(args["question"], "pick a >> b");
        assert_eq!(args["header"], "say \"hello\"");
        assert!(resp.choices[0].message.content.is_none());
    }

    #[test]
    fn e2e_escaped_backslash_in_args() {
        let applied = apply_required_agent();
        let text = r#"<<finish_task {"message":"path C:\\Users\\x"}>>"#;
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        let args: Value = serde_json::from_str(
            &resp.choices[0].message.tool_calls.as_ref().unwrap()[0]
                .function
                .arguments,
        )
        .unwrap();
        assert_eq!(args["message"], "path C:\\Users\\x");
    }

    #[test]
    fn e2e_text_around_call_still_decodes() {
        let applied = apply_required_agent();
        let text = "Sure.\n<<finish_task {\"message\":\"ok\"}>>\nDone.";
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "finish_task");
        // Content cleared once calls are extracted (native tool-call turn shape).
        assert!(resp.choices[0].message.content.is_none());
    }

    #[test]
    fn e2e_incomplete_call_fail_closed() {
        let applied = apply_required_agent();
        let text = r#"<<finish_task {"message":"ok""#; // missing close
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String(text.into()))
        );
    }

    #[test]
    fn e2e_whitespace_and_immediate_json() {
        let applied = apply_required_agent();
        // optional WS after << before name is accepted by decoder
        let text = "<<finish_task {\"message\":\"x\"}>>";
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        assert_eq!(
            resp.choices[0].message.tool_calls.as_ref().unwrap()[0]
                .function
                .name,
            "finish_task"
        );
    }

    #[test]
    fn unsupported_schema_with_required_preserves_native_required() {
        let cfg = cfg_on();
        let mut req = agent_shaped_req();
        req.tools = Some(vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "lookup".into(),
                description: Some("Lookup".into()),
                parameters: Some(json!({
                    "anyOf": [
                        {"type": "string"},
                        {"type": "integer"}
                    ]
                })),
            },
            extra: Default::default(),
        }]);
        req.tool_choice = Some(json!("required"));
        let before = serde_json::to_value(&req).unwrap();
        let outcome = apply(&mut req, &cfg);
        assert_eq!(outcome.err(), Some(Skipped::UnsupportedSchema));
        assert_eq!(req.tool_choice, Some(json!("required")));
        assert!(req.tools.is_some());
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            before,
            "unsupported+required must leave request byte-identical for provider enforcement"
        );
    }

    #[test]
    fn disabled_required_is_byte_identical() {
        let cfg = GatewayConfig {
            tool_compact_enabled: false,
            ..Default::default()
        };
        let original = agent_shaped_req();
        let mut req = original.clone();
        assert!(matches!(apply(&mut req, &cfg), Err(Skipped::Disabled)));
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            serde_json::to_string(&original).unwrap()
        );
        assert_eq!(req.tool_choice, Some(json!("required")));
        assert_eq!(req.stream, Some(false));
    }

    #[test]
    fn specific_function_force_still_bypasses_with_agent_tools() {
        let mut req = agent_shaped_req();
        req.tool_choice = Some(json!({
            "type": "function",
            "function": {"name": "finish_task"}
        }));
        let before = serde_json::to_value(&req).unwrap();
        assert_eq!(
            apply(&mut req, &cfg_on()).err(),
            Some(Skipped::ForcedToolChoice)
        );
        assert_eq!(serde_json::to_value(&req).unwrap(), before);
        assert!(req.tools.as_ref().unwrap().len() >= 2);
    }

    #[test]
    fn required_streaming_compacts_with_agent_tools() {
        let mut req = agent_shaped_req();
        req.stream = Some(true);
        let applied = apply(&mut req, &cfg_on()).expect("required+stream agent tools compact");
        assert!(applied.require_call);
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
    }

    // ── Gateway UUID-prefix name mapping ─────────────────────────────────────

    fn gateway_calendar_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "1234567890abcdef__create_calendar_event".into(),
                    description: Some("Create an event in the user's calendar.".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "start": {"type": "string", "format": "date-time"}
                        },
                        "required": ["title", "start"]
                    })),
                },
                extra: Default::default(),
            },
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "1234567890abcdef__list_events".into(),
                    description: Some("List events.".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "date": {"type": "string"}
                        },
                        "required": ["date"]
                    })),
                },
                extra: Default::default(),
            },
        ]
    }

    #[test]
    fn gateway_names_compact_with_suffix_aliases() {
        let mut req = sample_req();
        req.tools = Some(gateway_calendar_tools());
        let applied = apply(&mut req, &cfg_on()).unwrap();
        assert_eq!(applied.tools[0].name, "create_calendar_event");
        assert_eq!(applied.tools[1].name, "list_events");
        assert_eq!(
            applied
                .original_by_alias
                .get("create_calendar_event")
                .map(String::as_str),
            Some("1234567890abcdef__create_calendar_event")
        );
        assert_eq!(
            applied
                .original_by_alias
                .get("list_events")
                .map(String::as_str),
            Some("1234567890abcdef__list_events")
        );
        let sys = req
            .messages
            .iter()
            .find(|m| m.role == "system")
            .unwrap()
            .text()
            .unwrap();
        assert!(sys.contains("create_calendar_event("));
        assert!(!sys.contains("1234567890abcdef__"));
    }

    #[test]
    fn gateway_alias_roundtrip_restores_original_tool_name() {
        let mut req = sample_req();
        req.tools = Some(gateway_calendar_tools());
        let applied = apply(&mut req, &cfg_on()).unwrap();

        let marker =
            r#"<<create_calendar_event {"title":"Meeting","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let mut resp = assistant_text_response(marker);
        decode_chat_response(&mut resp, &applied);

        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(
            tcs[0].function.name,
            "1234567890abcdef__create_calendar_event"
        );
        let args: Value = serde_json::from_str(&tcs[0].function.arguments).unwrap();
        assert_eq!(args["title"], "Meeting");
    }

    #[test]
    fn gateway_suffix_collision_uses_distinct_aliases() {
        let mut req = sample_req();
        req.tools = Some(vec![
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "1111111111111111__search".into(),
                    description: None,
                    parameters: Some(json!({"type":"object","properties":{}})),
                },
                extra: Default::default(),
            },
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "2222222222222222__search".into(),
                    description: None,
                    parameters: Some(json!({"type":"object","properties":{}})),
                },
                extra: Default::default(),
            },
        ]);
        let applied = apply(&mut req, &cfg_on()).unwrap();
        assert_eq!(applied.tools[0].name, "search_1111111111111111");
        assert_eq!(applied.tools[1].name, "search_2222222222222222");
        assert_ne!(applied.tools[0].name, applied.tools[1].name);
        assert!(!applied.original_by_alias.contains_key("search"));

        let mut resp = assistant_text_response(r#"<<search_1111111111111111 {}>>"#);
        decode_chat_response(&mut resp, &applied);
        assert_eq!(
            resp.choices[0].message.tool_calls.as_ref().unwrap()[0]
                .function
                .name,
            "1111111111111111__search"
        );
    }

    #[test]
    fn malformed_gateway_prefix_bypasses() {
        let mut req = sample_req();
        // 8-hex prefix is not the documented 16-hex gateway pattern.
        req.tools = Some(vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "12345678__create_calendar_event".into(),
                description: None,
                parameters: Some(json!({"type":"object","properties":{}})),
            },
            extra: Default::default(),
        }]);
        let before = serde_json::to_value(&req).unwrap();
        assert_eq!(
            apply(&mut req, &cfg_on()).err(),
            Some(Skipped::UnsupportedSchema)
        );
        assert_eq!(serde_json::to_value(&req).unwrap(), before);
    }

    #[test]
    fn digit_leading_non_gateway_bypasses() {
        let mut req = sample_req();
        req.tools = Some(vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "9bad_tool".into(),
                description: None,
                parameters: Some(json!({"type":"object","properties":{}})),
            },
            extra: Default::default(),
        }]);
        let before = serde_json::to_value(&req).unwrap();
        assert_eq!(
            apply(&mut req, &cfg_on()).err(),
            Some(Skipped::UnsupportedSchema)
        );
        assert_eq!(serde_json::to_value(&req).unwrap(), before);
    }

    #[test]
    fn unknown_alias_fails_closed() {
        let mut original_by_alias = HashMap::new();
        original_by_alias.insert("ping".into(), "ping".into());
        let applied = Applied {
            tools: vec![CompactToolDef {
                name: "ping".into(),
                description: None,
                parameters: Some(json!({"type":"object","properties":{}})),
            }],
            original_by_alias,
            require_call: false,
        };
        // Decode would fail on unknown tool against schema; also cover resolve miss
        // by using a tool present in Applied.tools but missing from the reverse map.
        let applied_missing_map = Applied {
            tools: vec![CompactToolDef {
                name: "ghost".into(),
                description: None,
                parameters: Some(json!({"type":"object","properties":{}})),
            }],
            original_by_alias: HashMap::new(),
            require_call: false,
        };
        let mut resp = assistant_text_response(r#"<<ghost {}>>"#);
        decode_chat_response(&mut resp, &applied_missing_map);
        assert!(
            resp.choices[0].message.tool_calls.is_none(),
            "unknown alias must not invent a tool name"
        );
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String(r#"<<ghost {}>>"#.into()))
        );
        let _ = applied; // keep ping fixture available if extended later
    }

    #[test]
    fn multiple_gateway_calls_resolve_independently() {
        let mut req = sample_req();
        req.tools = Some(gateway_calendar_tools());
        let applied = apply(&mut req, &cfg_on()).unwrap();
        let text = concat!(
            r#"<<create_calendar_event {"title":"A","start":"2026-10-04T15:00:00+05:30"}>>"#,
            "\n",
            r#"<<list_events {"date":"2026-10-04"}>>"#,
        );
        let mut resp = assistant_text_response(text);
        decode_chat_response(&mut resp, &applied);
        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(
            tcs[0].function.name,
            "1234567890abcdef__create_calendar_event"
        );
        assert_eq!(tcs[1].function.name, "1234567890abcdef__list_events");
    }

    #[test]
    fn auto_tool_choice_compacts_when_enabled() {
        let mut req = sample_req();
        req.tool_choice = Some(json!("auto"));
        req.tools = Some(gateway_calendar_tools());
        let applied = apply(&mut req, &cfg_on()).unwrap();
        assert!(!applied.require_call);
        assert!(req.tools.is_none());
    }

    #[test]
    fn required_streaming_compacts_with_gateway_names() {
        let mut req = sample_req();
        req.tools = Some(gateway_calendar_tools());
        req.tool_choice = Some(json!("required"));
        req.stream = Some(true);
        let applied = apply(&mut req, &cfg_on()).unwrap();
        assert!(applied.require_call);
        assert!(req.tools.is_none());
        assert_eq!(
            applied
                .original_by_alias
                .get("create_calendar_event")
                .map(String::as_str),
            Some("1234567890abcdef__create_calendar_event")
        );
    }

    // ── Streaming compact decode ─────────────────────────────────────────────

    fn stream_template() -> ChatChunk {
        ChatChunk {
            id: "chatcmpl-stream".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "gpt-oss".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta::default(),
                finish_reason: None,
            }],
            usage: None,
            extra: Default::default(),
        }
    }

    fn content_chunk(text: &str) -> ChatChunk {
        let mut c = stream_template();
        c.choices[0].delta.content = Some(text.into());
        c
    }

    fn collect_tool_names(chunks: &[ChatChunk]) -> Vec<String> {
        let mut names = Vec::new();
        for ch in chunks {
            for choice in &ch.choices {
                if let Some(tcs) = &choice.delta.tool_calls {
                    for tc in tcs {
                        if let Some(name) = tc.function.as_ref().and_then(|f| f.name.clone()) {
                            names.push(name);
                        }
                    }
                }
            }
        }
        names
    }

    fn run_stream(session: &mut CompactStreamSession, pieces: &[&str]) -> Vec<ChatChunk> {
        let mut out = Vec::new();
        for p in pieces {
            out.extend(session.push_chunk(content_chunk(p)));
        }
        out.extend(session.finish(&stream_template()));
        out
    }

    fn calendar_applied() -> Applied {
        let mut req = sample_req();
        req.tools = Some(vec![
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "1234567890abcdef__calendar_create_event".into(),
                    description: None,
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "start_time": {"type": "string"},
                            "message": {"type": "string"},
                            "path": {"type": "string"}
                        },
                        "required": ["title"]
                    })),
                },
                extra: Default::default(),
            },
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "1234567890abcdef__calendar_list_events".into(),
                    description: None,
                    parameters: Some(json!({
                        "type": "object",
                        "properties": { "date": {"type": "string"} },
                        "required": ["date"]
                    })),
                },
                extra: Default::default(),
            },
        ]);
        apply(&mut req, &cfg_on()).unwrap()
    }

    #[test]
    fn stream_single_call_split_name() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(
            &mut session,
            &[
                "<<calendar_",
                "create_event {\"title\":\"Team",
                " Meeting\"}>>",
            ],
        );
        let names = collect_tool_names(&out);
        assert_eq!(names, vec!["1234567890abcdef__calendar_create_event"]);
        assert!(out.iter().all(|c| {
            c.choices
                .iter()
                .all(|ch| ch.delta.content.as_ref().is_none_or(|t| !t.contains("<<")))
        }));
    }

    #[test]
    fn stream_marker_split_open() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(
            &mut session,
            &["<<", "calendar_create_event {\"title\":\"T\"}", ">>"],
        );
        assert_eq!(
            collect_tool_names(&out),
            vec!["1234567890abcdef__calendar_create_event"]
        );
    }

    #[test]
    fn stream_json_split() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(
            &mut session,
            &[
                "<<calendar_create_event {\"ti",
                "tle\":\"Team Meeting\",\"start_",
                "time\":\"2026-10-04T15:00:00\"}>>",
            ],
        );
        assert_eq!(collect_tool_names(&out).len(), 1);
        let args = out
            .iter()
            .find_map(|c| {
                c.choices.iter().find_map(|ch| {
                    ch.delta
                        .tool_calls
                        .as_ref()
                        .and_then(|t| t[0].function.as_ref().and_then(|f| f.arguments.clone()))
                })
            })
            .unwrap();
        let v: Value = serde_json::from_str(&args).unwrap();
        assert_eq!(v["title"], "Team Meeting");
        assert_eq!(v["start_time"], "2026-10-04T15:00:00");
    }

    #[test]
    fn stream_gtgt_inside_string() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(
            &mut session,
            &[r#"<<calendar_create_event {"title":"t","message":"Use >> carefully"}>>"#],
        );
        assert_eq!(collect_tool_names(&out).len(), 1);
    }

    #[test]
    fn stream_escaped_quotes() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(
            &mut session,
            &[r#"<<calendar_create_event {"title":"t","message":"He said \"hello\""}>>"#],
        );
        assert_eq!(collect_tool_names(&out).len(), 1);
    }

    #[test]
    fn stream_backslashes() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(
            &mut session,
            &[r#"<<calendar_create_event {"title":"t","path":"C:\\temp\\file.txt"}>>"#],
        );
        assert_eq!(collect_tool_names(&out).len(), 1);
    }

    #[test]
    fn stream_multiple_calls_order() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(
            &mut session,
            &[concat!(
                r#"<<calendar_create_event {"title":"A"}>>"#,
                "\n",
                r#"<<calendar_list_events {"date":"2026-10-04"}>>"#
            )],
        );
        assert_eq!(
            collect_tool_names(&out),
            vec![
                "1234567890abcdef__calendar_create_event",
                "1234567890abcdef__calendar_list_events"
            ]
        );
    }

    #[test]
    fn stream_normal_text_no_tool_call() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(&mut session, &["I cannot perform that operation."]);
        assert!(collect_tool_names(&out).is_empty());
        let content: String = out
            .iter()
            .flat_map(|c| c.choices.iter())
            .filter_map(|ch| ch.delta.content.clone())
            .collect();
        assert!(content.contains("I cannot perform"));
    }

    #[test]
    fn stream_unknown_alias_no_tool_call() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(&mut session, &[r#"<<does_not_exist {"title":"x"}>>"#]);
        assert!(collect_tool_names(&out).is_empty());
    }

    #[test]
    fn stream_invalid_arguments_no_tool_call() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        // missing required title
        let out = run_stream(
            &mut session,
            &[r#"<<calendar_create_event {"start_time":"x"}>>"#],
        );
        assert!(collect_tool_names(&out).is_empty());
    }

    #[test]
    fn stream_incomplete_no_fabricated_call() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let out = run_stream(&mut session, &["<<calendar_create_event {\"title\":\"x\""]);
        assert!(collect_tool_names(&out).is_empty());
    }

    #[test]
    fn stream_unicode_prose_no_panic_no_tools() {
        let applied = calendar_applied();
        let mut session = CompactStreamSession::new(&applied);
        let prose = "Hello 👋 世界 café — long prose without markers.";
        let out = run_stream(&mut session, &[prose]);
        assert!(collect_tool_names(&out).is_empty());
        let content: String = out
            .iter()
            .flat_map(|c| c.choices.iter())
            .filter_map(|ch| ch.delta.content.clone())
            .collect();
        assert!(content.contains("👋"));
        assert!(content.contains("世界"));
    }
}
