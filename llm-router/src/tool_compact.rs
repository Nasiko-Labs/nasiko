//! Opt-in compact tool-schema encoding at the router seam (P1 bonus integration).
//!
//! When [`crate::config::GatewayConfig::compact_tools_enabled`] is `false` (the
//! default), [`maybe_compact_request`] returns the request untouched — existing
//! behaviour is byte-identical. When enabled, tool definitions are replaced by
//! compact signature lines plus a `<<call …>>` calling convention injected as a
//! leading `system` message, and [`decode_assistant_calls`] turns the model's reply
//! text back into IR [`ToolCall`](crate::ir::ToolCall)s.
//!
//! The pure encoding/decoding lives in `nasiko-tool-compact`, which knows nothing
//! about the router; this module only converts at the seam.

use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::{ChatRequest, FunctionCall, Message, ToolCall, ToolDef};

/// Whether compact tool encoding is switched on.
pub fn is_enabled(cfg: &GatewayConfig) -> bool {
    cfg.compact_tools_enabled
}

fn to_compact_tool(t: &ToolDef) -> nasiko_tool_compact::ToolDef {
    nasiko_tool_compact::ToolDef::new(
        t.function.name.clone(),
        t.function.description.clone(),
        t.function.parameters.clone(),
    )
}

/// Rewrite the request with compact tool schemas when the flag is on.
///
/// Returns the request unchanged when the flag is off, when it carries no tools,
/// or when any tool's schema is outside the supported subset (bypass) — the
/// native `tools` array is then the honest representation.
pub fn maybe_compact_request(cfg: &GatewayConfig, mut req: ChatRequest) -> ChatRequest {
    if !is_enabled(cfg) {
        return req;
    }
    let tools = match req.tools.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return req,
    };
    let compact_tools: Vec<nasiko_tool_compact::ToolDef> =
        tools.iter().map(to_compact_tool).collect();
    let encoded = match nasiko_tool_compact::encode_tools(&compact_tools) {
        Ok(e) if e.compacted.iter().all(|c| *c) => e,
        _ => return req,
    };
    req.tools = None;
    req.tool_choice = None;
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    messages.push(Message {
        role: "system".to_string(),
        content: Some(Value::String(encoded.instructions)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    messages.extend(req.messages.drain(..));
    req.messages = messages;
    req
}

/// Decode `<<call …>>` markers from an assistant message's text into IR tool calls.
///
/// Call ids are minted as `compact-call-<n>` — the compact wire format carries no
/// ids, so the seam assigns stable positional ones.
pub fn decode_assistant_calls(
    text: &str,
    tools: &[ToolDef],
) -> Result<Vec<ToolCall>, nasiko_tool_compact::Error> {
    let compact_tools: Vec<nasiko_tool_compact::ToolDef> =
        tools.iter().map(to_compact_tool).collect();
    let calls = nasiko_tool_compact::decode_calls(text, &compact_tools)?;
    Ok(calls
        .into_iter()
        .enumerate()
        .map(|(i, c)| ToolCall {
            id: format!("compact-call-{i}"),
            kind: "function".to_string(),
            function: FunctionCall {
                name: c.name,
                arguments: serde_json::to_string(&c.arguments).unwrap_or_default(),
            },
            extra: Default::default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionDef;
    use serde_json::json;

    fn cfg_on() -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: true,
            ..GatewayConfig::default()
        }
    }

    fn tool() -> ToolDef {
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "send_email".to_string(),
                description: Some("Send an email.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                    },
                    "required": ["to", "subject"]
                })),
            },
            extra: Default::default(),
        }
    }

    fn request() -> ChatRequest {
        ChatRequest {
            model: None,
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(Value::String("hi".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![tool()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_by_default() {
        assert!(!GatewayConfig::default().compact_tools_enabled);
    }

    #[test]
    fn disabled_leaves_request_untouched() {
        let req = request();
        let before = serde_json::to_string(&req).unwrap();
        let after = maybe_compact_request(&GatewayConfig::default(), req);
        assert_eq!(before, serde_json::to_string(&after).unwrap());
    }

    #[test]
    fn enabled_rewrites_tools_to_signatures() {
        let out = maybe_compact_request(&cfg_on(), request());
        assert!(out.tools.is_none());
        assert_eq!(out.messages.len(), 2);
        let sys = out.messages[0].text().unwrap();
        assert!(sys.contains("send_email(subject:str, to:[str])"));
        assert!(sys.contains("<<call tool_name"));
    }

    #[test]
    fn enabled_bypasses_unsupported_schema() {
        let mut req = request();
        req.tools.as_mut().unwrap()[0].function.parameters =
            Some(json!({"type": "object", "properties": {"x": {"oneOf": [{"type": "string"}]}}}));
        let before = serde_json::to_string(&req).unwrap();
        let after = maybe_compact_request(&cfg_on(), req);
        assert_eq!(before, serde_json::to_string(&after).unwrap());
    }

    #[test]
    fn decode_round_trip() {
        let text = r#"Sure <<call send_email {"to":["a@b.c"],"subject":"hi"}>> done"#;
        let calls = decode_assistant_calls(text, &[tool()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "compact-call-0");
        assert_eq!(calls[0].function.name, "send_email");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["subject"], json!("hi"));
    }

    #[test]
    fn decode_unknown_tool_errors() {
        let err = decode_assistant_calls(r#"<<call nope {}>>"#, &[tool()]).unwrap_err();
        assert_eq!(err.code(), "unknown_tool");
    }
}
