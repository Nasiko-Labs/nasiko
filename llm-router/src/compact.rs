//! Track P1: Compact Tool Schemas router integration (Opt-in only).
//!
//! Provides transpilation from canonical router [`ToolDef`] to compact DSL definitions,
//! injection into system prompt instructions, and response/stream decoding of model tool calls.

use axum::http::HeaderMap;
use futures::StreamExt;
use futures::stream::BoxStream;
use nasiko_tool_compact::{
    CompactToolError, StreamDecoder, StreamEvent, ToolDefinition, decode_output, encode_tools,
};
use serde_json::{Map, Value};

use crate::config::GatewayConfig;
use crate::ir::chat::{
    ChatChunk, ChatRequest, ChatResponse, ChunkChoice, Delta, FunctionCall, FunctionCallDelta,
    Message, ToolCall, ToolCallDelta,
};
use crate::providers::ProviderError;

/// Header name for request-level opt-in.
pub const COMPACT_TOOLS_HEADER: &str = "x-nasiko-compact-tools";

/// Checks if compact tools mode should be active for this request.
///
/// Both the global gate ([`GatewayConfig::compact_tools_enabled`]) AND an explicit
/// request-level opt-in (header `x-nasiko-compact-tools: true` or body `compact_tools: true`)
/// are required.
pub fn is_compact_opt_in(headers: &HeaderMap, req: &ChatRequest, cfg: &GatewayConfig) -> bool {
    if !cfg.compact_tools_enabled {
        return false;
    }

    let header_opt_in = headers
        .get(COMPACT_TOOLS_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    let body_opt_in = req
        .extra
        .get("compact_tools")
        .map(|v| {
            v.as_bool().unwrap_or(false)
                || v.as_str()
                    .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
                    .unwrap_or(false)
        })
        .unwrap_or(false);

    header_opt_in || body_opt_in
}

/// If compact mode is enabled and the request has tools, transpiles them into compact DSL,
/// injects the specification into the system instructions, removes `req.tools` and `req.tool_choice`,
/// and returns the compact tool definitions for later decoding.
///
/// If schema features are unsupported or tool_choice is "none", safely falls back by leaving
/// `req.tools` intact and returning `None`.
pub fn prepare_compact_request(req: &mut ChatRequest) -> Option<Vec<ToolDefinition>> {
    let tools = req.tools.as_ref()?;
    if tools.is_empty() {
        return None;
    }

    if is_tool_choice_none(&req.tool_choice) {
        tracing::debug!(
            target: "nasiko::llm_router::compact",
            "compact tool encoding skipped: tool_choice is 'none'"
        );
        return None;
    }

    let tool_defs: Vec<ToolDefinition> = tools
        .iter()
        .map(|t| ToolDefinition {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect();

    let encoded = match encode_tools(&tool_defs) {
        Ok(c) => c,
        Err(CompactToolError::UnsupportedSchemaFeature { .. }) => {
            tracing::debug!(
                target: "nasiko::llm_router::compact",
                "compact tool encoding bypassed for unsupported schema; using native provider tools"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::compact",
                error = %e,
                "compact tool encoding failed; falling back to native provider tools"
            );
            return None;
        }
    };

    let mut instructions = encoded.prompt;
    if is_tool_choice_required(&req.tool_choice) {
        instructions.push_str(
            "\n\nYou MUST call at least one tool in your response using the <<call tool_name {...}>> syntax.",
        );
    } else if let Some(fn_name) = specific_tool_choice(&req.tool_choice) {
        instructions.push_str(&format!(
            "\n\nYou MUST call the tool '{fn_name}' in your response using the <<call {fn_name} {{...}}>> syntax."
        ));
    }

    inject_system_instructions(req, &instructions);

    req.tools = None;
    req.tool_choice = None;

    Some(tool_defs)
}

/// Injects compact instructions into the existing system message, or prepends a new system message.
fn inject_system_instructions(req: &mut ChatRequest, instructions: &str) {
    if let Some(sys_msg) = req.messages.iter_mut().find(|m| m.role == "system") {
        match &mut sys_msg.content {
            Some(Value::String(s)) => {
                s.push_str("\n\n");
                s.push_str(instructions);
            }
            Some(Value::Array(parts)) => {
                parts.push(serde_json::json!({
                    "type": "text",
                    "text": format!("\n\n{}", instructions)
                }));
            }
            _ => {
                sys_msg.content = Some(Value::String(instructions.to_string()));
            }
        }
    } else {
        req.messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: Some(Value::String(instructions.to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            },
        );
    }
}

/// Decodes compact tool call markers from the assistant response and restores canonical
/// [`ToolCall`] structures.
pub fn process_compact_response(resp: &mut ChatResponse, tool_defs: &[ToolDefinition]) {
    let Some(choice) = resp.choices.first_mut() else {
        return;
    };

    let raw_text = choice.message.text().unwrap_or_default();
    match decode_output(&raw_text, tool_defs) {
        Ok(output) => {
            if output.tool_calls.is_empty() {
                choice.message.content = Some(Value::String(output.text));
                choice.message.tool_calls = None;
            } else {
                let canonical_calls: Vec<ToolCall> = output
                    .tool_calls
                    .into_iter()
                    .enumerate()
                    .map(|(idx, tc)| ToolCall {
                        id: format!("call_{}_{}", tc.name, idx),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: tc.name,
                            arguments: tc.arguments,
                        },
                        extra: Map::new(),
                    })
                    .collect();

                choice.message.tool_calls = Some(canonical_calls);
                choice.finish_reason = Some("tool_calls".to_string());

                let cleaned_trimmed = output.text.trim();
                if cleaned_trimmed.is_empty() {
                    choice.message.content = None;
                } else {
                    choice.message.content = Some(Value::String(output.text));
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::compact",
                error = %e,
                "failed to decode compact tool call output from model; leaving plain text"
            );
        }
    }
}

/// Adapts a raw text provider streaming response to emit canonical [`ToolCallDelta`] events
/// via [`StreamDecoder`].
pub fn compact_stream_adapter(
    mut provider_stream: BoxStream<'static, Result<ChatChunk, ProviderError>>,
    tool_defs: Vec<ToolDefinition>,
    model: String,
) -> BoxStream<'static, Result<ChatChunk, ProviderError>> {
    let s = async_stream::stream! {
        let mut decoder = StreamDecoder::new(tool_defs);
        let mut chunk_id = String::new();
        let mut created = None;
        let mut last_usage = None;
        let mut has_error = false;

        while let Some(item) = provider_stream.next().await {
            match item {
                Ok(chunk) => {
                    chunk_id = chunk.id.clone();
                    created = chunk.created;
                    if chunk.usage.is_some() {
                        last_usage = chunk.usage.clone();
                    }

                    let content = chunk.choices.first().and_then(|c| c.delta.content.as_deref());
                    if let Some(text) = content {
                        match decoder.feed(text) {
                            Ok(events) => {
                                for event in events {
                                    match event {
                                        StreamEvent::TextDelta(t) => {
                                            yield Ok(ChatChunk {
                                                id: chunk_id.clone(),
                                                object: "chat.completion.chunk".to_string(),
                                                created,
                                                model: model.clone(),
                                                choices: vec![ChunkChoice {
                                                    index: 0,
                                                    delta: Delta {
                                                        role: None,
                                                        content: Some(t),
                                                        tool_calls: None,
                                                    },
                                                    finish_reason: None,
                                                }],
                                                usage: None,
                                                extra: Map::new(),
                                            });
                                        }
                                        StreamEvent::ToolCallStart { index, id: _, name } => {
                                            yield Ok(ChatChunk {
                                                id: chunk_id.clone(),
                                                object: "chat.completion.chunk".to_string(),
                                                created,
                                                model: model.clone(),
                                                choices: vec![ChunkChoice {
                                                    index: 0,
                                                    delta: Delta {
                                                        role: None,
                                                        content: None,
                                                        tool_calls: Some(vec![ToolCallDelta {
                                                            index: index as i64,
                                                            id: Some(format!("call_{}_{}", name, index)),
                                                            kind: Some("function".to_string()),
                                                            function: Some(FunctionCallDelta {
                                                                 name: Some(name),
                                                                 arguments: Some(String::new()),
                                                            }),
                                                        }]),
                                                    },
                                                    finish_reason: None,
                                                }],
                                                usage: None,
                                                extra: Map::new(),
                                            });
                                        }
                                        StreamEvent::ToolCallArgsDelta { index, delta } => {
                                            yield Ok(ChatChunk {
                                                id: chunk_id.clone(),
                                                object: "chat.completion.chunk".to_string(),
                                                created,
                                                model: model.clone(),
                                                choices: vec![ChunkChoice {
                                                    index: 0,
                                                    delta: Delta {
                                                        role: None,
                                                        content: None,
                                                        tool_calls: Some(vec![ToolCallDelta {
                                                            index: index as i64,
                                                            id: None,
                                                            kind: None,
                                                            function: Some(FunctionCallDelta {
                                                                name: None,
                                                                arguments: Some(delta),
                                                            }),
                                                        }]),
                                                    },
                                                    finish_reason: None,
                                                }],
                                                usage: None,
                                                extra: Map::new(),
                                            });
                                        }
                                        StreamEvent::ToolCallComplete { .. } => {}
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    target: "nasiko::llm_router::compact",
                                    error = %e,
                                    "compact stream decoder encountered an error on feed; stopping tool delta emission"
                                );
                                has_error = true;
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    yield Err(e);
                    return;
                }
            }
        }

        if !has_error {
            match decoder.finish() {
                Ok(final_events) => {
                    for event in final_events {
                        if let StreamEvent::TextDelta(t) = event {
                            yield Ok(ChatChunk {
                                id: chunk_id.clone(),
                                object: "chat.completion.chunk".to_string(),
                                created,
                                model: model.clone(),
                                choices: vec![ChunkChoice {
                                    index: 0,
                                    delta: Delta {
                                        role: None,
                                        content: Some(t),
                                        tool_calls: None,
                                    },
                                    finish_reason: None,
                                }],
                                usage: None,
                                extra: Map::new(),
                            });
                        }
                    }

                    let finish_reason = if !decoder.completed_calls().is_empty() {
                        "tool_calls".to_string()
                    } else {
                        "stop".to_string()
                    };

                    yield Ok(ChatChunk {
                        id: chunk_id,
                        object: "chat.completion.chunk".to_string(),
                        created,
                        model,
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: Delta::default(),
                            finish_reason: Some(finish_reason),
                        }],
                        usage: last_usage,
                        extra: Map::new(),
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        target: "nasiko::llm_router::compact",
                        error = %e,
                        "compact stream decoder unclosed/invalid at finish"
                    );
                    yield Ok(ChatChunk {
                        id: chunk_id,
                        object: "chat.completion.chunk".to_string(),
                        created,
                        model,
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: Delta::default(),
                            finish_reason: Some("stop".to_string()),
                        }],
                        usage: last_usage,
                        extra: Map::new(),
                    });
                }
            }
        }
    };

    Box::pin(s)
}

fn is_tool_choice_none(tc: &Option<Value>) -> bool {
    matches!(tc, Some(Value::String(s)) if s.eq_ignore_ascii_case("none"))
}

fn is_tool_choice_required(tc: &Option<Value>) -> bool {
    matches!(tc, Some(Value::String(s)) if s.eq_ignore_ascii_case("required"))
}

fn specific_tool_choice(tc: &Option<Value>) -> Option<String> {
    match tc {
        Some(Value::Object(map)) => {
            if let Some(func_obj) = map.get("function").and_then(Value::as_object) {
                func_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            } else {
                map.get("name")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Choice, FunctionDef, ToolDef};
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                kind: "function".to_string(),
                function: FunctionDef {
                    name: "search".to_string(),
                    description: Some("Search the web".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        },
                        "required": ["query"]
                    })),
                },
                extra: Map::new(),
            },
            ToolDef {
                kind: "function".to_string(),
                function: FunctionDef {
                    name: "calculator".to_string(),
                    description: Some("Calculate expression".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "expr": { "type": "string" }
                        },
                        "required": ["expr"]
                    })),
                },
                extra: Map::new(),
            },
        ]
    }

    #[test]
    fn test_opt_in_requires_both_global_and_request_gate() {
        let disabled_cfg = GatewayConfig {
            compact_tools_enabled: false,
            ..Default::default()
        };
        let enabled_cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };

        let req_plain = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![],
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let mut req_body_opt_in = req_plain.clone();
        req_body_opt_in
            .extra
            .insert("compact_tools".to_string(), json!(true));

        let headers_plain = HeaderMap::new();
        let mut headers_opt_in = HeaderMap::new();
        headers_opt_in.insert(COMPACT_TOOLS_HEADER, "true".parse().unwrap());

        // Global OFF: always false regardless of request
        assert!(!is_compact_opt_in(
            &headers_plain,
            &req_plain,
            &disabled_cfg
        ));
        assert!(!is_compact_opt_in(
            &headers_opt_in,
            &req_plain,
            &disabled_cfg
        ));
        assert!(!is_compact_opt_in(
            &headers_plain,
            &req_body_opt_in,
            &disabled_cfg
        ));

        // Global ON but request OFF: false
        assert!(!is_compact_opt_in(&headers_plain, &req_plain, &enabled_cfg));

        // Global ON + header opt-in: true
        assert!(is_compact_opt_in(&headers_opt_in, &req_plain, &enabled_cfg));

        // Global ON + body opt-in: true
        assert!(is_compact_opt_in(
            &headers_plain,
            &req_body_opt_in,
            &enabled_cfg
        ));
    }

    #[test]
    fn test_prepare_compact_request_removes_tools_and_preserves_system_prompt() {
        let mut req = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: Some(Value::String("System instructions.".to_string())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Map::new(),
                },
                Message {
                    role: "user".to_string(),
                    content: Some(Value::String("Search for rust".to_string())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Map::new(),
                },
            ],
            tools: Some(sample_tools()),
            tool_choice: Some(json!("auto")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let defs = prepare_compact_request(&mut req).expect("should prepare compact request");
        assert_eq!(defs.len(), 2);
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());

        let sys_content = req.messages[0].text().unwrap();
        assert!(sys_content.starts_with("System instructions."));
        assert!(sys_content.contains("tool search("));
        assert!(sys_content.contains("<<call tool_name {\"arg\": \"value\"}>>"));
    }

    #[test]
    fn test_prepare_compact_request_prepends_system_message_when_missing() {
        let mut req = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("hello".to_string())),
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

        let defs = prepare_compact_request(&mut req).expect("should prepare");
        assert_eq!(defs.len(), 2);
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert!(req.messages[0].text().unwrap().contains("tool search("));
        assert_eq!(req.messages[1].role, "user");
    }

    #[test]
    fn test_prepare_compact_request_unsupported_schema_fallback() {
        let unsupported_tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "unsupported_tool".to_string(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "oneOf": [{ "type": "string" }]
                })),
            },
            extra: Map::new(),
        };

        let mut req = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("hi".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(vec![unsupported_tool]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };

        let defs = prepare_compact_request(&mut req);
        assert!(defs.is_none());
        assert!(req.tools.is_some(), "req.tools must remain untouched");
        assert_eq!(req.messages.len(), 1, "no system message injected");
    }

    #[test]
    fn test_tool_choice_variants() {
        // tool_choice = "none" -> skipped
        let mut req_none = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![],
            tools: Some(sample_tools()),
            tool_choice: Some(json!("none")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };
        assert!(prepare_compact_request(&mut req_none).is_none());
        assert!(req_none.tools.is_some());

        // tool_choice = "required"
        let mut req_req = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![],
            tools: Some(sample_tools()),
            tool_choice: Some(json!("required")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };
        let _ = prepare_compact_request(&mut req_req).unwrap();
        assert!(
            req_req.messages[0]
                .text()
                .unwrap()
                .contains("You MUST call at least one tool")
        );

        // specific function
        let mut req_spec = ChatRequest {
            model: Some("gpt-4o".to_string()),
            messages: vec![],
            tools: Some(sample_tools()),
            tool_choice: Some(json!({"type":"function","function":{"name":"search"}})),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };
        let _ = prepare_compact_request(&mut req_spec).unwrap();
        assert!(
            req_spec.messages[0]
                .text()
                .unwrap()
                .contains("You MUST call the tool 'search'")
        );
    }

    #[test]
    fn test_process_compact_response_single_and_multiple_calls() {
        let tools = vec![
            ToolDefinition::new(
                "search",
                None,
                Some(
                    json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
                ),
            ),
            ToolDefinition::new(
                "calculator",
                None,
                Some(
                    json!({"type":"object","properties":{"expr":{"type":"string"}},"required":["expr"]}),
                ),
            ),
        ];

        // Single call
        let mut resp_single = ChatResponse {
            id: "chatcmpl-1".to_string(),
            object: "chat.completion".to_string(),
            created: None,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String(
                        "<<call search {\"query\":\"nasiko\"}>>".into(),
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

        process_compact_response(&mut resp_single, &tools);
        let msg = &resp_single.choices[0].message;
        assert!(msg.content.is_none());
        assert_eq!(
            resp_single.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
        let calls = msg.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_search_0");
        assert_eq!(calls[0].function.name, "search");
        assert_eq!(calls[0].function.arguments, "{\"query\":\"nasiko\"}");

        // Multiple calls with mixed text
        let mut resp_multi = ChatResponse {
            id: "chatcmpl-2".to_string(),
            object: "chat.completion".to_string(),
            created: None,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String(
                        "Starting.\n<<call search {\"query\":\"first\"}>>\n<<call calculator {\"expr\":\"2+2\"}>>\nDone."
                            .into(),
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

        process_compact_response(&mut resp_multi, &tools);
        let msg = &resp_multi.choices[0].message;
        assert_eq!(
            msg.content.as_ref().unwrap().as_str().unwrap(),
            "Starting.\n\n\nDone."
        );
        let calls = msg.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_search_0");
        assert_eq!(calls[0].function.name, "search");
        assert_eq!(calls[1].id, "call_calculator_1");
        assert_eq!(calls[1].function.name, "calculator");
    }

    #[test]
    fn test_process_compact_response_no_calls_and_malformed_calls() {
        let tools = vec![ToolDefinition::new(
            "search",
            None,
            Some(
                json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
            ),
        )];

        // Plain text
        let mut resp_text = ChatResponse {
            id: "chatcmpl-3".to_string(),
            object: "chat.completion".to_string(),
            created: None,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String("Plain conversational text".into())),
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
        process_compact_response(&mut resp_text, &tools);
        assert_eq!(
            resp_text.choices[0].message.text().as_deref(),
            Some("Plain conversational text")
        );
        assert!(resp_text.choices[0].message.tool_calls.is_none());
        assert_eq!(resp_text.choices[0].finish_reason.as_deref(), Some("stop"));

        // Malformed call -> fail closed, no tool calls created
        let mut resp_malformed = ChatResponse {
            id: "chatcmpl-4".to_string(),
            object: "chat.completion".to_string(),
            created: None,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(Value::String("<<call search {not valid json}>>".into())),
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
        process_compact_response(&mut resp_malformed, &tools);
        assert!(resp_malformed.choices[0].message.tool_calls.is_none());
    }

    #[tokio::test]
    async fn test_streaming_adapter_and_split_chunks() {
        let tools = vec![ToolDefinition::new(
            "search",
            None,
            Some(
                json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
            ),
        )];

        let chunks = vec![
            Ok(ChatChunk {
                id: "c1".into(),
                object: "chat.completion.chunk".into(),
                created: None,
                model: "gpt-4o".into(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some("Here is the result: <<ca".into()),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
                usage: None,
                extra: Map::new(),
            }),
            Ok(ChatChunk {
                id: "c2".into(),
                object: "chat.completion.chunk".into(),
                created: None,
                model: "gpt-4o".into(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some("ll search {\"query\":\"tok".into()),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
                usage: None,
                extra: Map::new(),
            }),
            Ok(ChatChunk {
                id: "c3".into(),
                object: "chat.completion.chunk".into(),
                created: None,
                model: "gpt-4o".into(),
                choices: vec![ChunkChoice {
                    index: 0,
                    delta: Delta {
                        role: None,
                        content: Some("yo\"}>>".into()),
                        tool_calls: None,
                    },
                    finish_reason: None,
                }],
                usage: None,
                extra: Map::new(),
            }),
        ];

        let provider_stream = Box::pin(futures::stream::iter(chunks));
        let adapted = compact_stream_adapter(provider_stream, tools, "gpt-4o".into());
        let results: Vec<Result<ChatChunk, ProviderError>> = adapted.collect().await;

        let mut text_acc = String::new();
        let mut tool_calls_seen = Vec::new();
        let mut final_finish_reason = None;

        for res in results {
            let chunk = res.unwrap();
            let choice = &chunk.choices[0];
            if let Some(content) = &choice.delta.content {
                text_acc.push_str(content);
            }
            if let Some(calls) = &choice.delta.tool_calls {
                tool_calls_seen.extend(calls.clone());
            }
            if let Some(fr) = &choice.finish_reason {
                final_finish_reason = Some(fr.clone());
            }
        }

        assert_eq!(text_acc, "Here is the result: ");
        assert!(!tool_calls_seen.is_empty());
        assert_eq!(tool_calls_seen[0].id.as_deref(), Some("call_search_0"));
        assert_eq!(
            tool_calls_seen[0]
                .function
                .as_ref()
                .unwrap()
                .name
                .as_deref(),
            Some("search")
        );
        assert_eq!(final_finish_reason.as_deref(), Some("tool_calls"));
    }
}
