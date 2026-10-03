//! Parse `<<call NAME JSON>>` markers from model text.

use serde_json::Value;

use crate::error::{CompactError, Result};
use crate::types::{ToolCall, ToolDef};
use crate::validate::{find_tool, validate_arguments};

/// Prefix of every compact tool call.
pub const CALL_PREFIX: &str = "<<call ";
/// Suffix closing a compact tool call.
pub const CALL_SUFFIX: &str = ">>";

/// Decode all complete tool calls from `text`, validating against `tools`.
///
/// Plain text with no markers → empty `Ok(vec![])`.
/// Any unknown tool / invalid args / malformed marker → `Err` (fail-closed).
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut offset = 0;
    while let Some(rel) = text[offset..].find(CALL_PREFIX) {
        let start = offset + rel;
        let (call, end) = parse_one_call(&text[start..], tools)?;
        calls.push(call);
        offset = start + end;
    }
    Ok(calls)
}

/// Parse a single call starting at `<<call `. Returns `(call, bytes_consumed)`.
pub(crate) fn parse_one_call(input: &str, tools: &[ToolDef]) -> Result<(ToolCall, usize)> {
    if !input.starts_with(CALL_PREFIX) {
        return Err(CompactError::MalformedCall(
            "call must start with <<call ".into(),
        ));
    }
    let after_prefix = &input[CALL_PREFIX.len()..];
    let name_end = after_prefix
        .find(|c: char| c.is_whitespace())
        .ok_or_else(|| CompactError::MalformedCall("missing tool name".into()))?;
    if name_end == 0 {
        return Err(CompactError::MalformedCall("empty tool name".into()));
    }
    let name = &after_prefix[..name_end];
    let rest = after_prefix[name_end..].trim_start();
    if !rest.starts_with('{') {
        return Err(CompactError::MalformedCall(
            "arguments must be a JSON object".into(),
        ));
    }

    let json_len = scan_json_object(rest)?;
    let json_str = &rest[..json_len];
    let after_json = &rest[json_len..];
    let suffix_ws = trailing_ws_len(after_json);
    let after_ws = &after_json[suffix_ws..];
    if !after_ws.starts_with(CALL_SUFFIX) {
        return Err(CompactError::MalformedCall(
            "call must end with >> after the JSON object".into(),
        ));
    }

    let arguments: Value =
        serde_json::from_str(json_str).map_err(|e| CompactError::InvalidJson(e.to_string()))?;

    let tool = find_tool(tools, name)?;
    validate_arguments(tool, &arguments)?;

    let ws = after_prefix[name_end..].len() - rest.len();
    let consumed = CALL_PREFIX.len() + name_end + ws + json_len + suffix_ws + CALL_SUFFIX.len();

    Ok((
        ToolCall {
            name: name.to_string(),
            arguments,
        },
        consumed,
    ))
}

