use serde_json::{json, Map, Value};

use crate::error::ToolCompactError;
use crate::stream::StreamDecoder;
use crate::types::{CompactTools, FunctionDef, ToolCall, ToolDef};

/// Decode all tool calls in the given model response text, validating against the tool definitions.
///
/// Plain text without calls returns an empty vector. Surrounding text before, between, or after
/// calls is permitted and ignored.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, ToolCompactError> {
    let mut decoder = StreamDecoder::new(tools.to_vec());
    let mut calls = decoder.push_chunk(text)?;
    calls.extend(decoder.finish()?);
    Ok(calls)
}

/// Strip all `<<call ...>>` invocations from model text, returning only conversational content.
pub fn strip_calls(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start_idx) = remaining.find("<<call") {
        result.push_str(&remaining[..start_idx]);
        let after_marker = &remaining[start_idx + "<<call".len()..];
        if let Some(brace_rel) = after_marker.find('{') {
            let args_start = &after_marker[brace_rel..];
            let mut in_string = false;
            let mut escape_next = false;
            let mut brace_depth: usize = 0;
            let mut json_end_byte_idx = None;

            for (byte_idx, ch) in args_start.char_indices() {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                if ch == '\\' && in_string {
                    escape_next = true;
                    continue;
                }
                if ch == '"' {
                    in_string = !in_string;
                    continue;
                }
                if !in_string {
                    if ch == '{' {
                        brace_depth += 1;
                    } else if ch == '}' {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            json_end_byte_idx = Some(byte_idx + ch.len_utf8());
                            break;
                        }
                    }
                }
            }

            if let Some(end_of_json) = json_end_byte_idx {
                let after_json = &args_start[end_of_json..];
                if let Some(arrow_idx) = after_json.find(">>") {
                    let total_call_len = (start_idx + "<<call".len() + brace_rel + end_of_json + arrow_idx + 2) - start_idx;
                    remaining = &remaining[start_idx + total_call_len..];
                    continue;
                }
            }
        }
        // False positive or cut-off marker: advance past "<<call" to prevent infinite loop
        result.push_str("<<call");
        remaining = after_marker;
    }

    result.push_str(remaining);
    result
}

/// Decode compact tool signatures back into standard `ToolDef` schemas for round-trip fidelity verification.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, ToolCompactError> {
    let mut tools = Vec::new();

    for line in compact.signatures.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let open_paren = line
            .find('(')
            .ok_or_else(|| ToolCompactError::ParseError(format!("Missing '(' in line: {line}")))?;
        let name = line[..open_paren].trim();
        if name.is_empty() {
            return Err(ToolCompactError::ParseError(format!(
                "Missing tool name in line: {line}"
            )));
        }

        let close_paren_rel = line[open_paren + 1..]
            .find(')')
            .ok_or_else(|| ToolCompactError::ParseError(format!("Missing ')' in line: {line}")))?;
        let close_paren = open_paren + 1 + close_paren_rel;

        let params_str = line[open_paren + 1..close_paren].trim();
        let rest = line[close_paren + 1..].trim();

        let description = if let Some(stripped) = rest.strip_prefix("- ") {
            Some(stripped.trim().to_string())
        } else if let Some(stripped) = rest.strip_prefix('-') {
            Some(stripped.trim().to_string())
        } else if !rest.is_empty() {
            Some(rest.to_string())
        } else {
            None
        };

        let mut properties = Map::new();
        let mut required = Vec::new();

        if !params_str.is_empty() {
            let params = split_params(params_str);
            for param in params {
                let param = param.trim();
                if param.is_empty() {
                    continue;
                }

                let (pname, is_optional, type_part) = if let Some(idx) = param.find("?:") {
                    (param[..idx].trim(), true, param[idx + 2..].trim())
                } else if let Some(idx) = param.find(':') {
                    (param[..idx].trim(), false, param[idx + 1..].trim())
                } else {
                    return Err(ToolCompactError::ParseError(format!(
                        "Invalid parameter syntax in '{param}'"
                    )));
                };

                let schema = decode_type_schema(type_part);
                properties.insert(pname.to_string(), schema);

                if !is_optional {
                    required.push(pname.to_string());
                }
            }
        }

        let mut params_obj = Map::new();
        params_obj.insert("type".to_string(), Value::String("object".to_string()));
        params_obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            params_obj.insert(
                "required".to_string(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }

        tools.push(ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: name.to_string(),
                description,
                parameters: Some(Value::Object(params_obj)),
            },
        });
    }

    Ok(tools)
}

fn split_params(params_str: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0;

    for ch in params_str.chars() {
        if ch == '[' {
            bracket_depth += 1;
            current.push(ch);
        } else if ch == ']' {
            if bracket_depth > 0 {
                bracket_depth -= 1;
            }
            current.push(ch);
        } else if ch == ',' && bracket_depth == 0 {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(ch);
        }
    }

    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

fn decode_type_schema(type_str: &str) -> Value {
    if type_str.starts_with('[') && type_str.ends_with(']') {
        let inner = &type_str[1..type_str.len() - 1];
        let inner_schema = decode_type_schema(inner);
        json!({
            "type": "array",
            "items": inner_schema
        })
    } else if type_str.contains('|') {
        let enum_vals: Vec<Value> = type_str
            .split('|')
            .map(|s| Value::String(s.trim().to_string()))
            .collect();
        json!({
            "type": "string",
            "enum": enum_vals
        })
    } else {
        match type_str {
            "str" | "string" => json!({"type": "string"}),
            "datetime" | "date-time" => json!({"type": "string", "format": "date-time"}),
            "date" => json!({"type": "string", "format": "date"}),
            "time" => json!({"type": "string", "format": "time"}),
            "email" => json!({"type": "string", "format": "email"}),
            "uri" => json!({"type": "string", "format": "uri"}),
            "int" | "integer" => json!({"type": "integer"}),
            "float" | "number" => json!({"type": "number"}),
            "bool" | "boolean" => json!({"type": "boolean"}),
            "object" => json!({"type": "object"}),
            "null" => json!({"type": "null"}),
            "any" => json!({}),
            other => json!({"type": other}),
        }
    }
}
