//! Decoder: parse `<<call ToolName {json}>>` markers back into `Vec<ToolCall>`,
//! and round-trip `decode_tools` to reconstruct `Vec<ToolDef>` from compact metadata.
//!
//! # Fail-closed
//!
//! Every decoded call is validated against the original schema. Unknown tools,
//! missing required fields, and invalid enum values all produce explicit errors.

use std::collections::HashMap;

use serde_json::Value;

use crate::error::DecodeError;
use crate::types::{FunctionCall, FunctionDef, ToolCall, ToolDef, ToolSchema};

/// The opening marker for a tool call in compact format.
pub const CALL_OPEN: &str = "<<call ";
/// The closing marker for a tool call in compact format.
pub const CALL_CLOSE: &str = ">>";

/// Decode model output containing `<<call ToolName {json}>>` markers back into
/// OpenAI-shaped [`ToolCall`]s.
///
/// # Errors
///
/// - [`DecodeError::UnknownTool`] if a tool name is not in `tools`.
/// - [`DecodeError::InvalidArguments`] if a required field is missing.
/// - [`DecodeError::InvalidEnumValue`] if an enum value is not in the allowed set.
/// - [`DecodeError::MalformedArguments`] if the JSON inside a marker is invalid.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let schema_map = build_schema_map(tools);
    let raw_calls = extract_raw_calls(text);
    if raw_calls.is_empty() {
        return Err(DecodeError::NoCallsFound);
    }

    let mut result = Vec::with_capacity(raw_calls.len());
    for (idx, (name, args_str)) in raw_calls.into_iter().enumerate() {
        // Validate tool name.
        let schema = schema_map
            .get(name)
            .ok_or_else(|| DecodeError::UnknownTool(name.to_string()))?;

        // Unescape >> inside JSON strings.
        let unescaped = unescape_markers(args_str);

        // Parse JSON.
        let args: Value = serde_json::from_str(&unescaped).map_err(|e| {
            DecodeError::MalformedArguments {
                tool: name.to_string(),
                detail: e.to_string(),
            }
        })?;

        // Validate against schema.
        validate_call(name, &args, schema)?;

        result.push(ToolCall {
            id: format!("call_compact_{idx}"),
            kind: "function".into(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: serde_json::to_string(&args)?,
            },
            extra: serde_json::Map::new(),
        });
    }

    Ok(result)
}

/// Round-trip: reconstruct full `Vec<ToolDef>` from [`crate::CompactTools`] schemas.
///
/// Uses the stored `original_parameters` from encode-time for exact reconstruction.
pub fn decode_tools(schemas: &[ToolSchema]) -> Result<Vec<ToolDef>, DecodeError> {
    let mut tools = Vec::with_capacity(schemas.len());
    for schema in schemas {
        tools.push(ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: schema.name.clone(),
                description: schema.description.clone(),
                parameters: schema.original_parameters.clone(),
            },
            extra: serde_json::Map::new(),
        });
    }
    Ok(tools)
}

/// Extract `(tool_name, json_args_str)` pairs from raw text.
///
/// Finds all `<<call ToolName {json}>>` markers. Handles escaped `>>` inside
/// JSON strings by tracking JSON brace/string depth.
pub(crate) fn extract_raw_calls(text: &str) -> Vec<(&str, &str)> {
    let mut calls = Vec::new();
    let mut search_from = 0;

    while search_from < text.len() {
        // Find the next <<call marker.
        let Some(open_pos) = text[search_from..].find(CALL_OPEN) else {
            break;
        };
        let open_pos = search_from + open_pos;
        let after_marker = open_pos + CALL_OPEN.len();

        // Find tool name (up to the first space or {).
        let rest = &text[after_marker..];
        let name_end = rest
            .find([' ', '{'])
            .unwrap_or(rest.len());
        let tool_name = rest[..name_end].trim();

        if tool_name.is_empty() {
            search_from = after_marker;
            continue;
        }

        // Find the JSON block start.
        let json_rest = &rest[name_end..];
        let Some(json_start_rel) = json_rest.find('{') else {
            search_from = after_marker;
            continue;
        };
        let json_start = after_marker + name_end + json_start_rel;

        // Find matching >> that closes the call, accounting for >> inside JSON strings.
        let Some(close_pos) = find_call_close(&text[json_start..]) else {
            search_from = after_marker;
            continue;
        };
        let json_end = json_start + close_pos;
        let args_str = &text[json_start..json_end];

        calls.push((tool_name, args_str));
        search_from = json_end + CALL_CLOSE.len();
    }

    calls
}

/// Find the position of `>>` that closes a call marker, skipping `>>` inside
/// JSON strings (tracked by brace depth and string quoting).
fn find_call_close(text: &str) -> Option<usize> {
    let mut depth: i32 = 0;
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

        let b = bytes[i];

        if in_string {
            if b == b'\\' {
                escape_next = true;
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
                    // End of JSON object. The >> should follow.
                    let after_json = i + 1;
                    // Skip whitespace between } and >>.
                    let remaining = &text[after_json..];
                    let trimmed_offset = remaining.len() - remaining.trim_start().len();
                    if remaining.trim_start().starts_with(CALL_CLOSE) {
                        return Some(after_json);
                    }
                    // If no >> immediately after, maybe the JSON wasn't the whole call.
                    // Keep scanning.
                    let _ = trimmed_offset;
                }
            }
            _ => {}
        }
        i += 1;
    }

    None
}

/// Unescape `\u003e\u003e` → `>>` and `\u003c\u003ccall` → `<<call` in a JSON
/// string so the decoder can handle escaped markers inside values.
fn unescape_markers(s: &str) -> String {
    s.replace("\\u003e\\u003e", ">>")
        .replace("\\u003c\\u003ccall", "<<call")
        .replace("\\u003E\\u003E", ">>")
        .replace("\\u003C\\u003Ccall", "<<call")
}

