//! Opt-in compact tool schemas at the egress seam (hackathon P1).
//!
//! Default **off**. When disabled, this module must not mutate the request (byte-identical
//! to the pre-compact-tools path). When enabled, eligible tool schemas are replaced with a
//! compact prompt block; the model emits `<<call …>>` text; we decode back to standard
//! OpenAI `tool_calls` before the response reaches the client.

use nasiko_tool_compact::{self as compact, CompactError};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::config::GatewayConfig;
use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};

/// Why compaction was skipped (telemetry + tests).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    NoTools,
    UnsupportedSchema,
    EncodeFailed,
}

/// Successful application: original tools retained for decode.
#[derive(Debug, Clone)]
pub(crate) struct Applied {
    pub tools: Vec<compact::ToolDef>,
}

pub(crate) type Outcome = Result<Applied, Skipped>;

pub(crate) fn to_metadata(outcome: &Outcome) -> Value {
    match outcome {
        Ok(_) => json!({ "applied": true }),
        Err(reason) => json!({
            "applied": false,
            "skipped": reason.as_label(),
        }),
    }
}

impl Skipped {
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoTools => "no_tools",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::EncodeFailed => "encode_failed",
        }
    }
}

/// Convert IR tools → compact prompt. Mutates `req` only when compaction applies.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Outcome {
    if !cfg.compact_tools_enabled {
        return Err(Skipped::Disabled);
    }
    let Some(ir_tools) = req.tools.as_ref() else {
        return Err(Skipped::NoTools);
    };
    if ir_tools.is_empty() {
        return Err(Skipped::NoTools);
    }

    let tools: Vec<compact::ToolDef> = ir_tools.iter().map(ir_to_compact_def).collect();
    let encoded = match compact::encode_tools(&tools) {
        Ok(c) => c,
        Err(CompactError::UnsupportedSchema(_)) => return Err(Skipped::UnsupportedSchema),
        Err(_) => return Err(Skipped::EncodeFailed),
    };

    // Strip native tools so the provider does not also send full JSON Schema.
    req.tools = None;
    req.tool_choice = Some(Value::String("none".into()));

    req.messages.push(Message {
        role: "system".into(),
        content: Some(Value::String(encoded.prompt)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Map::new(),
    });

    Ok(Applied { tools })
}

/// Rewrite a non-streaming response: decode `<<call>>` markers into standard tool_calls.
pub(crate) fn materialize_response(resp: &mut ChatResponse, applied: &Applied) {
    for choice in &mut resp.choices {
        let msg = &mut choice.message;
        let text = msg.text().unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        match compact::decode_calls(&text, &applied.tools) {
            Ok(calls) if !calls.is_empty() => {
                let ir_calls: Vec<ToolCall> = calls.iter().map(compact_to_ir_call).collect();
                // Strip call markers from visible content; keep any surrounding prose.
                let cleaned = strip_call_markers(&text);
                msg.content = if cleaned.trim().is_empty() {
                    None
                } else {
                    Some(Value::String(cleaned))
                };
                msg.tool_calls = Some(ir_calls);
                choice.finish_reason = Some("tool_calls".into());
            }
            Ok(_) => {
                // No calls — leave content as plain text.
            }
            Err(e) => {
                // Fail closed: do not invent tool calls.
                tracing::warn!(
                    target: "nasiko::llm_router::tool_compact",
                    error = %e,
                    label = e.label(),
                    "compact tool decode failed; returning text without tool_calls"
                );
            }
        }
    }
}

/// Decode accumulated stream text into IR tool calls (for streaming materialization).
#[allow(dead_code)] // reserved for streaming rehydration path
pub(crate) fn decode_stream_text(
    text: &str,
    applied: &Applied,
) -> Result<Vec<ToolCall>, CompactError> {
    let calls = compact::decode_calls(text, &applied.tools)?;
    Ok(calls.iter().map(compact_to_ir_call).collect())
}

fn ir_to_compact_def(t: &ToolDef) -> compact::ToolDef {
    compact::ToolDef {
        name: t.function.name.clone(),
        description: t.function.description.clone(),
        parameters: t.function.parameters.clone(),
    }
}

fn compact_to_ir_call(c: &compact::ToolCall) -> ToolCall {
    let arguments = serde_json::to_string(&c.arguments).unwrap_or_else(|_| "{}".into());
    ToolCall {
        id: format!("call_{}", Uuid::new_v4().simple()),
        kind: "function".into(),
        function: FunctionCall {
            name: c.name.clone(),
            arguments,
        },
        extra: Map::new(),
    }
}

fn strip_call_markers(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(idx) = rest.find(compact::CALL_PREFIX) {
        out.push_str(&rest[..idx]);
        let after = &rest[idx..];
        if let Some(end_rel) = find_call_end(after) {
            rest = &after[end_rel..];
            continue;
        }
        out.push_str(after);
        return out;
    }
    out.push_str(rest);
    out
}

fn find_call_end(s: &str) -> Option<usize> {
    // Best-effort: use the library stream scanner via a throwaway tool list by
    // scanning JSON after the name.
    if !s.starts_with(compact::CALL_PREFIX) {
        return None;
    }
    let after = &s[compact::CALL_PREFIX.len()..];
    let name_end = after.find(|c: char| c.is_whitespace())?;
    let rest = after[name_end..].trim_start();
    if !rest.starts_with('{') {
        return None;
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let after_json = &rest[i + 1..];
                    if after_json.starts_with(compact::CALL_SUFFIX) {
                        let prefix_and_name = compact::CALL_PREFIX.len()
                            + name_end
                            + (after[name_end..].len() - rest.len());
                        return Some(prefix_and_name + i + 1 + compact::CALL_SUFFIX.len());
                    }
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionDef;

    fn sample_req() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("hi".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(vec![ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "send_email".into(),
                    description: Some("Send email".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "to": { "type": "array", "items": { "type": "string" } },
                            "subject": { "type": "string" },
                            "body": { "type": "string" }
                        },
                        "required": ["to", "subject", "body"]
                    })),
                },
                extra: Map::new(),
            }]),
            tool_choice: Some(json!("auto")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn disabled_is_byte_identical() {
        let cfg = GatewayConfig {
            compact_tools_enabled: false,
            ..GatewayConfig::default()
        };
        let original = sample_req();
        let mut req = original.clone();
        let outcome = apply(&mut req, &cfg);
        assert!(matches!(outcome, Err(Skipped::Disabled)));
        let a = serde_json::to_value(&original).unwrap();
        let b = serde_json::to_value(&req).unwrap();
        assert_eq!(a, b, "disabled compact tools must not mutate the request");
    }

    #[test]
    fn enabled_strips_tools_and_injects_prompt() {
        let cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..GatewayConfig::default()
        };
        let mut req = sample_req();
        let outcome = apply(&mut req, &cfg).unwrap();
        assert!(req.tools.is_none());
        assert_eq!(req.tool_choice, Some(json!("none")));
        let last = req.messages.last().unwrap();
        assert_eq!(last.role, "system");
        let text = last.text().unwrap();
        assert!(text.contains("send_email"));
        assert!(text.contains("<<call"));
        assert_eq!(outcome.tools.len(), 1);
    }
}
