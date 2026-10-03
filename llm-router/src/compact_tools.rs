//! Opt-in compact tool schemas on the OpenAI non-streaming chat path.
//!
//! Off unless [`crate::GatewayConfig::compact_tools`] is set. Covered when the
//! resolved provider is `openai`, the request is not streaming, every tool is a
//! function, the schema encodes, `tool_choice` is absent or `"auto"`, and the
//! transcript has no prior tool call or tool result. Every other request is left
//! unchanged. A model reply that names an unknown tool or breaks the schema is
//! [`GatewayError::BadRequest`] — never a guessed call.
//!
//! Not covered: streaming, Anthropic, Gemini, forced `tool_choice`, tool history.

use serde_json::{Map, Value};

use crate::error::GatewayError;
use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};

pub(crate) struct Prepared {
    tools: Vec<nasiko_tool_compact::ToolDef>,
}

/// Provider `openai` and not streaming. The flag itself is the third input.
pub(crate) fn eligible(enabled: bool, provider: &str, streaming: bool) -> bool {
    enabled && provider == "openai" && !streaming
}

/// Inject compact tool text and drop native `tools`. `None` means `req` was not touched.
pub(crate) fn prepare(req: &mut ChatRequest, enabled: bool) -> Option<Prepared> {
    if !enabled || req.is_streaming() {
        return None;
    }
    let tools = req.tools.as_ref().filter(|t| !t.is_empty())?;
    if tools.iter().any(|t| t.kind != "function") || tool_choice_blocks(req.tool_choice.as_ref()) {
        return None;
    }
    if has_tool_history(&req.messages) {
        return None;
    }
    let compact_tools: Vec<_> = tools.iter().map(to_compact).collect();
    let encoded = nasiko_tool_compact::encode_tools(&compact_tools).ok()?;
    req.messages.push(Message {
        role: "system".into(),
        content: Some(Value::String(encoded.text)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Map::new(),
    });
    req.tools = None;
    req.tool_choice = None;
    Some(Prepared {
        tools: compact_tools,
    })
}

/// Turn compact calls in assistant text into OpenAI `tool_calls`. No call is a no-op.
pub(crate) fn decode_response(
    resp: &mut ChatResponse,
    prepared: &Prepared,
) -> Result<(), GatewayError> {
    let mut updates = Vec::new();
    for (i, choice) in resp.choices.iter().enumerate() {
        if choice
            .message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty())
        {
            continue;
        }
        let Some(Value::String(text)) = &choice.message.content else {
            continue;
        };
        let calls = nasiko_tool_compact::decode_calls(text, &prepared.tools)
            .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
        if !calls.is_empty() {
            updates.push((i, calls));
        }
    }
    let mut n = 0usize;
    for (i, calls) in updates {
        let tool_calls = calls
            .into_iter()
            .map(|call| {
                let id = format!("call_{n}");
                n += 1;
                ToolCall {
                    id,
                    kind: "function".into(),
                    function: FunctionCall {
                        name: call.name,
                        arguments: call.arguments,
                    },
                    extra: Map::new(),
                }
            })
            .collect();
        let choice = &mut resp.choices[i];
        choice.message.tool_calls = Some(tool_calls);
        if matches!(choice.finish_reason.as_deref(), None | Some("stop")) {
            choice.finish_reason = Some("tool_calls".into());
        }
    }
    Ok(())
}

fn to_compact(tool: &crate::ir::ToolDef) -> nasiko_tool_compact::ToolDef {
    nasiko_tool_compact::ToolDef {
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        parameters: tool.function.parameters.clone(),
    }
}

fn tool_choice_blocks(choice: Option<&Value>) -> bool {
    match choice {
        None => false,
        Some(Value::String(s)) => s != "auto",
        Some(_) => true,
    }
}

fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|m| {
        m.role == "tool"
            || m.tool_call_id.is_some()
            || m.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_req(body: Value) -> ChatRequest {
        serde_json::from_value(body).expect("request")
    }

    fn ping_req(extra: Value) -> ChatRequest {
        let mut body = json!({
            "messages": [{"role": "user", "content": "ping"}],
            "tools": [{
                "type": "function",
                "function": {"name": "ping", "description": "Say ping."}
            }]
        });
        if let (Some(map), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                map.insert(k.clone(), v.clone());
            }
        }
        parse_req(body)
    }

    fn bytes(req: &ChatRequest) -> String {
        serde_json::to_string(req).expect("json")
    }

    #[test]
    fn flag_off_is_default_and_leaves_bytes() {
        assert!(!crate::GatewayConfig::default().compact_tools);
        assert!(!eligible(false, "openai", false));
        let mut req = ping_req(json!(null));
        let before = bytes(&req);
        assert!(prepare(&mut req, false).is_none());
        assert_eq!(bytes(&req), before);
    }

    #[test]
    fn other_providers_and_streams_are_not_eligible() {
        assert!(!eligible(true, "anthropic", false));
        assert!(!eligible(true, "gemini", false));
        assert!(!eligible(true, "openai", true));
        assert!(eligible(true, "openai", false));
    }

    #[test]
    fn enabled_strips_tools_and_appends_grammar() {
        let mut req = ping_req(json!({"tool_choice": "auto"}));
        let prepared = prepare(&mut req, true).expect("compacted");
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        let text = req.messages.last().expect("system").text().expect("text");
        assert!(text.contains("ping"));
        assert!(text.contains("<<call"));
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id": "c",
            "model": "m",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "<<call ping {}>>"},
                "finish_reason": "stop"
            }]
        }))
        .expect("response");
        decode_response(&mut resp, &prepared).expect("decoded");
        let call = &resp.choices[0].message.tool_calls.as_ref().expect("calls")[0];
        assert_eq!(call.id, "call_0");
        assert_eq!(call.function.name, "ping");
        assert_eq!(call.function.arguments, "{}");
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn plain_answer_adds_no_call() {
        let mut req = ping_req(json!(null));
        let prepared = prepare(&mut req, true).expect("compacted");
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id": "c",
            "model": "m",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }]
        }))
        .expect("response");
        decode_response(&mut resp, &prepared).expect("decoded");
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn unknown_tool_is_an_error() {
        let mut req = ping_req(json!(null));
        let prepared = prepare(&mut req, true).expect("compacted");
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id": "c",
            "model": "m",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "<<call other {}>>"},
                "finish_reason": "stop"
            }]
        }))
        .expect("response");
        let err = decode_response(&mut resp, &prepared).expect_err("fail closed");
        assert_eq!(err.to_string(), "unknown_tool");
        assert!(resp.choices[0].message.tool_calls.is_none());
    }

    #[test]
    fn unrepresentable_additional_properties_keeps_native_tool() {
        let mut req = parse_req(json!({
            "messages": [{"role": "user", "content": "ping"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "bag",
                    "description": "Hold items.",
                    "parameters": {
                        "type": "object",
                        "properties": {"label": {"type": "string"}},
                        "required": ["label"],
                        "additionalProperties": {"type": "string", "pattern": "^x"}
                    }
                }
            }]
        }));
        let before = bytes(&req);
        assert!(prepare(&mut req, true).is_none());
        assert_eq!(bytes(&req), before);
        assert!(before.contains("\"pattern\":\"^x\""));
    }

    #[test]
    fn unsupported_combinators_keep_native_tools() {
        let schemas = [
            json!({"type": "object", "properties": {"a": {"$ref": "#/defs/A"}}, "required": ["a"]}),
            json!({"type": "object", "properties": {"a": {"oneOf": [{"type": "string"}]}}}),
            json!({"type": "object", "properties": {"a": {"anyOf": [{"type": "string"}]}}}),
            json!({"type": "object", "properties": {"a": {"allOf": [{"type": "string"}]}}}),
        ];
        for schema in schemas {
            let mut req = parse_req(json!({
                "messages": [{"role": "user", "content": "ping"}],
                "tools": [{
                    "type": "function",
                    "function": {"name": "lookup", "description": "Look up.", "parameters": schema}
                }]
            }));
            let before = bytes(&req);
            assert!(prepare(&mut req, true).is_none());
            assert_eq!(bytes(&req), before);
            assert_eq!(req.tools.as_ref().expect("tools").len(), 1);
        }
    }

    #[test]
    fn bypass_leaves_bytes() {
        let cases = [
            ping_req(json!({"stream": true})),
            ping_req(json!({"tool_choice": {"type": "function", "function": {"name": "ping"}}})),
            parse_req(json!({
                "messages": [
                    {"role": "user", "content": "ping"},
                    {"role": "tool", "tool_call_id": "call_0", "content": "ok"}
                ],
                "tools": [{"type": "function", "function": {"name": "ping"}}]
            })),
            parse_req(json!({
                "messages": [{"role": "user", "content": "ping"}],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "ping",
                        "parameters": {
                            "type": "object",
                            "properties": {"a": {"type": "string", "pattern": "^x$"}}
                        }
                    }
                }]
            })),
        ];
        for mut req in cases {
            let before = bytes(&req);
            assert!(prepare(&mut req, true).is_none());
            assert_eq!(bytes(&req), before);
        }
    }
}
