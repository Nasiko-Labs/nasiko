//! Batch decoder converting model-generated compact tool calls into canonical `ToolCall`s.
//!
//! Parses `<<call name {json args}>>` markers from model output, validates arguments
//! against original JSON Schemas, and returns standard OpenAI-shaped `ToolCall` objects.

use serde_json::Value;

use crate::error::ToolCompactError;
use crate::types::{ToolCall, ToolDef};
use crate::validator::validate_call_arguments;

pub(crate) const CALL_PREFIX: &str = "<<call";
pub(crate) const CALL_SUFFIX: &str = ">>";

/// Decode compact tool calls from model output text.
///
/// # Behavior
/// - Scans the input text for `<<call name {json args}>>` markers.
/// - Allows conversational prose before, between, and after tool calls.
/// - Correctly parses multiple tool calls in a single response.
/// - Safely handles escaped characters and `>>` inside JSON string literals.
/// - Validates tool existence and argument constraints (types, required fields, enums).
/// - Fails closed: any unknown tool, malformed call, or invalid argument immediately returns an error.
/// - If no tool calls are present, returns an empty vector `Ok(vec![])`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, ToolCompactError> {
    let mut calls = Vec::new();
    let mut cursor = 0;
    let mut call_counter = 1;

    while let Some(rel_start) = text[cursor..].find(CALL_PREFIX) {
        let call_start = cursor + rel_start;
        let after_prefix = call_start + CALL_PREFIX.len();

        // Marker must be followed by whitespace (e.g. "<<call " or "<<call\n")
        let rem = &text[after_prefix..];
        let trimmed_rem = rem.trim_start();
        if trimmed_rem.is_empty() {
            return Err(ToolCompactError::MalformedSyntax(
                "incomplete '<<call' marker at end of output".to_string(),
            ));
        }

        if rem.len() == trimmed_rem.len() {
            // Not a tool call marker (e.g., "<<call_something"), advance past prefix
            cursor = after_prefix;
            continue;
        }

        // 1. Parse tool name
        let ws_offset = rem.len() - trimmed_rem.len();
        let name_start = after_prefix + ws_offset;
        let remaining_after_ws = trimmed_rem;
        let name_end_rel = remaining_after_ws
            .find(|c: char| c.is_whitespace() || c == '{')
            .ok_or_else(|| {
                ToolCompactError::MalformedSyntax("unterminated tool call marker".to_string())
            })?;

        let tool_name = remaining_after_ws[..name_end_rel].trim();
        if tool_name.is_empty() {
            return Err(ToolCompactError::MalformedSyntax(
                "missing tool name in '<<call' marker".to_string(),
            ));
        }

        // 2. Verify tool exists in toolset (Fail closed on unknown tool)
        let tool = tools
            .iter()
            .find(|t| t.function.name == tool_name)
            .ok_or_else(|| ToolCompactError::UnknownTool(tool_name.to_string()))?;

        // 3. Locate opening brace of JSON arguments
        let after_name = &remaining_after_ws[name_end_rel..];
        let open_brace_rel = after_name.find('{').ok_or_else(|| {
            ToolCompactError::MalformedSyntax(format!(
                "expected '{{' for arguments of tool '{tool_name}'"
            ))
        })?;

        let json_start = name_start + name_end_rel + open_brace_rel;

        // 4. Extract balanced JSON object respecting string escapes and internal '>>'
        let (json_str, json_end) = extract_balanced_json(&text[json_start..])?;
        let abs_json_end = json_start + json_end;

        // 5. Verify closing marker ">>"
        let after_json = &text[abs_json_end..];
        let trimmed_after = after_json.trim_start();
        if !trimmed_after.starts_with(CALL_SUFFIX) {
            return Err(ToolCompactError::MalformedSyntax(format!(
                "expected '>>' to close tool call for '{tool_name}'"
            )));
        }

        let suffix_offset = after_json.len() - trimmed_after.len() + CALL_SUFFIX.len();
        cursor = abs_json_end + suffix_offset;

        // 6. Parse and validate JSON arguments
        let args_val: Value = serde_json::from_str(json_str).map_err(|e| {
            ToolCompactError::InvalidArguments {
                tool: tool_name.to_string(),
                details: format!("malformed JSON arguments: {e}"),
            }
        })?;

        validate_call_arguments(tool, &args_val)?;

        // 7. Format into OpenAI-compliant ToolCall
        let call_id = format!("call_{call_counter}");
        call_counter += 1;

        let arguments_str = serde_json::to_string(&args_val).unwrap_or_else(|_| "{}".to_string());
        calls.push(ToolCall::new(call_id, tool_name, arguments_str));
    }

    Ok(calls)
}

