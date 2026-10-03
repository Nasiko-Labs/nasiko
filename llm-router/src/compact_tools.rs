//! Opt-in compact tool schemas seam (P1 track).
//!
//! Compacts tool definitions at the router egress seam to reduce prompt tokens,
//! and decodes model responses back into standard OpenAI-shaped tool calls.
//!
//! When `GatewayConfig::compact_tools_enabled` is `false` (the default), this layer
//! is completely bypassed and requests/responses are 100% byte-identical to before.

use serde_json::Value;

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{
    ChatRequest, ChatResponse, FunctionCall as IrFunctionCall, ToolCall as IrToolCall,
};
use nasiko_tool_compact::{ToolDef, decode_calls_and_text, encode_tools};

/// State preserved across a compact-tools request for decoding the response.
#[derive(Debug, Clone)]
pub struct AppliedCompactTools {
    pub original_tools: Vec<ToolDef>,
}

/// Convert an IR ToolDef to a `nasiko_tool_compact::ToolDef`.
pub fn ir_to_compact_tool(tool: &crate::ir::chat::ToolDef) -> ToolDef {
    ToolDef {
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        parameters: tool.function.parameters.clone(),
    }
}

/// Apply compact tool encoding to `req` if enabled and applicable.
///
/// Returns `Some(AppliedCompactTools)` if tools were compacted, or `None` if skipped.
pub fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Option<AppliedCompactTools> {
    if !cfg.compact_tools_enabled {
        return None;
    }

    // Tools must be present and non-empty.
    let tools = req.tools.as_ref().filter(|t| !t.is_empty())?;

    // Streaming is bypassed for compact tools (only non-streaming wired in this phase).
    if req.is_streaming() {
        return None;
    }

    // If tool_choice is explicitly "none", do not compact or inject instructions.
    if req.tool_choice.as_ref().and_then(|c| c.as_str()) == Some("none") {
        return None;
    }

    // Convert IR tools to tool-compact definitions.
    let compact_defs: Vec<ToolDef> = tools.iter().map(ir_to_compact_tool).collect();

    // Encode tools. If encoding fails, fail safe and leave request unchanged.
    let compact = match encode_tools(&compact_defs) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::compact_tools",
                error = %e,
                "compact_tools: failed to encode tools, bypassing compaction"
            );
            return None;
        }
    };

    // Remove native tools and tool_choice from request.
    req.tools = None;
    if req.tool_choice.as_ref().and_then(|c| c.as_str()) == Some("auto") {
        req.tool_choice = None;
    }

    // Inject compact prompt as a system message.
    req.messages.push(crate::ir::chat::Message {
        role: "system".into(),
        content: Some(Value::String(compact.prompt)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });

    tracing::debug!(
        target: "nasiko::llm_router::compact_tools",
        tool_count = compact_defs.len(),
        "compact_tools: compacted tools and injected system instructions"
    );

    Some(AppliedCompactTools {
        original_tools: compact_defs,
    })
}

