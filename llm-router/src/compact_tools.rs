//! Compact tool schema transformation at the router boundary (IP-3).
//!
//! Opt-in feature that compacts verbose JSON Schema tool definitions into lean,
//! token-efficient signatures injected into the prompt. When disabled (the default),
//! request and response payloads pass through completely untouched and byte-identical.

use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};

/// Convert router IR `ToolDef` into `nasiko_tool_compact::ToolDef`.
pub fn to_compact_tool_def(tool: &ToolDef) -> nasiko_tool_compact::ToolDef {
    nasiko_tool_compact::ToolDef {
        kind: tool.kind.clone(),
        function: nasiko_tool_compact::FunctionDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        },
        extra: tool.extra.clone(),
    }
}

/// Convert `nasiko_tool_compact::ToolCall` back into router IR `ToolCall`.
pub fn from_compact_tool_call(call: &nasiko_tool_compact::ToolCall) -> ToolCall {
    ToolCall {
        id: call.id.clone(),
        kind: call.kind.clone(),
        function: FunctionCall {
            name: call.function.name.clone(),
            arguments: call.function.arguments.clone(),
        },
        extra: call.extra.clone(),
    }
}

/// Apply compact tools transformation to a `ChatRequest` if enabled.
/// Returns the original tools if compaction was applied, or `None` if skipped/disabled.
pub fn apply_compact_request(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
) -> Option<Vec<ToolDef>> {
    if !cfg.compact_tools_enabled {
        return None;
    }

    // Do not compact if no tools are present
    let tools = match &req.tools {
        Some(t) if !t.is_empty() => t,
        _ => return None,
    };

    // If forced tool_choice is specified, bypass compaction for reliability
    if let Some(choice) = &req.tool_choice
        && choice.is_object() {
            return None;
        }

    let compact_defs: Vec<nasiko_tool_compact::ToolDef> =
        tools.iter().map(to_compact_tool_def).collect();

    let compact_tools = match nasiko_tool_compact::encode_tools(&compact_defs) {
        Ok(ct) => ct,
        Err(_) => return None,
    };

    let original_tools = req.tools.take()?;

    // Inject compact definitions into a system message
    let instruction = compact_tools.prompt;
    if let Some(sys_msg) = req.messages.iter_mut().find(|m| m.role == "system") {
        let existing = sys_msg.text().unwrap_or_default();
        let combined = if existing.is_empty() {
            instruction
        } else {
            format!("{}\n\n{}", instruction, existing)
        };
        sys_msg.content = Some(Value::String(combined));
    } else {
        req.messages.insert(
            0,
            Message {
                role: "system".into(),
                content: Some(Value::String(instruction)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
        );
    }

    Some(original_tools)
}

/// Decode compact tool calls from a `ChatResponse` back into standard OpenAI `ToolCall`s.
pub fn decompact_response(resp: &mut ChatResponse, original_tools: &[ToolDef]) {
    let compact_defs: Vec<nasiko_tool_compact::ToolDef> =
        original_tools.iter().map(to_compact_tool_def).collect();

    for choice in &mut resp.choices {
        let content_text = match choice.message.text() {
            Some(t) if !t.is_empty() => t,
            _ => continue,
        };

        if let Ok(decoded) = nasiko_tool_compact::decode_calls(&content_text, &compact_defs)
            && !decoded.is_empty() {
                let ir_calls: Vec<ToolCall> = decoded.iter().map(from_compact_tool_call).collect();
                choice.message.tool_calls = Some(ir_calls);
                choice.finish_reason = Some("tool_calls".into());
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionDef;
    use serde_json::json;

    fn sample_request() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("Book a meeting".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            }],
            tools: Some(vec![ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "book_meeting".into(),
                    description: Some("Book a meeting".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "time": {"type": "string"}
                        },
                        "required": ["time"]
                    })),
                },
                extra: serde_json::Map::new(),
            }]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn disabled_compact_tools_leaves_request_byte_identical() {
        let mut cfg = GatewayConfig::default();
        cfg.compact_tools_enabled = false;

        let mut req = sample_request();
        let serialized_before = serde_json::to_string(&req).unwrap();

        let applied = apply_compact_request(&mut req, &cfg);
        assert!(applied.is_none());

        let serialized_after = serde_json::to_string(&req).unwrap();
        assert_eq!(
            serialized_before, serialized_after,
            "request must remain byte-identical when compact tools is disabled"
        );
    }

    #[test]
    fn enabled_compact_tools_transforms_request_and_decompacts_response() {
        let mut cfg = GatewayConfig::default();
        cfg.compact_tools_enabled = true;

        let mut req = sample_request();
        let applied = apply_compact_request(&mut req, &cfg);
        assert!(applied.is_some());
        assert!(req.tools.is_none(), "tools must be removed from request");
        assert_eq!(req.messages[0].role, "system");
        assert!(
            req.messages[0]
                .text()
                .unwrap()
                .contains("book_meeting(time:str)")
        );

        let original_tools = applied.unwrap();

        // Simulate provider returning a compact call in message content
        let mut resp = ChatResponse {
            id: "chatcmpl-test".into(),
            object: "chat.completion".into(),
            created: Some(1234567890),
            model: "gpt-4o-mini".into(),
            choices: vec![crate::ir::Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(Value::String(
                        "<<call book_meeting {\"time\":\"10:00\"}>>".into(),
                    )),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: serde_json::Map::new(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: serde_json::Map::new(),
        };

        decompact_response(&mut resp, &original_tools);
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "book_meeting");
        assert_eq!(calls[0].function.arguments, "{\"time\":\"10:00\"}");
    }
}