/// Length of leading ASCII whitespace (space/tab/CR/LF) before `>>`.
fn trailing_ws_len(s: &str) -> usize {
    s.chars()
        .take_while(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
        .map(|c| c.len_utf8())
        .sum()
}

/// Scan a JSON object starting at `{`, respecting strings/escapes. Returns byte length.
pub(crate) fn scan_json_object(input: &str) -> Result<usize> {
    let bytes = input.as_bytes();
    if bytes.first() != Some(&b'{') {
        return Err(CompactError::MalformedCall("expected JSON object".into()));
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0;
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
            // Only `{}` depth matters for object termination; `[`/`]` are ignored here
            // (they cannot close an object, and nested objects still use `}`).
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i + 1);
                }
                if depth < 0 {
                    return Err(CompactError::MalformedCall("unbalanced JSON braces".into()));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(CompactError::MalformedCall("incomplete JSON object".into()))
}

/// Try to parse a complete call from the start of `buf` if one is fully present.
///
/// Returns:
/// - `Ok(Some((call, end)))` when a complete valid call was parsed
/// - `Ok(None)` when more input is needed (incomplete marker/JSON)
/// - `Err` when the buffer contains a definitively malformed / invalid call
pub(crate) fn try_parse_complete(
    buf: &str,
    tools: &[ToolDef],
) -> Result<Option<(ToolCall, usize)>> {
    let Some(start) = buf.find(CALL_PREFIX) else {
        return Ok(None);
    };
    // Ignore leading text; only attempt from the marker.
    let slice = &buf[start..];
    if slice.len() < CALL_PREFIX.len() + 1 {
        return Ok(None);
    }
    let after_prefix = &slice[CALL_PREFIX.len()..];
    let Some(name_end) = after_prefix.find(|c: char| c.is_whitespace()) else {
        // Still reading the name, or name runs to EOF without space yet.
        if after_prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Ok(None);
        }
        return Err(CompactError::MalformedCall(
            "invalid character in tool name".into(),
        ));
    };
    if name_end == 0 {
        return Err(CompactError::MalformedCall("empty tool name".into()));
    }
    let name = &after_prefix[..name_end];
    let rest = after_prefix[name_end..].trim_start();
    if rest.is_empty() {
        return Ok(None);
    }
    if !rest.starts_with('{') {
        return Err(CompactError::MalformedCall(
            "arguments must be a JSON object".into(),
        ));
    }
    match scan_json_object(rest) {
        Err(CompactError::MalformedCall(msg)) if msg.contains("incomplete") => Ok(None),
        Err(e) => Err(e),
        Ok(json_len) => {
            let after_json = &rest[json_len..];
            let suffix_ws = trailing_ws_len(after_json);
            let after_ws = &after_json[suffix_ws..];
            if after_ws.is_empty() {
                return Ok(None);
            }
            if after_ws == ">" {
                // Partial `>>`
                return Ok(None);
            }
            if !after_ws.starts_with(CALL_SUFFIX) {
                return Err(CompactError::MalformedCall(
                    "call must end with >> after the JSON object".into(),
                ));
            }
            let json_str = &rest[..json_len];
            let arguments: Value = serde_json::from_str(json_str)
                .map_err(|e| CompactError::InvalidJson(e.to_string()))?;
            let tool = find_tool(tools, name)?;
            validate_arguments(tool, &arguments)?;
            let consumed = start
                + CALL_PREFIX.len()
                + name_end
                + (after_prefix[name_end..].len() - rest.len())
                + json_len
                + suffix_ws
                + CALL_SUFFIX.len();
            Ok(Some((
                ToolCall {
                    name: name.to_string(),
                    arguments,
                },
                consumed,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::ToolDef;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string", "format": "date-time" },
                        "visibility": { "type": "string", "enum": ["public", "private"] }
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
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
        ]
    }

    #[test]
    fn no_call() {
        let calls = decode_calls("The weather is sunny today.", &tools()).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn single_call() {
        let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
    }

    #[test]
    fn gtgt_inside_string() {
        let text =
            r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls[0].arguments["subject"], json!("a >> b"));
    }

    #[test]
    fn unknown_tool() {
        let err = decode_calls("<<call delete_everything {}>>", &tools()).unwrap_err();
        assert_eq!(err.label(), "unknown_tool");
    }

    #[test]
    fn invalid_enum_and_missing() {
        let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let err = decode_calls(text, &tools()).unwrap_err();
        assert_eq!(err.label(), "invalid_arguments");
    }

    #[test]
    fn text_around_calls() {
        let text = "I will create the event now.\n\n<<call create_calendar_event {\"title\":\"T\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>\nDone.";
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn whitespace_before_closing_marker() {
        let text = "<<call create_calendar_event {\"title\":\"T\",\"start\":\"2026-10-05T15:00:00+05:30\"} \n>>";
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn multiple_calls() {
        let text = r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b"}>><<call create_calendar_event {"title":"T","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }

    #[test]
    fn escaped_quotes_in_json() {
        let text = r#"<<call send_email {"to":["a@b.c"],"subject":"say \"hi\"","body":"x"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls[0].arguments["subject"], json!("say \"hi\""));
    }
}