/// Extract a balanced JSON object `{ ... }` starting from the first `{`.
///
/// Respects string literals and escape sequences so characters like `}` or `>>`
/// inside strings do not prematurely terminate the balance scanner.
pub(crate) fn extract_balanced_json(slice: &str) -> Result<(&str, usize), ToolCompactError> {
    if !slice.starts_with('{') {
        return Err(ToolCompactError::MalformedSyntax(
            "arguments must begin with '{'".to_string(),
        ));
    }

    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escape = false;

    for (idx, ch) in slice.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        match ch {
            '\\' if in_string => {
                escape = true;
            }
            '"' => {
                in_string = !in_string;
            }
            '{' if !in_string => {
                depth += 1;
            }
            '}' if !in_string => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    ToolCompactError::MalformedSyntax("unbalanced '}' in arguments".to_string())
                })?;

                if depth == 0 {
                    let end_pos = idx + ch.len_utf8();
                    return Ok((&slice[..end_pos], end_pos));
                }
            }
            _ => {}
        }
    }

    Err(ToolCompactError::MalformedSyntax(
        "unclosed JSON object in tool call arguments".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn test_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(FunctionDef {
                name: "search".into(),
                description: Some("Search the web".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer" }
                    },
                    "required": ["query"]
                })),
            }),
            ToolDef::new(FunctionDef {
                name: "ping".into(),
                description: Some("Ping check".into()),
                parameters: None,
            }),
        ]
    }

    #[test]
    fn test_decode_no_calls() {
        let tools = test_tools();
        let res = decode_calls("Here is the answer without any tools.", &tools).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn test_decode_single_call() {
        let tools = test_tools();
        let text = r#"I will search: <<call search {"query": "rust lang", "limit": 5}>> Done."#;
        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "search");
        let parsed_args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed_args["query"], "rust lang");
        assert_eq!(parsed_args["limit"], 5);
    }

    #[test]
    fn test_decode_multiple_calls() {
        let tools = test_tools();
        let text = r#"First: <<call search {"query": "rust"}>> and then: <<call ping {}>>"#;
        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "search");
        assert_eq!(calls[1].id, "call_2");
        assert_eq!(calls[1].function.name, "ping");
    }

    #[test]
    fn test_decode_escaping_with_closing_delimiter_in_string() {
        let tools = test_tools();
        let text = r#"<<call search {"query": "a >> b with \"quotes\""}>>"#;
        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 1);
        let parsed_args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed_args["query"], "a >> b with \"quotes\"");
    }

    #[test]
    fn test_decode_unknown_tool() {
        let tools = test_tools();
        let text = r#"<<call unknown_function {"foo": "bar"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        match err {
            ToolCompactError::UnknownTool(name) => assert_eq!(name, "unknown_function"),
            other => panic!("expected UnknownTool, got {:?}", other),
        }
    }

    #[test]
    fn test_decode_missing_required_argument() {
        let tools = test_tools();
        let text = r#"<<call search {"limit": 10}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        match err {
            ToolCompactError::MissingRequiredField { tool, field } => {
                assert_eq!(tool, "search");
                assert_eq!(field, "query");
            }
            other => panic!("expected MissingRequiredField, got {:?}", other),
        }
    }

    #[test]
    fn test_decode_malformed_syntax() {
        let tools = test_tools();
        let text = r#"<<call search {"query": "test""#; // unclosed JSON and no >>
        let err = decode_calls(text, &tools).unwrap_err();
        match err {
            ToolCompactError::MalformedSyntax(_) => {}
            other => panic!("expected MalformedSyntax, got {:?}", other),
        }
    }
}
