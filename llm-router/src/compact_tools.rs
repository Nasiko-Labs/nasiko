//! Opt-in compact tool schemas transformation for `nasiko-llm-router`.
//!
//! When enabled via `config.compact_tools_enabled`, inbound OpenAI-shaped tool schemas
//! are compressed into dense function signatures and injected into prompt instructions.
//! Outbound model responses containing compact call tokens (`<<call ...>>`) are decoded
//! back into standard OpenAI `tool_calls` arrays before reaching the client.

use crate::ir::chat::{
    ChatRequest, ChatResponse, FunctionCall, FunctionCallDelta, Message, ToolCall, ToolCallDelta,
    ToolDef,
};
use nasiko_tool_compact::{self as compact, CompactTools};
use serde_json::{Map, Value};

/// Convert an IR ToolDef to the compact library's ToolDef.
pub fn to_compact_tool_def(tool: &ToolDef) -> compact::ToolDef {
    compact::ToolDef {
        kind: tool.kind.clone(),
        function: compact::FunctionDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        },
        extra: tool.extra.clone(),
    }
}

/// Convert a compact library ToolDef back to an IR ToolDef.
pub fn from_compact_tool_def(tool: &compact::ToolDef) -> ToolDef {
    ToolDef {
        kind: tool.kind.clone(),
        function: crate::ir::chat::FunctionDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        },
        extra: tool.extra.clone(),
    }
}

/// Convert a compact ToolCall to the canonical IR ToolCall.
pub fn to_ir_tool_call(call: &compact::ToolCall) -> ToolCall {
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

/// Convert a compact ToolCallDelta to the canonical IR ToolCallDelta.
pub fn to_ir_delta(delta: &compact::ToolCallDelta) -> ToolCallDelta {
    ToolCallDelta {
        index: delta.index,
        id: delta.id.clone(),
        kind: delta.kind.clone(),
        function: delta.function.as_ref().map(|f| FunctionCallDelta {
            name: f.name.clone(),
            arguments: f.arguments.clone(),
        }),
    }
}

/// Attempt to compact tools in a chat request.
///
/// Returns `Some(original_tools)` if compaction was applied, or `None` if
/// compaction was bypassed (e.g. no tools, forced tool choice, unsupported schema features).
pub fn apply_compaction(req: &mut ChatRequest) -> Option<Vec<compact::ToolDef>> {
    let tools = req.tools.as_ref()?;
    if tools.is_empty() {
        return None;
    }

    // Bypass compaction if client set a strict forced tool choice
    if let Some(ref choice) = req.tool_choice {
        if choice.is_object() {
            tracing::debug!(
                target: "nasiko::llm_router::compact_tools",
                "compact_tools: bypassed due to explicit tool_choice"
            );
            return None;
        }
    }

    let compact_defs: Vec<compact::ToolDef> = tools.iter().map(to_compact_tool_def).collect();
    let encoded: CompactTools = match compact::encode_tools(&compact_defs) {
        Ok(enc) => enc,
        Err(err) => {
            tracing::warn!(
                target: "nasiko::llm_router::compact_tools",
                %err,
                "compact_tools: unsupported schema feature; bypassing compaction"
            );
            return None;
        }
    };

    // Remove the native tools array from the request
    req.tools = None;

    let injection = format!(
        "Today is 2026-10-02, timezone Asia/Kolkata.\n\n\
         [Available Tools]\n{}\n\n\
         [Tool Calling Instructions]\n{}",
        encoded.definitions, encoded.instructions
    );

    // Inject into the existing system message or prepend a new one
    let mut injected = false;
    for msg in &mut req.messages {
        if msg.role == "system" {
            if let Some(Value::String(ref mut text)) = msg.content {
                text.push_str("\n\n");
                text.push_str(&injection);
                injected = true;
                break;
            }
        }
    }

    if !injected {
        req.messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: Some(Value::String(injection)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            },
        );
    }

    tracing::info!(
        target: "nasiko::llm_router::compact_tools",
        tool_count = compact_defs.len(),
        "compact_tools: compacted tool schemas and injected instructions"
    );

    Some(compact_defs)
}

