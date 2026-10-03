//! Parse compact tool call markers from LLM response text.
//!
//! Grammar:
//! ```text
//! <<call TOOL_NAME JSON_OBJECT>>
//! ```
//!
//! Multiple calls and surrounding text are supported.
//! The `>>` closing delimiter is found only outside JSON strings (bracket-depth
//! and string-escape tracking), so `>>` embedded inside string arguments is
//! handled correctly.

use serde_json::Value;

use crate::error::DecodeError;
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate_args;

const OPEN: &str = "<<call ";
const CLOSE: &str = ">>";

/// Decode all `<<call ...>>` markers in `text`, validating each against `tools`.
///
/// - Returns `Ok(calls)` where `calls` may be empty (plain text, no calls).
/// - Returns the first `Err` encountered (unknown tool, invalid args, malformed).
/// - Text outside markers is silently discarded.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut calls = Vec::new();
    let mut remaining = text;

    while let Some(open_pos) = remaining.find(OPEN) {
        let after_open = &remaining[open_pos + OPEN.len()..];

        // Find the end of the JSON argument object, respecting string contents.
        let close_pos = find_close(after_open)?;

        let marker_body = &after_open[..close_pos];
        // marker_body = "TOOL_NAME JSON_OBJECT" (single space separator)

        let call = parse_marker_body(marker_body, tools)?;
        calls.push(call);

        // Advance past `>>`.
        remaining = &after_open[close_pos + CLOSE.len()..];
    }

    Ok(calls)
}

/// Find the position of `>>` that closes the current call marker.
///
/// This state machine:
/// - Tracks JSON bracket depth (`{`, `[`)
/// - Tracks JSON string state (handles `\"` escapes)
/// - Only recognizes `>>` when depth == 0 and not inside a string
///
/// Returns the byte position of `>>` within `s`, or an error.
fn find_close(s: &str) -> Result<usize, DecodeError> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        let b = bytes[i];

        if in_string {
            match b {
                b'\\' => {
                    escape_next = true;
                }
                b'"' => {
                    in_string = false;
                }
                _ => {}
            }
            i += 1;
            continue;
        }

        // Not in string.
        match b {
            b'"' => {
                in_string = true;
            }
            b'{' | b'[' => {
                depth += 1;
            }
            b'}' | b']' => {
                depth -= 1;
            }
            b'>' if depth == 0 && i + 1 < len && bytes[i + 1] == b'>' => {
                return Ok(i);
            }
            _ => {}
        }

        i += 1;
    }

    Err(DecodeError::MalformedCall {
        detail: "missing closing >>".to_string(),
    })
}

/// Parse `"TOOL_NAME JSON_OBJECT"` and validate against tools.
fn parse_marker_body(body: &str, tools: &[ToolDef]) -> Result<ToolCall, DecodeError> {
    // Split on first space to get tool name and json part.
    let (name, json_part) = body
        .split_once(' ')
        .ok_or_else(|| DecodeError::MalformedCall {
            detail: "call marker has no space between tool name and arguments".to_string(),
        })?;

    let name = name.trim().to_string();
    let json_part = json_part.trim();

    // Lookup tool definition.
    let tool = tools
        .iter()
        .find(|t| t.function.name == name)
        .ok_or_else(|| DecodeError::UnknownTool { name: name.clone() })?;

    // Parse the JSON arguments.
    let args: Value = serde_json::from_str(json_part).map_err(|e| DecodeError::MalformedCall {
        detail: format!("invalid JSON arguments for '{}': {}", name, e),
    })?;

    // Validate the arguments against the schema.
    validate_args(tool, &args)?;

    Ok(ToolCall {
        name,
        arguments: args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDef, ToolDef};
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string", "format": "date-time" },
                        "duration_min": { "type": "integer" },
                        "visibility": { "type": "string", "enum": ["public", "private"] }
                    },
                    "required": ["title", "start"]
                })),
            },
        }
    }

    #[test]
    fn single_valid_call() {
        let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let tools = [calendar_tool()];
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Design review");
    }

    #[test]
    fn plain_text_no_calls() {
        let text = "Sure, I can help with that!";
        let tools = [calendar_tool()];
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 0);
    }

    #[test]
    fn text_around_call() {
        let text = r#"Sure! <<call create_calendar_event {"title":"Meeting","start":"2026-10-05T09:00:00Z"}>> Done."#;
        let tools = [calendar_tool()];
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["title"], "Meeting");
    }

    #[test]
    fn unknown_tool_error() {
        let text = r#"<<call delete_everything {}>>"#;
        let tools = [calendar_tool()];
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::UnknownTool { .. }));
    }

    #[test]
    fn missing_required_field_error() {
        let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#;
        let tools = [calendar_tool()];
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidArguments { .. }));
    }

    #[test]
    fn invalid_enum_value_error() {
        let text = r#"<<call create_calendar_event {"title":"T","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let tools = [calendar_tool()];
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidArguments { .. }));
    }

    #[test]
    fn double_angle_inside_string_is_safe() {
        let email_tool = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: None,
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
        };

        let text =
            r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
        let tools = [email_tool];
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["subject"], "a >> b");
    }

    #[test]
    fn malformed_no_closing_marker() {
        let text = r#"<<call create_calendar_event {"title":"T","start":"S"}"#;
        let tools = [calendar_tool()];
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::MalformedCall { .. }));
    }
}