/// Validate parsed arguments against the tool's schema.
fn validate_call(tool_name: &str, args: &Value, schema: &InternalSchema) -> Result<(), DecodeError> {
    let obj = args.as_object();

    // Check required fields.
    for field in &schema.required_fields {
        let present = obj
            .map(|o| o.contains_key(field.as_str()))
            .unwrap_or(false);
        if !present {
            return Err(DecodeError::InvalidArguments {
                tool: tool_name.to_string(),
                field: field.clone(),
            });
        }
    }

    // Check enum values.
    if let Some(obj) = obj {
        for (field, allowed) in &schema.enum_values {
            if let Some(val) = obj.get(field.as_str()) {
                let val_str = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if !allowed.contains(&val_str) {
                    return Err(DecodeError::InvalidEnumValue {
                        tool: tool_name.to_string(),
                        field: field.clone(),
                        value: val_str,
                        expected: allowed.clone(),
                    });
                }
            }
        }
    }

    Ok(())
}

/// Lightweight internal schema for validation, built from `ToolDef`.
struct InternalSchema {
    required_fields: Vec<String>,
    enum_values: HashMap<String, Vec<String>>,
}

/// Build a map of tool_name → internal schema from the original tool definitions.
fn build_schema_map(tools: &[ToolDef]) -> HashMap<&str, InternalSchema> {
    let mut map = HashMap::new();
    for tool in tools {
        let name = tool.function.name.as_str();
        let params = tool.function.parameters.as_ref();

        let required_fields: Vec<String> = params
            .and_then(|p| p.get("required"))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let mut enum_values = HashMap::new();
        if let Some(props) = params
            .and_then(|p| p.get("properties"))
            .and_then(Value::as_object)
        {
            for (field_name, prop_schema) in props {
                if let Some(vals) = prop_schema.get("enum").and_then(Value::as_array) {
                    let allowed: Vec<String> = vals
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect();
                    if !allowed.is_empty() {
                        enum_values.insert(field_name.clone(), allowed);
                    }
                }
            }
        }

        map.insert(name, InternalSchema {
            required_fields,
            enum_values,
        });
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn search_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "search".into(),
                description: Some("Search the web.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                })),
            },
            extra: serde_json::Map::new(),
        }
    }

    fn translate_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "translate".into(),
                description: Some("Translate text.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" },
                        "lang": { "type": "string", "enum": ["en", "fr", "de"] }
                    },
                    "required": ["text", "lang"]
                })),
            },
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn decode_single_call() {
        let tools = vec![search_tool()];
        let text = r#"Sure, let me search. <<call search {"query":"rust"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search");
        assert_eq!(calls[0].id, "call_compact_0");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["query"], "rust");
    }

    #[test]
    fn decode_multiple_calls() {
        let tools = vec![search_tool(), translate_tool()];
        let text = r#"<<call search {"query":"hello"}>> and <<call translate {"text":"hello","lang":"fr"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "search");
        assert_eq!(calls[1].function.name, "translate");
        assert_eq!(calls[1].id, "call_compact_1");
    }

    #[test]
    fn decode_unknown_tool_is_error() {
        let tools = vec![search_tool()];
        let text = r#"<<call unknown_tool {"x":"y"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::UnknownTool(name) if name == "unknown_tool"));
    }

    #[test]
    fn decode_missing_required_field_is_error() {
        let tools = vec![search_tool()];
        let text = r#"<<call search {}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidArguments { tool, field } if tool == "search" && field == "query"));
    }

    #[test]
    fn decode_invalid_enum_is_error() {
        let tools = vec![translate_tool()];
        let text = r#"<<call translate {"text":"hi","lang":"xx"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidEnumValue { tool, field, .. } if tool == "translate" && field == "lang"));
    }

    #[test]
    fn decode_malformed_json_is_error() {
        let tools = vec![search_tool()];
        let text = r#"<<call search {not valid json}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::MalformedArguments { .. }));
    }

    #[test]
    fn decode_no_markers_is_error() {
        let tools = vec![search_tool()];
        let text = "Just some regular text with no calls.";
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::NoCallsFound));
    }

    #[test]
    fn decode_handles_escaped_markers_in_json_values() {
        let tools = vec![search_tool()];
        // The JSON value itself contains >> but escaped.
        let text = r#"<<call search {"query":"a \\u003e\\u003e b"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn decode_tools_roundtrip() {
        let original = vec![search_tool(), translate_tool()];
        let compact = crate::encode_tools(&original).unwrap();
        let reconstructed = decode_tools(&compact.schemas).unwrap();
        assert_eq!(reconstructed.len(), 2);
        assert_eq!(reconstructed[0].function.name, "search");
        assert_eq!(reconstructed[1].function.name, "translate");
        // Parameters should be identical.
        assert_eq!(
            reconstructed[0].function.parameters,
            original[0].function.parameters
        );
        assert_eq!(
            reconstructed[1].function.parameters,
            original[1].function.parameters
        );
    }

    #[test]
    fn decode_tools_preserves_descriptions() {
        let original = vec![search_tool()];
        let compact = crate::encode_tools(&original).unwrap();
        let reconstructed = decode_tools(&compact.schemas).unwrap();
        assert_eq!(
            reconstructed[0].function.description,
            original[0].function.description
        );
    }

    #[test]
    fn extract_raw_calls_with_surrounding_text() {
        let text = r#"Here's what I found. <<call search {"query":"test"}>> Let me also do <<call search {"query":"test2"}>>"#;
        let calls = extract_raw_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "search");
        assert_eq!(calls[1].0, "search");
    }
}