/// Decode tool calls from the model's response and restore OpenAI-shaped tool calls.
///
/// Fails closed if the model returned malformed markers, unknown tools, or invalid arguments.
pub fn decode_response(
    resp: &mut ChatResponse,
    applied: &AppliedCompactTools,
) -> Result<(), GatewayError> {
    for choice in &mut resp.choices {
        let text = match choice.message.text() {
            Some(t) => t,
            None => continue,
        };

        let (calls, remaining_text) = decode_calls_and_text(&text, &applied.original_tools)
            .map_err(|e| GatewayError::Upstream(format!("compact tool decode error: {e}")))?;

        if !calls.is_empty() {
            let ir_calls: Vec<IrToolCall> = calls
                .into_iter()
                .enumerate()
                .map(|(idx, call)| IrToolCall {
                    id: format!("call_{}", idx + 1),
                    kind: "function".to_string(),
                    function: IrFunctionCall {
                        name: call.name,
                        arguments: serde_json::to_string(&call.arguments).unwrap_or_default(),
                    },
                    extra: Default::default(),
                })
                .collect();

            choice.message.tool_calls = Some(ir_calls);
            choice.finish_reason = Some("tool_calls".to_string());

            if remaining_text.is_empty() {
                choice.message.content = None;
            } else {
                choice.message.content = Some(Value::String(remaining_text));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Choice, FunctionDef, Message, ToolDef as IrToolDef};
    use serde_json::json;

    fn sample_tool() -> IrToolDef {
        IrToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "get_weather".to_string(),
                description: Some("Get the current weather".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name" }
                    },
                    "required": ["location"]
                })),
            },
            extra: Default::default(),
        }
    }

    fn sample_request() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(json!("What is the weather in Tokyo?")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![sample_tool()]),
            tool_choice: Some(json!("auto")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_by_default_leaves_request_byte_identical() {
        let mut req = sample_request();
        let original_serialized = serde_json::to_string(&req).unwrap();

        let cfg = GatewayConfig::default();
        assert!(!cfg.compact_tools_enabled, "must be off by default");

        let applied = apply(&mut req, &cfg);
        assert!(applied.is_none(), "must not apply when disabled");

        let after_serialized = serde_json::to_string(&req).unwrap();
        assert_eq!(
            original_serialized, after_serialized,
            "request must remain byte-identical when flag is off"
        );
    }

    #[test]
    fn enabled_compacts_tools_and_removes_native() {
        let mut req = sample_request();
        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };

        let applied = apply(&mut req, &cfg);
        assert!(applied.is_some(), "must apply when enabled");
        assert!(req.tools.is_none(), "native tools must be removed");
        assert!(
            req.tool_choice.is_none(),
            "auto tool_choice must be removed"
        );
        assert_eq!(
            req.messages.len(),
            2,
            "compact prompt injected as system message"
        );

        let sys_msg = &req.messages[1];
        assert_eq!(sys_msg.role, "system");
        let content = sys_msg.text().unwrap();
        assert!(content.contains("get_weather(location:str)"));
        assert!(content.contains("<<call"));
    }

    #[test]
    fn decode_response_converts_compact_calls() {
        let tool = sample_tool();
        let compact_tool = ir_to_compact_tool(&tool);
        let applied = AppliedCompactTools {
            original_tools: vec![compact_tool],
        };

        let mut resp = ChatResponse {
            id: "chatcmpl-1".to_string(),
            object: "chat.completion".to_string(),
            created: None,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(json!(r#"<<call get_weather {"location": "Tokyo"}>>"#)),
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

        decode_response(&mut resp, &applied).unwrap();

        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert!(
            choice.message.content.is_none(),
            "content stripped when pure call"
        );
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(calls[0].function.arguments, r#"{"location":"Tokyo"}"#);
    }

    #[test]
    fn decode_response_fails_closed_on_unknown_tool() {
        let tool = sample_tool();
        let compact_tool = ir_to_compact_tool(&tool);
        let applied = AppliedCompactTools {
            original_tools: vec![compact_tool],
        };

        let mut resp = ChatResponse {
            id: "chatcmpl-1".to_string(),
            object: "chat.completion".to_string(),
            created: None,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(json!(r#"<<call nonexistent_tool {"arg": 1}>>"#)),
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

        let err = decode_response(&mut resp, &applied);
        assert!(err.is_err(), "must fail closed on unknown tool");
    }

    #[test]
    fn decode_response_fails_closed_on_invalid_arguments() {
        let tool = sample_tool();
        let compact_tool = ir_to_compact_tool(&tool);
        let applied = AppliedCompactTools {
            original_tools: vec![compact_tool],
        };

        let mut resp = ChatResponse {
            id: "chatcmpl-1".to_string(),
            object: "chat.completion".to_string(),
            created: None,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(json!(r#"<<call get_weather {}>"#)), // missing location
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

        let err = decode_response(&mut resp, &applied);
        assert!(err.is_err(), "must fail closed on missing required args");
    }
}
