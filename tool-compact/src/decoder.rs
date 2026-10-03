use crate::error::ToolCompactError;
use crate::types::{FunctionCall, ToolCall, ToolDef};
use serde_json::{Map, Value};

/// Validate JSON argument values against the parameter JSON Schema.
pub fn validate_arguments(
    tool_name: &str,
    args: &Value,
    params_schema: Option<&Value>,
) -> Result<(), ToolCompactError> {
    let args_obj = args.as_object().ok_or_else(|| {
        ToolCompactError::InvalidArguments("arguments must be a JSON object".to_string())
    })?;

    let Some(schema) = params_schema else {
        return Ok(());
    };

    // 1. Check required fields
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for req in required {
            if let Some(field) = req.as_str()
                && !args_obj.contains_key(field)
            {
                return Err(ToolCompactError::MissingRequiredField {
                    tool: tool_name.to_string(),
                    field: field.to_string(),
                });
            }
        }
    }

    // 2. Check property constraints (type and enum)
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (field_name, field_val) in args_obj {
            if let Some(prop_schema) = properties.get(field_name) {
                // Check enum
                if let Some(enum_vals) = prop_schema.get("enum").and_then(Value::as_array) {
                    let allowed: Vec<String> = enum_vals
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect();
                    let got = field_val.as_str().unwrap_or("");
                    if !allowed.iter().any(|a| a == got) {
                        return Err(ToolCompactError::InvalidEnumValue {
                            tool: tool_name.to_string(),
                            field: field_name.clone(),
                            allowed,
                            got: got.to_string(),
                        });
                    }
                }

                // Check type
                if let Some(expected_type) = prop_schema.get("type").and_then(Value::as_str) {
                    let matches_type = match expected_type {
                        "string" => field_val.is_string(),
                        "integer" => field_val.is_i64() || field_val.is_u64(),
                        "number" => field_val.is_number(),
                        "boolean" => field_val.is_boolean(),
                        "array" => field_val.is_array(),
                        "object" => field_val.is_object(),
                        _ => true,
                    };

                    if !matches_type {
                        return Err(ToolCompactError::InvalidType {
                            tool: tool_name.to_string(),
                            field: field_name.clone(),
                            expected: expected_type.to_string(),
                            got: format!("{:?}", field_val),
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

/// Extract and parse calls in the format `<<call <name> <json_args>>>`.
pub fn extract_raw_calls(text: &str) -> Result<Vec<(String, String)>, ToolCompactError> {
    let mut results = Vec::new();
    let marker_start = "<<call";
    let mut cursor = 0;

    while let Some(rel_start) = text[cursor..].find(marker_start) {
        let call_start = cursor + rel_start;
        let mut idx = call_start + marker_start.len();

        // Must be followed by whitespace or tool name
        while idx < text.len()
            && text[idx..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace())
        {
            idx += 1;
        }

        // Extract tool name
        let name_start = idx;
        while idx < text.len() {
            let c = text[idx..].chars().next().unwrap();
            if c.is_alphanumeric() || c == '_' || c == '-' {
                idx += c.len_utf8();
            } else {
                break;
            }
        }
        let tool_name = text[name_start..idx].trim().to_string();
        if tool_name.is_empty() {
            cursor = idx;
            continue;
        }

        // Skip whitespace to JSON start
        while idx < text.len()
            && text[idx..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace())
        {
            idx += 1;
        }

        // Scan JSON payload handling strings, escapes and closing >>
        let json_start = idx;
        let mut in_string = false;
        let mut escape = false;
        let mut brace_depth = 0;
        let mut found_end = false;
        let mut json_end = idx;

        let bytes = text.as_bytes();
        while idx < bytes.len() {
            let b = bytes[idx];

            if in_string {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_string = false;
                }
                idx += 1;
                continue;
            }

            if b == b'"' {
                in_string = true;
                idx += 1;
                continue;
            }

            if b == b'{' {
                brace_depth += 1;
            } else if b == b'}' && brace_depth > 0 {
                brace_depth -= 1;
            }

            // Check if we hit the closing >>
            if idx + 1 < bytes.len() && bytes[idx] == b'>' && bytes[idx + 1] == b'>' {
                json_end = idx;
                found_end = true;
                idx += 2;
                break;
            }

            idx += 1;
        }

        if !found_end {
            // Incomplete marker or malformed syntax
            cursor = idx;
            continue;
        }

        let raw_json_str = text[json_start..json_end].trim();
        // Unescape escaped closing markers \>> -> >>
        let unescaped_json = raw_json_str.replace("\\>>", ">>");
        results.push((tool_name, unescaped_json));
        cursor = idx;
    }

    Ok(results)
}

/// Decode tool calls from raw model text, validating against schemas.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, ToolCompactError> {
    let raw_calls = extract_raw_calls(text)?;
    let mut tool_calls = Vec::with_capacity(raw_calls.len());

    for (i, (name, raw_args)) in raw_calls.into_iter().enumerate() {
        // Find tool definition
        let tool_def = tools
            .iter()
            .find(|t| t.function.name == name)
            .ok_or_else(|| ToolCompactError::UnknownTool(name.clone()))?;

        // Parse args
        let args_val: Value = serde_json::from_str(&raw_args)
            .map_err(|e| ToolCompactError::InvalidArguments(format!("JSON parse error: {e}")))?;

        // Validate args
        validate_arguments(&name, &args_val, tool_def.function.parameters.as_ref())?;

        let canonical_arguments = serde_json::to_string(&args_val).unwrap_or(raw_args);

        tool_calls.push(ToolCall {
            id: format!("call_{}", i + 1),
            kind: "function".to_string(),
            function: FunctionCall {
                name,
                arguments: canonical_arguments,
            },
            extra: Map::new(),
        });
    }

    Ok(tool_calls)
}
