//! Lossless compact tool schemas and fail-closed decoder at the router seam.
//!
//! When enabled (`GatewayConfig.compact_tools_enabled`), outbound requests carrying
//! tool definitions are compacted into terse signature lines and injected into the
//! system message alongside strict JSON-call instructions (`<<call name {args}>>`).
//! Native `tools` are cleared before forwarding to the provider, drastically reducing
//! prompt tokens.
//!
//! Upon provider response:
//! - In non-streaming mode: Assistant text is scanned for `<<call name {json}>>`.
//!   Calls are parsed, validated against the original tool schemas (fail closed),
//!   and placed in `message.tool_calls`. Surrounding assistant text is preserved.
//! - In streaming mode: SSE chunks are decoded incrementally using [`StreamDecoder`],
//!   emitting standard [`ToolCallDelta`] items.
//!
//! Bypass rules (fall back to native tools without compaction):
//! 1. `compact_tools_enabled` is false.
//! 2. No tools in request.
//! 3. `tool_choice` is set to "required" or forces a specific function.
//! 4. Conversation already contains prior tool calls or tool results.
//! 5. Any tool schema uses unsupported features (`$ref`, `patternProperties`, etc.).

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};
use nasiko_tool_compact::{CompactError, InstructionVariant, ToolDef as CompactToolDef};
use serde_json::Value;

/// Context preserved from egress to ingress to decode and validate tool calls.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactToolsContext {
    pub original_tools: Vec<CompactToolDef>,
}

/// Why compaction was bypassed for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BypassReason {
    Disabled,
    NoTools,
    ToolChoiceNone,
    ToolChoiceForced,
    ToolContinuation,
    UnsupportedSchema(String),
}

/// Convert IR ToolDef to nasiko_tool_compact ToolDef.
fn to_compact_tool_def(tool: &crate::ir::chat::ToolDef) -> CompactToolDef {
    CompactToolDef {
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        parameters: tool.function.parameters.clone(),
    }
}

