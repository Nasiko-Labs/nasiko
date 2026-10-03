//! Call Parser and Extractor for `<<call ...>>` format.

use crate::types::{DecodeError, FunctionCall, ToolCall, ToolDef};
use crate::validate::validate_call;
use serde_json::Map;

/// Decodes model output text into standard OpenAI-compatible tool calls.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut calls = Vec::new();
    let mut cursor = 0;
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut call_counter = 1;

    while cursor < len {
        // Look for "<<" marker
        if let Some(pos) = text[cursor..].find("<<") {
            let start_idx = cursor + pos + 2; // index after "<<"
            let remaining = &text[start_idx..];

            // Parse tool name (skip optional 'call' keyword, whitespace, and colon)
            let trimmed = remaining.trim_start();
            let after_call = if let Some(stripped) = trimmed.strip_prefix("call") {
                stripped.trim_start().trim_start_matches(':').trim_start()
            } else {
                trimmed
            };

            let name_start = len - after_call.len();

            if name_start >= len {
                cursor = start_idx;
                continue;
            }

            // Tool name ends at first whitespace, colon, parenthesis, '{', or '>'
            let mut name_end = name_start;
            while name_end < len {
                let b = bytes[name_end];
                if b.is_ascii_whitespace() || b == b'{' || b == b':' || b == b'(' || b == b'>' {
                    break;
                }
                name_end += 1;
            }

            let mut tool_name = text[name_start..name_end].trim().trim_end_matches(':').trim_end_matches('>');
            if tool_name.is_empty() {
                cursor = start_idx;
                continue;
            }

            // If the model literally output `<<call name <actual_name> ...` or `<<call tool <actual_name>`, skip the descriptor
            if (tool_name.eq_ignore_ascii_case("name") || tool_name.eq_ignore_ascii_case("tool"))
                && !tools.iter().any(|t| t.function.name == tool_name)
            {
                let after_desc = &text[name_end..];
                let trimmed_after = after_desc.trim_start().trim_start_matches(':').trim_start();
                let next_start = name_end + (after_desc.len() - trimmed_after.len());
                let mut next_end = next_start;
                while next_end < len {
                    let b = bytes[next_end];
                    if b.is_ascii_whitespace() || b == b'{' || b == b':' || b == b'(' || b == b'>' {
                        break;
                    }
                    next_end += 1;
                }
                if next_end > next_start {
                    tool_name = text[next_start..next_end].trim().trim_end_matches(':');
                    name_end = next_end;
                }
            }

            // Find start of JSON arguments '{'
            let mut json_start = name_end;
            while json_start < len && bytes[json_start] != b'{' {
                json_start += 1;
            }

            if json_start >= len {
                return Err(DecodeError::MalformedSyntax("missing argument object in call".to_string()));
            }

            // Find matching closing brace and '>>' marker while respecting string escapes
            let mut depth = 0;
            let mut in_string = false;
            let mut escape_next = false;
            let mut json_end = None;
            let mut call_end = len;

            let mut i = json_start;
            while i < len {
                let b = bytes[i];

                if escape_next {
                    escape_next = false;
                    i += 1;
                    continue;
                }

                if b == b'\\' && in_string {
                    escape_next = true;
                    i += 1;
                    continue;
                }

                if b == b'"' {
                    in_string = !in_string;
                    i += 1;
                    continue;
                }

                if !in_string {
                    if b == b'{' {
                        depth += 1;
                    } else if b == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            json_end = Some(i + 1);
                            // Look for closing ">>"
                            let after_brace = &text[i + 1..];
                            if let Some(close_pos) = after_brace.find(">>") {
                                call_end = (i + 1) + close_pos + 2;
                                break;
                            }
                        }
                    }
                }

                i += 1;
            }

            let json_end_idx = json_end.ok_or_else(|| {
                DecodeError::InvalidArguments("unclosed json argument object".to_string())
            })?;

            let raw_args = &text[json_start..json_end_idx];

            // Strict schema and enum validation
            validate_call(tool_name, raw_args, tools)?;

            let id = format!("call_{}", call_counter);
            call_counter += 1;

            let clean_args = crate::validate::sanitize_json(raw_args.trim());

            calls.push(ToolCall {
                id,
                kind: "function".to_string(),
                function: FunctionCall {
                    name: tool_name.to_string(),
                    arguments: clean_args,
                },
                extra: Map::new(),
            });

            cursor = call_end;
        } else {
            break;
        }
    }

    Ok(calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(FunctionDef {
                name: "create_calendar_event".to_string(),
                description: Some("Create calendar event".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string"},
                        "duration_min": {"type": "integer"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            }),
            ToolDef::new(FunctionDef {
                name: "send_email".to_string(),
                description: Some("Send email".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            }),
        ]
    }

    #[test]
    fn test_decode_single_call() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
    }

    #[test]
    fn test_decode_call_with_embedded_angle_brackets_in_string() {
        let tools = sample_tools();
        let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "send_email");
        let parsed_args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed_args["subject"], "a >> b");
    }

    #[test]
    fn test_decode_unknown_tool_fails() {
        let tools = sample_tools();
        let text = r#"<<call delete_everything {}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_error_kind(), "unknown_tool");
    }

    #[test]
    fn test_decode_invalid_enum_fails() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05","visibility":"secret"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_error_kind(), "invalid_arguments");
    }

    #[test]
    fn test_decode_missing_required_field_fails() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"start":"2026-10-05"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_error_kind(), "invalid_arguments");
    }

    #[test]
    fn test_decode_plain_text_no_calls() {
        let tools = sample_tools();
        let text = "Today's weather is sunny and warm.";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_decode_tolerant_colon_and_formatting() {
        let tools = sample_tools();
        let text1 = r#"<<call:create_calendar_event {"title":"Sync","start":"2026-10-05"}>>"#;
        let calls1 = decode_calls(text1, &tools).unwrap();
        assert_eq!(calls1.len(), 1);
        assert_eq!(calls1[0].function.name, "create_calendar_event");

        let text2 = r#"<<call create_calendar_event: {"title":"Sync","start":"2026-10-05"}>>"#;
        let calls2 = decode_calls(text2, &tools).unwrap();
        assert_eq!(calls2.len(), 1);
        assert_eq!(calls2[0].function.name, "create_calendar_event");
    }

    #[test]
    fn test_decode_handles_trailing_commas_gracefully() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":"Sync","start":"2026-10-05",}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let parsed: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed["title"], "Sync");
    }
}

