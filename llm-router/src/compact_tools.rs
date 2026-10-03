//! Opt-in compact tool schemas at the egress seam.
//!
//! The canonical implementation lives in the pure `nasiko-tool-compact` crate
//! (no I/O, no env reads); this module is the thin router seam that converts
//! the router's IR types ([`crate::ir`]) at the boundary and enforces the
//! opt-in, fail-closed policy.
//!
//! # Policy
//!
//! - Disabled by default (`GatewayConfig::compact_tools_enabled == false`).
//!   [`apply_to_request`] returns `Applied::Disabled` and leaves the request
//!   byte-identical (guarded by test).
//! - Enabled: native `tools` are replaced by an injected system message
//!   carrying the compact block **only** when every tool compacts and the
//!   request is eligible; otherwise the request is left untouched
//!   (`Applied::Bypassed`) and native tools are sent.
//! - Eligibility is intentionally narrow (fail closed): non-streaming OpenAI
//!   chat requests with `tool_choice` absent or `auto`/`none`. Streaming,
//!   forced `tool_choice`, and non-OpenAI surfaces bypass today.
//!
//! # Coverage
//!
//! Supported helper scope: OpenAI `POST /v1/chat/completions`, non-streaming,
//! request side (tools → compact block). The helper itself bypasses streaming
//! requests, forced `tool_choice`, and unsupported schemas. Not yet addressed:
//! Anthropic / Gemini / Responses surfaces, streaming tool deltas, and
//! mid-loop history handling. The helper is not called from any handler yet —
//! wiring it in is future work. Response decoding is available via
//! [`decode_text`] for callers that opt in.
//!
//! # IDs
//!
//! The router assigns tool-call IDs. [`to_ir_calls`] numbers decoded calls
//! `call_1`, `call_2`, … in decoded order (deterministic).

use crate::config::GatewayConfig;
use crate::ir::{ChatRequest, FunctionCall, Message, ToolCall, ToolDef};

/// What [`apply_to_request`] decided for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Compact block injected; native `tools` removed.
    Compacted,
    /// Left untouched because the flag is off.
    Disabled,
    /// Left untouched despite the flag (ineligible request or unsupported schema).
    Bypassed,
}

/// Convert router IR tool definitions to the compact crate's owned types.
pub fn from_ir(tools: &[ToolDef]) -> Vec<nasiko_tool_compact::ToolDef> {
    tools
        .iter()
        .map(|t| nasiko_tool_compact::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect()
}

/// Convert decoded compact calls to router IR calls, assigning IDs
/// `call_1`, `call_2`, … in order.
pub fn to_ir_calls(calls: Vec<nasiko_tool_compact::ToolCall>) -> Vec<ToolCall> {
    calls
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let arguments = c.arguments_json();
            ToolCall {
                id: format!("call_{}", i + 1),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: c.name,
                    arguments,
                },
                extra: Default::default(),
            }
        })
        .collect()
}

/// Decode model text into router IR tool calls (router assigns IDs).
/// Returns `Ok(vec![])` for a plain answer with no marker.
pub fn decode_text(
    text: &str,
    tools: &[ToolDef],
) -> Result<Vec<ToolCall>, nasiko_tool_compact::CompactError> {
    let compact_defs = from_ir(tools);
    let calls = nasiko_tool_compact::decode_calls(text, &compact_defs)?;
    Ok(to_ir_calls(calls))
}

/// Replace native `tools` with an injected compact block when enabled and
/// eligible. See the module docs for the bypass conditions.
pub fn apply_to_request(req: &mut ChatRequest, cfg: &GatewayConfig) -> Applied {
    if !cfg.compact_tools_enabled {
        return Applied::Disabled;
    }
    let Some(tools) = req.tools.as_ref() else {
        return Applied::Bypassed;
    };
    if tools.is_empty() {
        return Applied::Bypassed;
    }
    if req.is_streaming() {
        return Applied::Bypassed;
    }
    // Forced tool choice cannot be guaranteed through a text grammar today:
    // bypass rather than risk the model answering in plain text.
    if let Some(choice) = req.tool_choice.as_ref() {
        let forced = match choice {
            serde_json::Value::String(s) => {
                let s = s.trim().to_ascii_lowercase();
                !(s == "auto" || s == "none")
            }
            serde_json::Value::Null => false,
            // Objects like {"type":"function","function":{"name":"…"}} force a call.
            _ => true,
        };
        if forced {
            return Applied::Bypassed;
        }
    }
    let defs = from_ir(tools);
    let compact = match nasiko_tool_compact::encode_tools(&defs) {
        Ok(c) => c,
        Err(_) => return Applied::Bypassed,
    };
    if !compact.compacted {
        return Applied::Bypassed;
    }
    req.tools = None;
    req.tool_choice = None;
    req.messages.push(Message {
        role: "system".into(),
        content: Some(serde_json::Value::String(compact.text)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    Applied::Compacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{FunctionDef, Message};
    use serde_json::{Map, json};

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: enabled,
            ..Default::default()
        }
    }

    fn tool(name: &str) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: Some(format!("Do {name}.")),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"title": {"type": "string"}},
                    "required": ["title"]
                })),
            },
            extra: Map::new(),
        }
    }

    fn req_with_tools() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!("hi")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(vec![tool("search")]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn disabled_mode_is_byte_identical() {
        let mut r = req_with_tools();
        let before = serde_json::to_string(&r).unwrap();
        assert_eq!(apply_to_request(&mut r, &cfg(false)), Applied::Disabled);
        assert_eq!(serde_json::to_string(&r).unwrap(), before);
    }

    #[test]
    fn enabled_compacts_an_eligible_request() {
        let mut r = req_with_tools();
        assert_eq!(apply_to_request(&mut r, &cfg(true)), Applied::Compacted);
        assert!(r.tools.is_none());
        let last = r.messages.last().unwrap();
        assert_eq!(last.role, "system");
        let text = last.content.as_ref().unwrap().as_str().unwrap();
        assert!(text.contains("search("));
        assert!(text.contains("<<call name {json args}>>"));
    }

    #[test]
    fn streaming_requests_bypass() {
        let mut r = req_with_tools();
        r.stream = Some(true);
        assert_eq!(apply_to_request(&mut r, &cfg(true)), Applied::Bypassed);
        assert!(r.tools.is_some());
    }

    #[test]
    fn forced_tool_choice_bypasses() {
        let mut r = req_with_tools();
        r.tool_choice = Some(json!({"type": "function", "function": {"name": "search"}}));
        assert_eq!(apply_to_request(&mut r, &cfg(true)), Applied::Bypassed);
        assert!(r.tools.is_some());
    }

    #[test]
    fn unsupported_schema_bypasses() {
        let mut bad = tool("bad");
        bad.function.parameters = Some(json!({"oneOf": [{"type": "string"}]}));
        let mut r = req_with_tools();
        r.tools = Some(vec![bad]);
        assert_eq!(apply_to_request(&mut r, &cfg(true)), Applied::Bypassed);
        assert!(r.tools.is_some());
    }

    #[test]
    fn decode_text_assigns_deterministic_ids() {
        let tools = vec![tool("search")];
        let calls = decode_text("<<call search {\"title\":\"hi\"}>>", &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "search");
        let v: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(v["title"], json!("hi"));
    }

    #[test]
    fn default_config_keeps_compact_tools_off() {
        assert!(!GatewayConfig::default().compact_tools_enabled);
    }
}