/// Inspect outbound request and apply compact tools transformation if eligible.
///
/// If bypassed, leaves `req` completely untouched and returns `Ok(None)` or `Err(reason)`.
pub fn apply_egress(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
) -> Result<Option<CompactToolsContext>, BypassReason> {
    if !cfg.compact_tools_enabled {
        return Err(BypassReason::Disabled);
    }

    let tools = match req.tools.as_ref() {
        Some(t) if !t.is_empty() => t,
        _ => return Err(BypassReason::NoTools),
    };

    // Check tool_choice
    if let Some(ref choice) = req.tool_choice {
        match choice {
            Value::String(s) if s == "none" => return Err(BypassReason::ToolChoiceNone),
            Value::String(s) if s == "required" => return Err(BypassReason::ToolChoiceForced),
            Value::Object(map) if !map.is_empty() => return Err(BypassReason::ToolChoiceForced),
            _ => {}
        }
    }

    // Check for prior tool calls or tool continuation in history
    let has_prior_tools = req.messages.iter().any(|m| {
        m.role == "tool"
            || m.tool_call_id.is_some()
            || m.tool_calls
                .as_ref()
                .map(|tc| !tc.is_empty())
                .unwrap_or(false)
    });
    if has_prior_tools || crate::routing::is_tool_continuation(&req.messages) {
        return Err(BypassReason::ToolContinuation);
    }

    // Convert tool definitions
    let compact_defs: Vec<CompactToolDef> = tools.iter().map(to_compact_tool_def).collect();

    // Check encoding support
    let encoded = match nasiko_tool_compact::encode_tools(&compact_defs) {
        Ok(enc) => enc,
        Err(CompactError::Unsupported { tool, feature }) => {
            return Err(BypassReason::UnsupportedSchema(format!(
                "tool '{}' uses unsupported feature '{}'",
                tool, feature
            )));
        }
        Err(e) => {
            return Err(BypassReason::UnsupportedSchema(format!("{}", e)));
        }
    };

    // Compaction is eligible and succeeded: mutate request
    req.tools = None;
    req.tool_choice = None;
    req.extra.remove("parallel_tool_calls");

    // Inject system message with signatures and grammar instructions
    let instruction = InstructionVariant::Concise.text();
    let prompt = format!("{}\n\n{}", encoded.text, instruction);

    // Prepend as initial system message if none exists, or append
    let injected = Message {
        role: "system".into(),
        content: Some(Value::String(prompt)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    };
    req.messages.insert(0, injected);

    Ok(Some(CompactToolsContext {
        original_tools: compact_defs,
    }))
}

/// Decode tool calls from non-streaming assistant response choices.
pub fn apply_ingress(
    resp: &mut ChatResponse,
    ctx: &CompactToolsContext,
) -> Result<(), GatewayError> {
    for choice in resp.choices.iter_mut() {
        let content_str = match choice.message.content.as_ref() {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };

        if !content_str.contains("<<call") {
            continue;
        }

        // Decode calls
        let calls = nasiko_tool_compact::decode_calls(&content_str, &ctx.original_tools)
            .map_err(|e| GatewayError::Upstream(format!("compact-tools decode error: {}", e)))?;

        if !calls.is_empty() {
            let ir_calls: Vec<ToolCall> = calls
                .into_iter()
                .enumerate()
                .map(|(idx, call)| ToolCall {
                    id: format!("call_{}", idx + 1),
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

            // Strip <<call ...>> syntax from content
            let cleaned = strip_calls(&content_str);
            if cleaned.trim().is_empty() {
                choice.message.content = None;
            } else {
                choice.message.content = Some(Value::String(cleaned));
            }
        }
    }

    Ok(())
}

/// Helper to strip <<call ...>> occurrences from text using exact JSON-aware call spans.
fn strip_calls(text: &str) -> String {
    let spans = nasiko_tool_compact::scan_call_spans(text);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end) in spans {
        if start > last {
            out.push_str(&text[last..start]);
        }
        last = end;
    }
    if last < text.len() {
        out.push_str(&text[last..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{FunctionDef, ToolDef};
    use serde_json::json;

    fn sample_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "get_weather".into(),
                description: Some("Fetch current weather".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" },
                        "units": { "type": "string", "enum": ["metric", "imperial"], "default": "metric" }
                    },
                    "required": ["city"]
                })),
            },
            extra: Default::default(),
        }
    }

    #[test]
    fn test_flag_off_leaves_request_untouched() {
        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!("hello")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![sample_tool()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        };

        let cfg = GatewayConfig {
            compact_tools_enabled: false,
            ..Default::default()
        };

        let orig_json = serde_json::to_string(&req).unwrap();
        let res = apply_egress(&mut req, &cfg);
        assert_eq!(res, Err(BypassReason::Disabled));
        let after_json = serde_json::to_string(&req).unwrap();
        assert_eq!(orig_json, after_json, "Byte-identical when off");
    }

    #[test]
    fn test_bypass_unsupported_schema() {
        let mut tool = sample_tool();
        tool.function.parameters = Some(json!({
            "type": "object",
            "properties": {
                "ref_data": { "$ref": "#/definitions/Foo" }
            }
        }));

        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!("query")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![tool]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        };

        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };

        let res = apply_egress(&mut req, &cfg);
        assert!(matches!(res, Err(BypassReason::UnsupportedSchema(_))));
        assert!(req.tools.is_some(), "Tools preserved on bypass");
    }

    #[test]
    fn test_apply_egress_and_ingress_non_streaming() {
        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!("weather in Paris")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![sample_tool()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        };

        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };

        let ctx = apply_egress(&mut req, &cfg).unwrap().expect("Compacted");
        assert!(req.tools.is_none(), "Tools cleared from request");
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");

        // Simulate provider response
        let mut resp = ChatResponse {
            id: "chatcmpl-123".into(),
            object: "chat.completion".into(),
            created: Some(1720000000),
            model: "gpt-4o".into(),
            choices: vec![crate::ir::chat::Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(json!(
                        "Checking weather <<call get_weather {\"city\":\"Paris\",\"units\":\"metric\"}>> done"
                    )),
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

        apply_ingress(&mut resp, &ctx).unwrap();

        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(
            choice.message.content.as_ref().and_then(|v| v.as_str()),
            Some("Checking weather  done")
        );
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(calls[0].id, "call_1");
    }

    #[test]
    fn test_ingress_decode_error_fails_closed() {
        let ctx = CompactToolsContext {
            original_tools: vec![to_compact_tool_def(&sample_tool())],
        };

        let mut resp = ChatResponse {
            id: "chatcmpl-123".into(),
            object: "chat.completion".into(),
            created: Some(1720000000),
            model: "gpt-4o".into(),
            choices: vec![crate::ir::chat::Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(json!("<<call unknown_tool {}>>")),
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

        let res = apply_ingress(&mut resp, &ctx);
        assert!(res.is_err(), "Must fail closed on unknown tool");
    }

    #[test]
    fn test_bypass_when_tool_choice_is_none() {
        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!("hello")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![sample_tool()]),
            tool_choice: Some(json!("none")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        };

        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };

        let res = apply_egress(&mut req, &cfg);
        assert_eq!(res, Err(BypassReason::ToolChoiceNone));
        assert!(req.tools.is_some());
    }

    #[test]
    fn test_tools_cleared_removes_tool_choice_and_parallel_tool_calls() {
        let mut extra = serde_json::Map::new();
        extra.insert("parallel_tool_calls".to_string(), json!(true));

        let mut req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!("hello")),
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
            extra,
        };

        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };

        let res = apply_egress(&mut req, &cfg);
        assert!(res.is_ok());
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        assert!(!req.extra.contains_key("parallel_tool_calls"));
    }

    #[test]
    fn test_strip_calls_with_embedded_markers_in_arguments() {
        let text = "Result: <<call send_email {\"to\":[\"a@b.co\"],\"subject\":\"a >> b\",\"body\":\"c >> d <<call x>>\"}>> and done!";
        let cleaned = strip_calls(text);
        assert_eq!(cleaned, "Result:  and done!");
    }

    #[test]
    fn test_serde_json_preserves_canonical_lexicographical_ordering() {
        let mut extra = serde_json::Map::new();
        extra.insert("zebra".to_string(), json!(1));
        extra.insert("apple".to_string(), json!(2));
        extra.insert("mango".to_string(), json!(3));

        let req = ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![],
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra,
        };

        let serialized = serde_json::to_string(&req).unwrap();
        let apple_pos = serialized.find("\"apple\":2").unwrap();
        let mango_pos = serialized.find("\"mango\":3").unwrap();
        let zebra_pos = serialized.find("\"zebra\":1").unwrap();
        assert!(
            apple_pos < mango_pos && mango_pos < zebra_pos,
            "Keys in extra must follow upstream BTreeMap sorted ordering without preserve_order"
        );
    }
}
