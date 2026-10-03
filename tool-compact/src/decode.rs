//! Decode compact `<<call ...>>` markers back to standard tool calls.

use crate::error::CompactError;
use crate::types::{ToolCall, ToolDef};
use serde_json::Value;
use std::collections::HashMap;

const CALL_OPEN: &str = "<<call ";
const CALL_CLOSE: &str = ">>";

/// Decode all `<<call TOOL {args}>>` markers in text into validated tool calls.
///
/// Also accepts `<<TOOL {args}>>` (without the `call` keyword) for robustness —
/// some models drop the keyword but produce an otherwise valid call.
///
/// Returns an error on unknown tools, missing required fields, or invalid arguments.
/// Text outside markers is ignored (the model may produce natural language alongside calls).
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let tool_map: HashMap<&str, &ToolDef> = tools.iter().map(|t| (t.name.as_str(), t)).collect();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let raw_calls = extract_raw_calls(text, &tool_names)?;

    let mut result = Vec::new();
    for (name, args_str) in raw_calls {
        let tool = tool_map
            .get(name.as_str())
            .ok_or_else(|| CompactError::UnknownTool(name.clone()))?;

        let args: Value = serde_json::from_str(&args_str).map_err(|e| {
            CompactError::MalformedCall(format!("invalid JSON arguments for `{name}`: {e}"))
        })?;

        validate_args(tool, &args)?;

        result.push(ToolCall {
            name,
            arguments: args_str,
        });
    }

    Ok(result)
}

/// Extract raw (name, args_json) pairs from text.
///
/// Recognizes two patterns:
/// 1. `<<call TOOL_NAME {args}>>` — canonical format
/// 2. `<<TOOL_NAME {args}>>` — fallback for models that drop the `call` keyword
fn extract_raw_calls(
    text: &str,
    tool_names: &[&str],
) -> Result<Vec<(String, String)>, CompactError> {
    let mut calls = Vec::new();
    let mut search_from = 0;

    while search_from < text.len() {
        // Try canonical `<<call ` first, then fallback `<<tool_name `
        let (_open_pos, after_open) = match find_call_open(&text[search_from..], tool_names) {
            Some((pos, content_start)) => (search_from + pos, search_from + content_start),
            None => break,
        };

        // Find the matching >> that closes this call.
        let close_pos = find_closing_marker(&text[after_open..])
            .map(|p| after_open + p)
            .ok_or_else(|| CompactError::MalformedCall("unclosed <<call marker".to_string()))?;

        let inner = &text[after_open..close_pos];

        // Split into tool name and JSON args
        let (name, args_str) = parse_call_inner(inner)?;
        calls.push((name, args_str));

        search_from = close_pos + CALL_CLOSE.len();
    }

    Ok(calls)
}

/// Find the next call opening marker. Returns (position, bytes_to_skip_past_open).
///
/// Tries `<<call ` first (canonical). If not found, looks for `<<KNOWN_TOOL ` or
/// `<<KNOWN_TOOL{` as a fallback for models that drop the `call` keyword.
fn find_call_open(text: &str, tool_names: &[&str]) -> Option<(usize, usize)> {
    // Canonical: <<call NAME — always preferred
    if let Some(pos) = text.find(CALL_OPEN) {
        return Some((pos, pos + CALL_OPEN.len()));
    }

    // Fallback: <<TOOL_NAME{ or <<TOOL_NAME  (without "call" keyword)
    // Only reached when no canonical <<call  exists in the remaining text.
    let mut best: Option<(usize, usize)> = None;
    for name in tool_names {
        let pattern = format!("<<{name}");
        if let Some(pos) = text.find(&pattern) {
            let after = pos + pattern.len();
            if after < text.len() {
                let next_ch = text.as_bytes()[after];
                if next_ch == b' ' || next_ch == b'{' || next_ch == b'\t' {
                    let skip = pos + 2; // skip past '<<', leave tool name for parse_call_inner
                    if best.is_none() || best.is_some_and(|b| pos < b.0) {
                        best = Some((pos, skip));
                    }
                }
            }
        }
    }
    best
}

/// Find the closing `>>` that is not inside a JSON string.
fn find_closing_marker(text: &str) -> Option<usize> {
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
            b'\\' if in_string => {
                escape_next = true;
            }
            b'"' => {
                in_string = !in_string;
            }
            b'>' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }

    None
}

/// Parse the inner content of a `<<call ...>>` marker into (name, json_args).
fn parse_call_inner(inner: &str) -> Result<(String, String), CompactError> {
    let inner = inner.trim();

    // Find the start of the JSON object
    let brace_pos = inner.find('{').ok_or_else(|| {
        // Could be a no-args call
        if inner
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return CompactError::MalformedCall(format!(
                "tool `{inner}` called without arguments — use empty object {{}} if no args needed"
            ));
        }
        CompactError::MalformedCall(format!("cannot find JSON arguments in: {inner}"))
    })?;

    let name = inner[..brace_pos].trim().to_string();
    if name.is_empty() {
        return Err(CompactError::MalformedCall(
            "empty tool name in <<call>>".to_string(),
        ));
    }

    let args_str = inner[brace_pos..].trim().to_string();
    Ok((name, args_str))
}