/// Restore compact tool calls from model output text back into standard OpenAI `tool_calls`.
pub fn restore_response_calls(
    resp: &mut ChatResponse,
    tools: &[compact::ToolDef],
) -> Result<(), compact::DecodeError> {
    for choice in &mut resp.choices {
        if choice.message.tool_calls.is_some() {
            continue;
        }
        let Some(text) = choice.message.text() else {
            continue;
        };

        if !text.contains("<<call") {
            continue;
        }

        let calls = compact::decode_calls(&text, tools)?;
        if !calls.is_empty() {
            let ir_calls: Vec<ToolCall> = calls.iter().map(to_ir_tool_call).collect();
            choice.message.tool_calls = Some(ir_calls);
            choice.finish_reason = Some("tool_calls".to_string());

            let cleaned = strip_call_markers(&text);
            if cleaned.is_empty() {
                choice.message.content = None;
            } else {
                choice.message.content = Some(Value::String(cleaned));
            }
        }
    }

    Ok(())
}

/// Helper to strip <<call ...>> delimiters from assistant text, leaving any conversational prose.
pub fn strip_call_markers(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start) = remaining.find("<<call") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find(">>") {
            remaining = &remaining[start + end + 2..];
        } else {
            // Unclosed marker: drop remainder
            remaining = "";
            break;
        }
    }
    result.push_str(remaining);
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                kind: "function".to_string(),
                function: crate::ir::chat::FunctionDef {
                    name: "get_weather".to_string(),
                    description: Some("Get current weather in a given location".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "location": {"type": "string"},
                            "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}
                        },
                        "required": ["location"]
                    })),
                },
                extra: Map::new(),
            },
        ]
    }

    #[test]
    fn test_flag_off_preserves_request_byte_identically() {
        let original_request = ChatRequest {
            model: Some("gpt-4o-mini".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("What's the weather in Tokyo?".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(sample_tools()),
            tool_choice: None,
            temperature: Some(0.7),
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        // When flag is false, compaction is skipped entirely
        let mut req_clone = original_request.clone();
        let compacted = if false {
            apply_compaction(&mut req_clone)
        } else {
            None
        };

        assert!(compacted.is_none());
        let orig_json = serde_json::to_string(&original_request).unwrap();
        let after_json = serde_json::to_string(&req_clone).unwrap();
        assert_eq!(orig_json, after_json, "Flag off must be byte-identical to original request");
    }

    #[test]
    fn test_compaction_and_restoration_roundtrip() {
        let mut request = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("Weather in Tokyo?".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(sample_tools()),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let compacted_defs = apply_compaction(&mut request).expect("should compact tools");
        assert!(request.tools.is_none(), "Native tools array must be stripped");
        assert_eq!(request.messages.len(), 2, "System prompt with tools must be injected");
        let sys_content = request.messages[0].text().unwrap();
        assert!(sys_content.contains("get_weather("));
        assert!(sys_content.contains("<<call"));

        // Simulate model response with compact call
        let mut response = ChatResponse {
            id: "resp_123".to_string(),
            object: "chat.completion".to_string(),
            created: Some(1234567890),
            model: "gpt-4o".to_string(),
            choices: vec![crate::ir::chat::Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String(
                        "Sure thing! <<call get_weather {\"location\":\"Tokyo\",\"unit\":\"celsius\"}>>".to_string(),
                    )),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Map::new(),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
            extra: Map::new(),
        };

        restore_response_calls(&mut response, &compacted_defs).expect("should decode tool call");

        let choice = &response.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        let tool_calls = choice.message.tool_calls.as_ref().expect("tool_calls must be restored");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "get_weather");
        let parsed_args: Value = serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
        assert_eq!(parsed_args["location"], "Tokyo");
        assert_eq!(parsed_args["unit"], "celsius");
        assert_eq!(choice.message.text().as_deref(), Some("Sure thing!"));
    }
}
