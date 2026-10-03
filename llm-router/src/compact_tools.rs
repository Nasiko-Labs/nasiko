//! Compact Tool Schemas integration seam for `nasiko-llm-router` (Track P1).
//!
//! When `cfg.compact_tools_enabled` is true, outbound requests with tool definitions
//! have their schemas compressed into concise DSL signatures and injected into prompt
//! instructions. Inbound model responses (both non-streaming and streaming) are decoded
//! back into standard OpenAI-compatible `ToolCall` and `ToolCallDelta` IR types.

use crate::config::GatewayConfig;
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, ChunkChoice, Delta, FunctionCall, Message, ToolCall,
    ToolCallDelta, ToolDef,
};
use nasiko_tool_compact::{
    CompactTools, StreamDecoder, decode_calls, encode_tools, strip_call_markers,
};
use serde_json::Value;

/// Holds the context of a compacted request to decode incoming responses.
#[derive(Debug, Clone)]
pub struct CompactContext {
    pub original_tools: Vec<nasiko_tool_compact::ToolDef>,
    pub compact_tools: CompactTools,
}

/// Convert IR ToolDef slice to nasiko_tool_compact::ToolDef vector.
pub fn to_compact_tool_defs(ir_tools: &[ToolDef]) -> Vec<nasiko_tool_compact::ToolDef> {
    ir_tools
        .iter()
        .map(|t| nasiko_tool_compact::ToolDef {
            kind: t.kind.clone(),
            function: nasiko_tool_compact::FunctionDef {
                name: t.function.name.clone(),
                description: t.function.description.clone(),
                parameters: t.function.parameters.clone(),
            },
        })
        .collect()
}

/// Convert nasiko_tool_compact::ToolCall to canonical IR ToolCall.
pub fn from_compact_tool_call(tc: &nasiko_tool_compact::ToolCall) -> ToolCall {
    ToolCall {
        id: tc.id.clone(),
        kind: tc.kind.clone(),
        function: FunctionCall {
            name: tc.function.name.clone(),
            arguments: tc.function.arguments.clone(),
        },
        extra: Default::default(),
    }
}

/// If enabled and tools are present, compacts the request schemas into prompt instructions
/// and removes native tools/tool_choice. Returns `Some(CompactContext)` if compaction was applied.
pub fn apply_compact_tools(req: &mut ChatRequest, cfg: &GatewayConfig) -> Option<CompactContext> {
    if !cfg.compact_tools_enabled {
        return None;
    }

    let ir_tools = req.tools.as_ref()?;
    if ir_tools.is_empty() {
        return None;
    }

    let compact_defs = to_compact_tool_defs(ir_tools);
    let compact_tools = match encode_tools(&compact_defs) {
        Ok(ct) => ct,
        Err(err) => {
            tracing::debug!(
                target: "nasiko::llm_router::compact_tools",
                error = %err,
                "compact_tools: schema not eligible for compaction, using native tools"
            );
            return None;
        }
    };

    // Inject compact schema prompt into system instructions
    inject_compact_prompt(&mut req.messages, &compact_tools.prompt);

    // Remove native tool definitions and tool_choice so provider treats as standard prompt
    req.tools = None;
    req.tool_choice = None;

    tracing::debug!(
        target: "nasiko::llm_router::compact_tools",
        tools_count = compact_defs.len(),
        "compact_tools: successfully applied compact schema transformation"
    );

    Some(CompactContext {
        original_tools: compact_defs,
        compact_tools,
    })
}

fn inject_compact_prompt(messages: &mut Vec<Message>, prompt: &str) {
    let directive = format!("\n\n[Tools]\n{prompt}");

    // If an existing system message exists, append to the first one
    if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == "system") {
        match &mut sys_msg.content {
            Some(Value::String(s)) => {
                s.push_str(&directive);
            }
            Some(Value::Array(arr)) => {
                arr.push(serde_json::json!({
                    "type": "text",
                    "text": directive,
                }));
            }
            _ => {
                sys_msg.content = Some(Value::String(directive.trim_start().to_string()));
            }
        }
    } else {
        // Prepend new system message
        messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: Some(Value::String(directive.trim_start().to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            },
        );
    }
}

/// Decode tool calls from a non-streaming ChatResponse.
pub fn decode_chat_response(resp: &mut ChatResponse, ctx: &CompactContext) {
    for choice in &mut resp.choices {
        let content_text = choice.message.text();
        let Some(text) = content_text else {
            continue;
        };

        if !text.contains("<<call ") {
            continue;
        }

        match decode_calls(&text, &ctx.original_tools) {
            Ok(calls) if !calls.is_empty() => {
                let ir_calls: Vec<ToolCall> = calls.iter().map(from_compact_tool_call).collect();
                choice.message.tool_calls = Some(ir_calls);
                choice.finish_reason = Some("tool_calls".to_string());

                let cleaned = strip_call_markers(&text);
                if cleaned.is_empty() {
                    choice.message.content = None;
                } else {
                    choice.message.content = Some(Value::String(cleaned));
                }
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    target: "nasiko::llm_router::compact_tools",
                    error = %err,
                    "compact_tools: failed to decode tool call from model output"
                );
            }
        }
    }
}

/// Streaming helper that intercepts chunks, extracts plain text, and decodes compact tool calls.
pub struct CompactStreamFilter {
    decoder: StreamDecoder,
    model: String,
    chunk_index: i64,
}

impl CompactStreamFilter {
    pub fn new(tools: Vec<nasiko_tool_compact::ToolDef>, model: String) -> Self {
        Self {
            decoder: StreamDecoder::new(tools),
            model,
            chunk_index: 0,
        }
    }

