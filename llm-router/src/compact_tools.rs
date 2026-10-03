//! Opt-in compact tool schemas, at the same egress seam as [`crate::compress`].
//!
//! **Off by default.** Enabled only when `GatewayConfig::compact_tools_enabled` is
//! true. With the flag off this module returns the request/response untouched.
//!
//! Coverage (documented, intentionally partial):
//! * Non-streaming OpenAI-shaped requests only. Streaming requests, and requests
//!   with `tool_choice` set, bypass compaction entirely (behavior identical to
//!   today) because converting streamed text deltas back into structured tool
//!   calls is not implemented yet.
//! * If the tool set uses schema features outside the compact subset
//!   (`anyOf`/`oneOf`/`$ref`, ...), compaction is skipped for that request.
//! * Decoding is fail-closed: an unknown tool or a schema violation leaves the
//!   raw text in place rather than fabricating a call.

use serde_json::Value;

use nasiko_tool_compact::{ToolDef as CompactToolDef, decode_calls, encode_tools};

use crate::config::GatewayConfig;
use crate::ir::chat::{ChatRequest, ChatResponse, Message, ToolCall, ToolDef};

/// Rewrite `req` to carry compact tool definitions instead of native `tools`.
/// Returns `true` when compaction was applied (the caller must keep the original
/// tool defs to decode the response).
pub(crate) fn apply_request(req: &mut ChatRequest, cfg: &GatewayConfig) -> Option<Vec<ToolDef>> {
    if !cfg.compact_tools_enabled || req.is_streaming() || req.tool_choice.is_some() {
        return None;
    }
    let tools = req.tools.as_ref()?;
    if tools.is_empty() {
        return None;
    }
    let compact_defs: Vec<CompactToolDef> = tools
        .iter()
        .filter_map(|t| {
            serde_json::to_value(t)
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
        })
        .collect();
    if compact_defs.len() != tools.len() {
        return None;
    }
    let Ok(compact) = encode_tools(&compact_defs) else {
        return None; // unsupported schema → bypass
    };

    let preserved = tools.clone();
    // Replace native tools with one injected system message. A standalone
    // system message (not edited into the author's) keeps their bytes intact,
    // mirroring the brevity directive's reasoning, and Anthropic/Gemini hoist
    // consecutive system messages into one block anyway.
    let insert_at = req
        .messages
        .iter()
        .take_while(|m| m.role == "system")
        .count();
    req.messages.insert(
        insert_at,
        Message {
            role: "system".into(),
            content: Some(Value::String(compact.text)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    req.tools = None;
    Some(preserved)
}

/// Decode compact call markers in a non-streaming response's text into standard
/// tool calls. On any decode error the raw text is left untouched (fail-closed:
/// never a guessed call).
pub(crate) fn apply_response(resp: &mut ChatResponse, original_tools: &[ToolDef]) {
    let compact_defs: Vec<CompactToolDef> = original_tools
        .iter()
        .filter_map(|t| {
            serde_json::to_value(t)
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
        })
        .collect();
    for choice in &mut resp.choices {
        let Some(text) = choice.message.text() else {
            continue;
        };
        if !text.contains("<<call") {
            continue;
        }
        match decode_calls(&text, &compact_defs) {
            Ok(calls) if !calls.is_empty() => {
                let remaining = strip_calls(&text);
                choice.message.content = if remaining.trim().is_empty() {
                    None
                } else {
                    Some(Value::String(remaining))
                };
                choice.message.tool_calls = Some(
                    calls
                        .into_iter()
                        .enumerate()
                        .map(|(i, c)| ToolCall {
                            id: format!("call_{}", i + 1),
                            kind: "function".into(),
                            function: crate::ir::chat::FunctionCall {
                                name: c.name,
                                arguments: c.arguments.to_string(),
                            },
                            extra: Default::default(),
                        })
                        .collect(),
                );
            }
            _ => {
                // Either no calls in the text or a decode error: leave the
                // provider's text in place so nothing is fabricated or lost.
            }
        }
    }
}

/// Remove complete `<<call ...>>` markers (string-aware `>>`) from text.
fn strip_calls(text: &str) -> String {
    // Reuse the decoder's scanner semantics: feed everything, drop marker spans.
    // Simplest correct approach: walk with a string-aware scan for "<<call".
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("<<call") {
        out.push_str(&rest[..pos]);
        match marker_end(&rest[pos..]) {
            Some(end) => rest = &rest[pos + end..],
            None => {
                out.push_str(&rest[pos..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Length of a complete `<<call ...>>` marker at the start of `s`, where the
/// terminator `>>` is outside a JSON string. `None` when incomplete or malformed.
fn marker_end(s: &str) -> Option<usize> {
    let after = &s["<<call".len()..];
    let rest = after.trim_start();
    let name_end = rest.find(|c: char| c.is_whitespace() || c == '{')?;
    let after_name = rest[name_end..].trim_start();
    if !after_name.starts_with('{') {
        return None;
    }
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in after_name.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    let after_json = after_name[i + 1..].trim_start();
                    let consumed_ws = after_name[i + 1..].len() - after_json.len();
                    return after_json.strip_prefix(">>").map(|_| {
                        "<<call".len()
                            + (after.len() - rest.len())
                            + name_end
                            + (rest[name_end..].len() - after_name.len())
                            + i
                            + 1
                            + consumed_ws
                            + 2
                    });
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: enabled,
            ..Default::default()
        }
    }

    fn req_with_tool() -> ChatRequest {
        serde_json::from_value(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                        "required": ["city"]
                    }
                }
            }]
        }))
        .unwrap()
    }

    #[test]
    fn off_by_default_is_byte_identical() {
        let mut req = req_with_tool();
        let before = serde_json::to_string(&req).unwrap();
        assert!(apply_request(&mut req, &cfg(false)).is_none());
        assert_eq!(serde_json::to_string(&req).unwrap(), before);
    }

    #[test]
    fn on_replaces_tools_with_injected_definitions() {
        let mut req = req_with_tool();
        let preserved = apply_request(&mut req, &cfg(true)).expect("applied");
        assert_eq!(preserved.len(), 1);
        assert!(req.tools.is_none());
        let sys = req.messages[0].text().unwrap();
        assert!(sys.contains("get_weather"));
        assert!(sys.contains("<<call name {json args}>>"));
    }

    #[test]
    fn response_decoding_roundtrips_calls() {
        let req = req_with_tool();
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id": "r1", "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "<<call get_weather {\"city\":\"Oslo\"}>>"}}]
        })).unwrap();
        apply_response(&mut resp, req.tools.as_ref().unwrap());
        let msg = &resp.choices[0].message;
        let calls = msg.tool_calls.as_ref().unwrap();
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(calls[0].function.arguments, "{\"city\":\"Oslo\"}");
        assert!(msg.content.is_none());
    }

    #[test]
    fn response_with_error_keeps_raw_text() {
        let req = req_with_tool();
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id": "r1", "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "<<call nope {}>>"}}]
        })).unwrap();
        apply_response(&mut resp, req.tools.as_ref().unwrap());
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert!(resp.choices[0].message.text().unwrap().contains("<<call"));
    }

    #[test]
    fn streaming_and_tool_choice_bypass() {
        let mut req = req_with_tool();
        req.stream = Some(true);
        assert!(apply_request(&mut req, &cfg(true)).is_none());
        let mut req2 = req_with_tool();
        req2.tool_choice = Some(json!("auto"));
        assert!(apply_request(&mut req2, &cfg(true)).is_none());
    }

    #[test]
    fn unsupported_schema_bypasses() {
        let mut req = req_with_tool();
        req.tools = Some(vec![serde_json::from_value(json!({
            "type": "function",
            "function": {"name": "x", "parameters": {"type": "object", "properties": {"a": {"anyOf": []}}}}
        }))
        .unwrap()]);
        assert!(apply_request(&mut req, &cfg(true)).is_none());
    }

    #[test]
    fn strip_calls_preserves_surrounding_text() {
        let t = "here <<call get_weather {\"city\":\"Oslo\"}>> done";
        assert_eq!(strip_calls(t).trim(), "here  done");
        let t2 = "a >> b stays";
        assert_eq!(strip_calls(t2), t2);
    }
}
