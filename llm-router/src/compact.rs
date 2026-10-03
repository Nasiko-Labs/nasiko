//! Opt-in compact tool schema transformation (P1).
//!
//! When enabled, this seam replaces JSON Schema tool definitions in the outbound
//! `ChatRequest` with compact one-line signatures injected as a system message,
//! and decodes `<<call ...>>` markers in the model's response back into standard
//! `tool_calls`. The client never sees the compact format.
//!
//! **Off by default.** Enabled via `TOKEN_COMPACT_TOOLS=true` env var and the
//! per-agent `compress_enabled` flag (same switch that gates compression/brevity).

use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};
use serde_json::Value;

/// Outcome of the compact-tools encode step.
#[derive(Debug)]
pub(crate) enum CompactOutcome {
    /// Tools were compacted; the original definitions are stored for decode.
    Applied { original_tools: Vec<ToolDef> },
    /// Compaction was skipped (disabled, no tools, or bypass condition).
    Skipped(SkipReason),
}

#[derive(Debug)]
pub(crate) enum SkipReason {
    Disabled,
    NoTools,
    ToolChoice,
    EncodeFailed,
}

/// Convert router `ToolDef` → tool-compact `ToolDef`.
fn to_compact_tool(t: &ToolDef) -> nasiko_tool_compact::ToolDef {
    nasiko_tool_compact::ToolDef {
        name: t.function.name.clone(),
        description: t.function.description.clone(),
        parameters: t.function.parameters.clone(),
    }
}

/// Encode: strip `tools` from the request, inject compact definitions as a system message.
///
/// Returns the original tools (needed for decode on the response side).
pub(crate) fn encode(req: &mut ChatRequest, enabled: bool) -> CompactOutcome {
    if !enabled {
        return CompactOutcome::Skipped(SkipReason::Disabled);
    }

    let tools = match req.tools.take() {
        Some(t) if !t.is_empty() => t,
        _ => return CompactOutcome::Skipped(SkipReason::NoTools),
    };

    // Bypass when tool_choice forces a specific tool — compaction can't guarantee
    // the model will use the exact <<call>> format when forced.
    if let Some(tc) = &req.tool_choice
        && tc != "auto"
        && tc != "none"
        && !tc.is_null()
    {
        req.tools = Some(tools);
        return CompactOutcome::Skipped(SkipReason::ToolChoice);
    }

    let compact_tools: Vec<nasiko_tool_compact::ToolDef> =
        tools.iter().map(to_compact_tool).collect();

    let compact = match nasiko_tool_compact::encode_tools(&compact_tools) {
        Ok(c) => c,
        Err(_) => {
            req.tools = Some(tools);
            return CompactOutcome::Skipped(SkipReason::EncodeFailed);
        }
    };

    // Inject compact definitions as a leading system message
    req.messages.insert(
        0,
        Message {
            role: "system".to_string(),
            content: Some(Value::String(compact.text)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );

    // Remove tool_choice since we're not using native tool calling
    req.tool_choice = None;

    CompactOutcome::Applied {
        original_tools: tools,
    }
}

/// Decode: parse `<<call ...>>` markers in the response text back into standard `tool_calls`.
///
/// Only runs when `encode` returned `Applied`. Modifies the response in-place.
pub(crate) fn decode(resp: &mut ChatResponse, original_tools: &[ToolDef]) {
    let compact_tools: Vec<nasiko_tool_compact::ToolDef> =
        original_tools.iter().map(to_compact_tool).collect();

    for choice in &mut resp.choices {
        let text = match &choice.message.content {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };

        if !text.contains("<<call ") {
            continue;
        }

        let calls = match nasiko_tool_compact::decode_calls(&text, &compact_tools) {
            Ok(c) if !c.is_empty() => c,
            _ => continue,
        };

        // Convert to router ToolCall format
        let router_calls: Vec<ToolCall> = calls
            .iter()
            .enumerate()
            .map(|(i, c)| ToolCall {
                id: format!("call_{}", i + 1),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                },
                extra: Default::default(),
            })
            .collect();

        // Strip <<call ...>> markers from content, keep surrounding text
        let cleaned = strip_call_markers(&text);
        if cleaned.trim().is_empty() {
            choice.message.content = None;
        } else {
            choice.message.content = Some(Value::String(cleaned));
        }

        choice.message.tool_calls = Some(router_calls);
    }
}

/// Remove `<<call ...>>` markers from text, preserving surrounding content.
fn strip_call_markers(text: &str) -> String {
    let mut result = String::new();
    let mut pos = 0;

    while pos < text.len() {
        if let Some(start) = text[pos..].find("<<call ") {
            let abs_start = pos + start;
            result.push_str(&text[pos..abs_start]);

            // Find closing >>
            if let Some(end) = find_close_marker(&text[abs_start + 7..]) {
                pos = abs_start + 7 + end + 2; // skip past >>
            } else {
                pos = text.len();
            }
        } else {
            result.push_str(&text[pos..]);
            break;
        }
    }

    result
}

fn find_close_marker(text: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escape_next = false;
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }
        match bytes[i] {
            b'\\' if in_string => escape_next = true,
            b'"' => in_string = !in_string,
            b'>' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                return Some(i);
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
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![ToolDef {
            kind: "function".to_string(),
            function: crate::ir::chat::FunctionDef {
                name: "get_weather".to_string(),
                description: Some("Get weather for a city.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    },
                    "required": ["city"]
                })),
            },
            extra: Default::default(),
        }]
    }

    fn sample_request() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("Weather in Tokyo?".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(sample_tools()),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn encode_disabled_is_noop() {
        let mut req = sample_request();
        let result = encode(&mut req, false);
        assert!(matches!(
            result,
            CompactOutcome::Skipped(SkipReason::Disabled)
        ));
        assert!(req.tools.is_some());
    }

    #[test]
    fn encode_strips_tools_and_injects_system_message() {
        let mut req = sample_request();
        let result = encode(&mut req, true);
        assert!(matches!(result, CompactOutcome::Applied { .. }));
        assert!(req.tools.is_none());
        assert_eq!(req.messages[0].role, "system");
        let sys = req.messages[0].content.as_ref().unwrap().as_str().unwrap();
        assert!(sys.contains("get_weather"));
        assert!(sys.contains("<<call"));
    }

    #[test]
    fn flag_off_is_byte_identical() {
        let req_before = serde_json::to_string(&sample_request()).unwrap();
        let mut req = sample_request();
        let _ = encode(&mut req, false);
        let req_after = serde_json::to_string(&req).unwrap();
        assert_eq!(req_before, req_after);
    }

    #[test]
    fn strip_markers() {
        let text = r#"Sure! <<call get_weather {"city":"Tokyo"}>> Hope that helps."#;
        let cleaned = strip_call_markers(text);
        assert_eq!(cleaned, "Sure!  Hope that helps.");
        assert!(!cleaned.contains("<<call"));
    }
}
