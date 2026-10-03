//! Compact tool schema encoding and decoding at the router seam (Track P1).
//!
//! Opt-in via `COMPACT_TOOLS_ENABLED=true` (off by default; behavior is byte-identical
//! when disabled). Compresses verbose OpenAI JSON schemas into compact 1-line signatures,
//! instructs the model to call via `<<call name {json args}>>`, and decodes model output
//! back into standard OpenAI `tool_calls`.

use nasiko_tool_compact::{ToolDef as CompactToolDef, decode_calls, encode_tools};
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};

/// State kept between request compaction and response decoding.
#[derive(Debug, Clone)]
pub struct CompactContext {
    pub tools: Vec<CompactToolDef>,
}

/// Applies compact tool encoding to a request if enabled.
///
/// Bypasses compaction (returns `None`) when:
/// - Feature flag `compact_tools_enabled` is false.
/// - The request is streaming (partial wiring focuses on non-streaming).
/// - The request has no tools.
/// - `tool_choice` is forced (not `None` and not `"auto"`).
/// - Any tool schema uses unsupported keywords (fails closed).
pub fn apply_to_request(req: &mut ChatRequest, cfg: &GatewayConfig) -> Option<CompactContext> {
    if !cfg.compact_tools_enabled {
        return None;
    }
    if req.is_streaming() {
        return None;
    }
    let ir_tools = req.tools.as_ref()?;
    if ir_tools.is_empty() {
        return None;
    }
    if !tool_choice_allows_compaction(req) {
        return None;
    }

    let compact_tool_defs = to_compact_tool_defs(ir_tools);
    let compact = encode_tools(&compact_tool_defs).ok()?;

    // Strip native tools and tool choice controls
    req.tools = None;
    req.tool_choice = None;

    // Prepend instructions and definitions as a system message
    let system_message = Message {
        role: "system".to_string(),
        content: Some(Value::String(compact.render())),
        tool_calls: None,
        name: None,
        tool_call_id: None,
        extra: Default::default(),
    };
    req.messages.insert(0, system_message);

    Some(CompactContext {
        tools: compact_tool_defs,
    })
}

/// Decodes model text response containing `<<call ...>>` markers back into `tool_calls`.
pub fn decode_response(resp: &mut ChatResponse, ctx: &CompactContext) {
    let Some(choice) = resp.choices.first_mut() else {
        return;
    };
    let content = match &choice.message.content {
        Some(Value::String(s)) => s.clone(),
        _ => return,
    };

    if !content.contains("<<call ") {
        return;
    }

    if let Ok(calls) = decode_calls(&content, &ctx.tools)
        && !calls.is_empty()
    {
        let ir_calls: Vec<ToolCall> = calls
            .into_iter()
            .map(|call| ToolCall {
                id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: call.name,
                    arguments: call.arguments.to_string(),
                },
                extra: Default::default(),
            })
            .collect();

        choice.message.tool_calls = Some(ir_calls);
        choice.finish_reason = Some("tool_calls".to_string());

        // Remove call markers from the assistant content text
        let cleaned_text = strip_call_markers(&content);
        choice.message.content = if cleaned_text.trim().is_empty() {
            None
        } else {
            Some(Value::String(cleaned_text))
        };
    }
}

fn tool_choice_allows_compaction(request: &ChatRequest) -> bool {
    match &request.tool_choice {
        None => true,
        Some(choice) => choice.as_str() == Some("auto"),
    }
}

fn to_compact_tool_defs(tools: &[ToolDef]) -> Vec<CompactToolDef> {
    tools
        .iter()
        .map(|t| CompactToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect()
}

fn strip_call_markers(mut text: &str) -> String {
    let mut out = String::new();
    while let Some(start) = text.find("<<call ") {
        out.push_str(&text[..start]);
        if let Some(end) = text[start..].find(">>") {
            text = &text[start + end + 2..];
        } else {
            break;
        }
    }
    out.push_str(text);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Choice, FunctionDef};
    use serde_json::json;

    fn sample_tool() -> ToolDef {
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "lookup".to_string(),
                description: Some("Search records".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    },
                    "required": ["query"]
                })),
            },
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_by_default_leaves_request_byte_identical() {
        let cfg = GatewayConfig::default();
        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("Find Alice".into())),
                tool_calls: None,
                name: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![sample_tool()]),
            tool_choice: None,
            stream: Some(false),
            temperature: None,
            max_tokens: None,
            extra: Default::default(),
        };

        let initial_json = serde_json::to_string(&req).unwrap();
        let ctx = apply_to_request(&mut req, &cfg);
        assert!(ctx.is_none());
        let final_json = serde_json::to_string(&req).unwrap();
        assert_eq!(initial_json, final_json);
    }

    #[test]
    fn enabled_compacts_tools_and_prepends_system_message() {
        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };
        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("Find Alice".into())),
                tool_calls: None,
                name: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![sample_tool()]),
            tool_choice: None,
            stream: Some(false),
            temperature: None,
            max_tokens: None,
            extra: Default::default(),
        };

        let ctx = apply_to_request(&mut req, &cfg).expect("compaction should activate");
        assert!(req.tools.is_none());
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert!(
            req.messages[0]
                .text()
                .as_ref()
                .unwrap()
                .contains("lookup(query:str)")
        );

        // Simulate model response
        let mut resp = ChatResponse {
            id: "chatcmpl-1".into(),
            object: "chat.completion".into(),
            created: None,
            model: "gpt-4o".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(Value::String(
                        "<<call lookup {\"query\": \"Alice\"}>>".into(),
                    )),
                    tool_calls: None,
                    name: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        };

        decode_response(&mut resp, &ctx);
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert!(choice.message.content.is_none());
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "lookup");
        assert_eq!(calls[0].function.arguments, r#"{"query":"Alice"}"#);
    }

    #[test]
    fn unsupported_schema_bypasses_compaction() {
        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };
        let mut tool = sample_tool();
        // Insert unsupported schema keyword "$ref"
        tool.function.parameters = Some(json!({
            "type": "object",
            "$ref": "#/definitions/SomeType"
        }));

        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![],
            tools: Some(vec![tool]),
            tool_choice: None,
            stream: Some(false),
            temperature: None,
            max_tokens: None,
            extra: Default::default(),
        };

        let ctx = apply_to_request(&mut req, &cfg);
        assert!(ctx.is_none());
        assert!(req.tools.is_some());
    }
}