/// Validate arguments against the tool's JSON Schema parameters.
fn validate_args(tool: &ToolDef, args: &Value) -> Result<(), CompactError> {
    let schema = match &tool.parameters {
        Some(s) => s,
        None => {
            // No schema means no args expected; accept empty object
            if args.as_object().is_some_and(|m| m.is_empty()) {
                return Ok(());
            }
            return Ok(()); // Permissive if no schema defined
        }
    };

    let obj = args
        .as_object()
        .ok_or_else(|| CompactError::InvalidArgument {
            tool: tool.name.clone(),
            field: "(root)".to_string(),
            reason: "arguments must be a JSON object".to_string(),
        })?;

    // Check required fields
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for req in required {
            if let Some(field_name) = req.as_str()
                && !obj.contains_key(field_name)
            {
                return Err(CompactError::MissingRequired {
                    tool: tool.name.clone(),
                    field: field_name.to_string(),
                });
            }
        }
    }

    // Check enum constraints
    if let Some(Value::Object(props)) = schema.get("properties") {
        for (key, val) in obj {
            if let Some(prop_schema) = props.get(key) {
                validate_field_value(&tool.name, key, val, prop_schema)?;
            }
            // Unknown fields are allowed (permissive)
        }
    }

    Ok(())
}

fn validate_field_value(
    tool_name: &str,
    field: &str,
    value: &Value,
    schema: &Value,
) -> Result<(), CompactError> {
    // Enum check
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        let matches = allowed.iter().any(|a| a == value);
        if !matches {
            return Err(CompactError::InvalidArgument {
                tool: tool_name.to_string(),
                field: field.to_string(),
                reason: format!(
                    "value {} not in allowed enum {:?}",
                    value,
                    allowed
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                ),
            });
        }
    }

    // Type check
    if let Some(type_str) = schema.get("type").and_then(Value::as_str) {
        let type_ok = match type_str {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !type_ok {
            return Err(CompactError::InvalidArgument {
                tool: tool_name.to_string(),
                field: field.to_string(),
                reason: format!("expected type `{type_str}`, got {}", value_type_name(value)),
            });
        }
    }

    Ok(())
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    fn email_tool() -> ToolDef {
        ToolDef {
            name: "send_email".to_string(),
            description: Some("Send an email.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"},
                    "cc": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["to", "subject", "body"]
            })),
        }
    }

    #[test]
    fn decode_single_call() {
        let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>""#;
        let calls = decode_calls(text, &[calendar_tool()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
    }

    #[test]
    fn decode_multiple_calls() {
        let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>"#;
        let calls = decode_calls(text, &[calendar_tool(), email_tool()]).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }

    #[test]
    fn decode_with_surrounding_text() {
        let text = r#"Sure, I'll create that event for you.
<<call create_calendar_event {"title":"Standup","start":"2026-10-05T09:00:00+05:30"}>>
Done!"#;
        let calls = decode_calls(text, &[calendar_tool()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
    }

    #[test]
    fn decode_no_calls() {
        let text = "I don't have access to a calendar tool.";
        let calls = decode_calls(text, &[calendar_tool()]).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn decode_unknown_tool() {
        let text = r#"<<call delete_everything {"confirm": true}>>"#;
        let result = decode_calls(text, &[calendar_tool()]);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CompactError::UnknownTool(_)));
    }

    #[test]
    fn decode_missing_required() {
        let text = r#"<<call create_calendar_event {"duration_min": 30}>>"#;
        let result = decode_calls(text, &[calendar_tool()]);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CompactError::MissingRequired { .. }
        ));
    }

    #[test]
    fn decode_invalid_enum() {
        let text = r#"<<call create_calendar_event {"title":"Test","start":"2026-10-05","visibility":"secret"}>>"#;
        let result = decode_calls(text, &[calendar_tool()]);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CompactError::InvalidArgument { .. }
        ));
    }

    #[test]
    fn decode_wrong_type() {
        let text = r#"<<call create_calendar_event {"title":"Test","start":"2026-10-05","duration_min":"thirty"}>>"#;
        let result = decode_calls(text, &[calendar_tool()]);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CompactError::InvalidArgument { .. }
        ));
    }

    #[test]
    fn decode_close_marker_inside_string() {
        let text = r#"<<call create_calendar_event {"title":"Meeting >> Discussion","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &[calendar_tool()]).unwrap();
        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["title"], "Meeting >> Discussion");
    }

    #[test]
    fn decode_without_call_keyword() {
        // GPT-5.6 Luna style: <<tool_name {args}>> without "call"
        let text = r#"<<send_email {"to":["sam@example.com"],"subject":"Build is green","body":"The build is green."}>>"#;
        let calls = decode_calls(text, &[calendar_tool(), email_tool()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
    }

    #[test]
    fn decode_mixed_call_and_no_call_keyword() {
        // Mix of canonical and fallback format
        let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"Hi","body":"Hello"}>>
<<create_calendar_event {"title":"Retro","start":"2026-10-03T10:00:00+05:30","duration_min":30,"visibility":"private"}>>"#;
        let calls = decode_calls(text, &[calendar_tool(), email_tool()]).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }
}
