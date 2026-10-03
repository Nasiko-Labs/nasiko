//! Opt-in Tool Compaction Integration for the LLM Router.
//!
//! Transforms verbose JSON Schema tool definitions into compact TOON signatures
//! before passing to the upstream LLM, and transparently decodes `<<call ...>>`
//! model emissions back into OpenAI standard `ToolCall` shapes.

use nasiko_tool_compact::{
    decode_calls, encode_tools, FunctionDef as CompactFnDef, ToolCall as CompactToolCall,
    ToolDef as CompactToolDef,
};
use serde_json::Value;

use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};

/// Converts router IR `ToolDef` into `nasiko_tool_compact::ToolDef`.
pub fn to_compact_tool_def(tool: &ToolDef) -> CompactToolDef {
    CompactToolDef {
        kind: tool.kind.clone(),
        function: CompactFnDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        },
        extra: tool.extra.clone(),
    }
}

/// Converts decoded `nasiko_tool_compact::ToolCall` into router IR `ToolCall`.
pub fn from_compact_tool_call(call: CompactToolCall) -> ToolCall {
    ToolCall {
        id: call.id,
        kind: call.kind,
        function: FunctionCall {
            name: call.function.name,
            arguments: call.function.arguments,
        },
        extra: call.extra,
    }
}

/// Compacts a `ChatRequest` in place by moving its tools into the system prompt.
///
/// Returns the list of `CompactToolDef`s if compaction was applied, or `None` if
/// no tools were present or compaction was skipped.
pub fn apply_compaction(req: &mut ChatRequest) -> Option<Vec<CompactToolDef>> {
    let tools = req.tools.take()?;
    if tools.is_empty() {
        return None;
    }

    let compact_defs: Vec<CompactToolDef> = tools.iter().map(to_compact_tool_def).collect();
    let compact = encode_tools(&compact_defs).ok()?;

    let prompt_addition = compact.prompt_block();

    // Inject into existing system message or prepend a new one
    if let Some(sys_msg) = req.messages.iter_mut().find(|m| m.role == "system") {
        if let Some(Value::String(ref mut s)) = sys_msg.content {
            s.push_str("\n\n");
            s.push_str(&prompt_addition);
        } else {
            sys_msg.content = Some(Value::String(prompt_addition));
        }
    } else {
        req.messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: Some(Value::String(prompt_addition)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            },
        );
    }

    Some(compact_defs)
}

/// Restores decoded tool calls from the model's text into standard `ToolCall`s on the response.
pub fn restore_response_tool_calls(resp: &mut ChatResponse, tools: &[CompactToolDef]) {
    for choice in &mut resp.choices {
        if let Some(text) = choice.message.text() {
            if let Ok(calls) = decode_calls(&text, tools) {
                if !calls.is_empty() {
                    let standard_calls: Vec<ToolCall> =
                        calls.into_iter().map(from_compact_tool_call).collect();
                    choice.message.tool_calls = Some(standard_calls);
                    choice.finish_reason = Some("tool_calls".to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::FunctionDef;
    use serde_json::json;

    fn sample_request() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("Book a meeting Monday at 3pm".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![ToolDef {
                kind: "function".to_string(),
                function: FunctionDef {
                    name: "create_calendar_event".to_string(),
                    description: Some("Create calendar event".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "start": {"type": "string"}
                        },
                        "required": ["title", "start"]
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
    fn test_apply_compaction_transforms_tools_to_system_prompt() {
        let mut req = sample_request();
        let compact_defs = apply_compaction(&mut req).expect("compaction should apply");

        assert_eq!(compact_defs.len(), 1);
        assert!(req.tools.is_none(), "tools array must be cleared from upstream request");
        assert_eq!(req.messages[0].role, "system");
        let sys_content = req.messages[0].text().unwrap();
        assert!(sys_content.contains("create_calendar_event("));
        assert!(sys_content.contains("<<call name {json}>>"));
    }

    #[test]
    fn test_restore_response_tool_calls_decodes_model_output() {
        let mut req = sample_request();
        let compact_defs = apply_compaction(&mut req).unwrap();

        let mut resp = ChatResponse {
            id: "chatcmpl-1".to_string(),
            object: "chat.completion".to_string(),
            created: Some(1718500000),
            model: "gpt-4o".to_string(),
            choices: vec![crate::ir::chat::Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String(
                        r#"<<call create_calendar_event {"title":"Design Review","start":"2026-10-05T15:00:00"}>>"#
                            .to_string(),
                    )),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
            extra: Default::default(),
        };

        restore_response_tool_calls(&mut resp, &compact_defs);

        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        let calls = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        assert!(calls[0].function.arguments.contains("Design Review"));
    }
}
