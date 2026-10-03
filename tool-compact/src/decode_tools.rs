use crate::error::{CompactError, Result};
use crate::types::{CompactTools, ToolDef};
use serde_json::{Map, Value, json};

/// Parses a `CompactTools` definition string back into standard `ToolDef` schemas.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let mut tools = Vec::new();

    for line in compact.definitions.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let tool = decode_single_tool(trimmed)?;
        tools.push(tool);
    }

    Ok(tools)
}

fn decode_single_tool(line: &str) -> Result<ToolDef> {
    let open_idx = line
        .find('(')
        .ok_or_else(|| CompactError::InvalidSyntax(format!("missing '(' in tool line: {line}")))?;

    let name = line[..open_idx].trim().to_string();
    if name.is_empty() {
        return Err(CompactError::InvalidSyntax("empty tool name".to_string()));
    }

    // Find matching ')'
    let close_idx = find_matching_paren(&line[open_idx..])
        .map(|idx| open_idx + idx)
        .ok_or_else(|| CompactError::InvalidSyntax(format!("unclosed '(' in tool line: {line}")))?;

    let params_str = line[open_idx + 1..close_idx].trim();
    let after_close = line[close_idx + 1..].trim();

    let description = if let Some(stripped) = after_close.strip_prefix('-') {
        let d = stripped.trim();
        if d.is_empty() {
            None
        } else {
            Some(d.to_string())
        }
    } else {
        None
    };

    let parameters = if params_str.is_empty() {
        None
    } else {
        let (properties, required) = parse_properties_list(params_str)?;
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            obj.insert(
                "required".to_string(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
        Some(Value::Object(obj))
    };

    Ok(ToolDef {
        name,
        description,
        parameters,
    })
}

fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_str = false;
    let mut escaped = false;

    for (i, c) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }

    None
}

/// Splits a comma-separated list of properties at depth 0 (ignoring commas inside nested {} [] or quotes).
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut in_str = false;
    let mut escaped = false;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                ',' if depth == 0 => {
                    result.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
    }

    if start < s.len() {
        result.push(&s[start..]);
    }

    result
}

fn parse_properties_list(params_str: &str) -> Result<(Map<String, Value>, Vec<String>)> {
    let mut properties = Map::new();
    let mut required = Vec::new();

    let chunks = split_top_level_commas(params_str);
    for chunk in chunks {
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            continue;
        }

        let colon_idx = trimmed.find(':').ok_or_else(|| {
            CompactError::InvalidSyntax(format!("missing ':' in parameter: {trimmed}"))
        })?;

        let raw_name = trimmed[..colon_idx].trim();
        let (prop_name, is_required) = if let Some(stripped) = raw_name.strip_suffix('?') {
            (stripped.trim().to_string(), false)
        } else {
            (raw_name.to_string(), true)
        };

        if is_required {
            required.push(prop_name.clone());
        }

        let type_and_desc = trimmed[colon_idx + 1..].trim();
        let (type_str, description_opt) = extract_trailing_description(type_and_desc);
        let mut prop_schema = parse_type_schema(type_str)?;

        if let Some(desc) = description_opt
            && let Value::Object(ref mut map) = prop_schema
        {
            map.insert("description".to_string(), Value::String(desc));
        }

        properties.insert(prop_name, prop_schema);
    }

    Ok((properties, required))
}

fn extract_trailing_description(s: &str) -> (&str, Option<String>) {
    let trimmed = s.trim();
    if !trimmed.ends_with('"') {
        return (trimmed, None);
    }

    // Scan backwards from end-1 to find matching unescaped opening quote
    let bytes = trimmed.as_bytes();
    let mut quote_start = None;

    let mut i = bytes.len() - 1;
    while i > 0 {
        i -= 1;
        if bytes[i] == b'"' {
            // Count preceding backslashes
            let mut backslashes = 0;
            let mut j = i;
            while j > 0 && bytes[j - 1] == b'\\' {
                backslashes += 1;
                j -= 1;
            }
            if backslashes % 2 == 0 {
                // This is the unescaped opening quote
                quote_start = Some(i);
                break;
            }
        }
    }

    if let Some(start_idx) = quote_start {
        let type_part = trimmed[..start_idx].trim();
        let desc_raw = &trimmed[start_idx + 1..trimmed.len() - 1];
        let unescaped = desc_raw.replace("\\\"", "\"").replace("\\\\", "\\");
        (type_part, Some(unescaped))
    } else {
        (trimmed, None)
    }
}

fn parse_type_schema(type_str: &str) -> Result<Value> {
    let s = type_str.trim();

    if s == "str" {
        return Ok(json!({ "type": "string" }));
    }

    if s == "datetime" {
        return Ok(json!({ "type": "string", "format": "date-time" }));
    }

    if s == "int" {
        return Ok(json!({ "type": "integer" }));
    }

    if s == "num" {
        return Ok(json!({ "type": "number" }));
    }

    if s == "bool" {
        return Ok(json!({ "type": "boolean" }));
    }

    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let items_schema = parse_type_schema(inner)?;
        return Ok(json!({
            "type": "array",
            "items": items_schema
        }));
    }

    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len() - 1];
        let (properties, required) = parse_properties_list(inner)?;
        let mut obj = Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            obj.insert(
                "required".to_string(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
        return Ok(Value::Object(obj));
    }

    if s.contains('|') {
        let variants: Vec<Value> = s
            .split('|')
            .map(|v| Value::String(v.trim().to_string()))
            .collect();
        return Ok(json!({
            "type": "string",
            "enum": variants
        }));
    }

    Ok(json!({ "type": "string" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode_tools;

    #[test]
    fn test_roundtrip_sample_tools() {
        let original_tools = vec![
            ToolDef {
                name: "create_calendar_event".to_string(),
                description: Some("Create an event in the user's calendar.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "attendees": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Attendee emails"
                        },
                        "duration_min": {
                            "type": "integer",
                            "description": "Duration in minutes"
                        },
                        "start": {
                            "type": "string",
                            "format": "date-time",
                            "description": "Start time, ISO 8601"
                        },
                        "title": {
                            "type": "string",
                            "description": "Event title"
                        },
                        "visibility": {
                            "type": "string",
                            "enum": ["public", "private"]
                        }
                    },
                    "required": ["start", "title"]
                })),
            },
            ToolDef {
                name: "send_email".to_string(),
                description: Some("Send an email from the user's account.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "body": {
                            "type": "string",
                            "description": "Plain-text body"
                        },
                        "cc": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "CC emails"
                        },
                        "subject": {
                            "type": "string",
                            "description": "Subject line"
                        },
                        "to": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Recipient emails"
                        }
                    },
                    "required": ["body", "subject", "to"]
                })),
            },
        ];

        let compact = encode_tools(&original_tools).unwrap();
        let decoded = decode_tools(&compact).unwrap();

        assert_eq!(decoded.len(), original_tools.len());
        for (dec, orig) in decoded.iter().zip(original_tools.iter()) {
            assert_eq!(dec.name, orig.name);
            assert_eq!(dec.description, orig.description);
            assert_eq!(dec.parameters, orig.parameters);
        }
    }
}