    /// Process a stream chunk. If tool calls complete within this chunk,
    /// returns an additional synthesized ChatChunk carrying ToolCallDelta.
    /// Updates `chunk` in-place so surrounding conversational text is preserved
    /// while call syntax is stripped.
    pub fn process_chunk(&mut self, chunk: &mut ChatChunk) -> Option<ChatChunk> {
        let mut text_piece = None;
        let mut choice_idx = 0;
        for (i, choice) in chunk.choices.iter_mut().enumerate() {
            if let Some(content) = choice.delta.content.take() {
                text_piece = Some(content);
                choice_idx = i;
                break;
            }
        }

        let Some(text) = text_piece else {
            return None;
        };

        let (plain_text, new_calls) = self.decoder.push_chunk_with_text(&text);
        if let Some(pt) = plain_text {
            chunk.choices[choice_idx].delta.content = Some(pt);
        }

        if new_calls.is_empty() {
            return None;
        }

        let deltas: Vec<ToolCallDelta> = new_calls
            .iter()
            .enumerate()
            .map(|(i, c)| ToolCallDelta {
                index: self.chunk_index + i as i64,
                id: Some(c.id.clone()),
                kind: Some(c.kind.clone()),
                function: Some(crate::ir::FunctionCallDelta {
                    name: Some(c.function.name.clone()),
                    arguments: Some(c.function.arguments.clone()),
                }),
            })
            .collect();

        self.chunk_index += new_calls.len() as i64;

        Some(ChatChunk {
            id: chunk.id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: chunk.created,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: None,
                    tool_calls: Some(deltas),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: None,
            extra: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Choice, FunctionDef};
    use serde_json::json;

    fn test_tool() -> ToolDef {
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "lookup_user".to_string(),
                description: Some("Find user by email".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "email": { "type": "string" }
                    },
                    "required": ["email"]
                })),
            },
            extra: Default::default(),
        }
    }

    #[test]
    fn test_opt_in_disabled_by_default() {
        let cfg = GatewayConfig::default();
        assert!(!cfg.compact_tools_enabled);

        let mut req = ChatRequest {
            model: None,
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("hello".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![test_tool()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        };

        let applied = apply_compact_tools(&mut req, &cfg);
        assert!(applied.is_none());
        assert!(req.tools.is_some(), "native tools must remain intact");
        assert_eq!(req.messages.len(), 1, "no prompt instruction inserted");
    }

    #[test]
    fn test_unsupported_schema_bypassed_safely() {
        let mut cfg = GatewayConfig::default();
        cfg.compact_tools_enabled = true;

        let unsupported_tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "any_of_tool".to_string(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "anyOf": [{ "type": "string" }, { "type": "integer" }]
                })),
            },
            extra: Default::default(),
        };

        let mut req = ChatRequest {
            model: None,
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("test".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![unsupported_tool]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        };

        let applied = apply_compact_tools(&mut req, &cfg);
        assert!(applied.is_none(), "must bypass unsupported schema");
        assert!(req.tools.is_some(), "native tools must be preserved");
    }

    #[test]
    fn test_opt_in_enabled_transformation() {
        let mut cfg = GatewayConfig::default();
        cfg.compact_tools_enabled = true;

        let mut req = ChatRequest {
            model: None,
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("find me user@example.com".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![test_tool()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        };

        let ctx = apply_compact_tools(&mut req, &cfg).expect("should apply compact tools");
        assert!(req.tools.is_none(), "tools must be removed from request");
        assert_eq!(ctx.original_tools.len(), 1);

        // Verify system message was injected
        let sys_msg = &req.messages[0];
        assert_eq!(sys_msg.role, "system");
        let content = sys_msg.text().unwrap();
        assert!(content.contains("lookup_user(email:str)"));
        assert!(content.contains("<<call name {json args}>>"));

        // Simulate model response
        let mut resp = ChatResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: Some(1234567890),
            model: "gpt-4o-mini".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String(
                        "<<call lookup_user {\"email\":\"user@example.com\"}>>".to_string(),
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

        decode_chat_response(&mut resp, &ctx);

        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert!(choice.message.content.is_none());
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "lookup_user");
        assert_eq!(
            calls[0].function.arguments,
            "{\"email\":\"user@example.com\"}"
        );
    }

    #[test]
    fn test_streaming_filter_text_and_tool_call() {
        let tools = to_compact_tool_defs(&[test_tool()]);
        let mut filter = CompactStreamFilter::new(tools, "gpt-4o-mini".to_string());

        // 1. Normal conversational text chunk
        let mut chunk1 = ChatChunk {
            id: "c1".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: None,
            model: "gpt-4o-mini".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: Some("Checking user record: ".to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
            extra: Default::default(),
        };

        let synth1 = filter.process_chunk(&mut chunk1);
        assert!(synth1.is_none());
        assert_eq!(
            chunk1.choices[0].delta.content.as_deref(),
            Some("Checking user record: ")
        );

        // 2. Tool call chunk
        let mut chunk2 = ChatChunk {
            id: "c2".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: None,
            model: "gpt-4o-mini".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: Some("<<call lookup_user {\"email\":\"a@b.com\"}>>".to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
            extra: Default::default(),
        };

        let synth2 = filter
            .process_chunk(&mut chunk2)
            .expect("should emit tool chunk");
        assert!(
            chunk2.choices[0].delta.content.is_none(),
            "marker stripped from content"
        );
        assert_eq!(
            synth2.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
        let tool_deltas = synth2.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_deltas.len(), 1);
        assert_eq!(
            tool_deltas[0].function.as_ref().unwrap().name.as_deref(),
            Some("lookup_user")
        );
    }
}
