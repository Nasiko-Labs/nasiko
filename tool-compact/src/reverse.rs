//! Bidirectional TOON Parser: Reconstructs standard ToolDef JSON Schemas from compact definitions.

use crate::types::{CompactTools, DecodeError, FunctionDef, ToolDef};
use serde_json::{json, Map, Value};

/// Reconstructs full `ToolDef` schemas from compact TOON definitions.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, DecodeError> {
    let mut tools = Vec::new();

    for line in compact.definitions.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (sig_part, desc_part) = if let Some((sig, desc)) = trimmed.split_once(" - ") {
            (sig.trim(), Some(desc.trim().to_string()))
        } else {
            (trimmed, None)
        };

        let open_paren = sig_part.find('(').ok_or_else(|| {
            DecodeError::MalformedSyntax(format!("missing '(' in signature: {}", sig_part))
        })?;
        let close_paren = sig_part.rfind(')').ok_or_else(|| {
            DecodeError::MalformedSyntax(format!("missing ')' in signature: {}", sig_part))
        })?;

        let name = sig_part[..open_paren].trim().to_string();
        let params_content = sig_part[open_paren + 1..close_paren].trim();

        let (properties, required) = parse_params(params_content)?;

        let mut parameters_obj = Map::new();
        parameters_obj.insert("type".to_string(), json!("object"));
        parameters_obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            parameters_obj.insert("required".to_string(), json!(required));
        }

        tools.push(ToolDef::new(FunctionDef {
            name,
            description: desc_part,
            parameters: Some(Value::Object(parameters_obj)),
        }));
    }

    Ok(tools)
}

fn parse_params(params_str: &str) -> Result<(Map<String, Value>, Vec<String>), DecodeError> {
    let mut properties = Map::new();
    let mut required = Vec::new();

    if params_str.is_empty() {
        return Ok((properties, required));
    }

    // Split parameters by top-level commas (avoiding nested commas inside {} or [])
    let param_tokens = split_top_level(params_str, ',')?;

    for token in param_tokens {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }

        let colon_pos = trimmed.find(':').ok_or_else(|| {
            DecodeError::MalformedSyntax(format!("missing ':' in parameter: {}", trimmed))
        })?;

        let name_part = trimmed[..colon_pos].trim();
        let type_part = trimmed[colon_pos + 1..].trim();

        let (prop_name, is_optional) = if let Some(stripped) = name_part.strip_suffix('?') {
            (stripped.trim(), true)
        } else {
            (name_part, false)
        };

        if !is_optional {
            required.push(prop_name.to_string());
        }

        let type_schema = parse_type_str(type_part)?;
        properties.insert(prop_name.to_string(), type_schema);
    }

    Ok((properties, required))
}

fn parse_type_str(type_str: &str) -> Result<Value, DecodeError> {
    let t = type_str.trim();

    // Enum check: a|b|c
    if t.contains('|') && !t.starts_with('[') && !t.starts_with('{') {
        let variants: Vec<&str> = t.split('|').map(str::trim).collect();
        return Ok(json!({
            "type": "string",
            "enum": variants
        }));
    }

    // Array check: [inner]
    if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let item_schema = parse_type_str(inner.trim())?;
        return Ok(json!({
            "type": "array",
            "items": item_schema
        }));
    }

    // Nested object check: {k:v,...}
    if let Some(inner) = t.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let (props, req) = parse_params(inner.trim())?;
        let mut obj = Map::new();
        obj.insert("type".to_string(), json!("object"));
        obj.insert("properties".to_string(), Value::Object(props));
        if !req.is_empty() {
            obj.insert("required".to_string(), json!(req));
        }
        return Ok(Value::Object(obj));
    }

    match t {
        "str" | "string" => Ok(json!({"type": "string"})),
        "datetime" => Ok(json!({"type": "string", "format": "date-time"})),
        "date" => Ok(json!({"type": "string", "format": "date"})),
        "email" => Ok(json!({"type": "string", "format": "email"})),
        "uri" | "url" => Ok(json!({"type": "string", "format": "uri"})),
        "int" | "integer" => Ok(json!({"type": "integer"})),
        "float" | "number" => Ok(json!({"type": "number"})),
        "bool" | "boolean" => Ok(json!({"type": "boolean"})),
        _ => Ok(json!({"type": "string"})),
    }
}

fn split_top_level(s: &str, delimiter: char) -> Result<Vec<String>, DecodeError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth_paren = 0;
    let mut depth_brace = 0;
    let mut depth_bracket = 0;

    for ch in s.chars() {
        match ch {
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '{' => depth_brace += 1,
            '}' => depth_brace -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            _ => {}
        }

        if ch == delimiter && depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 {
            tokens.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(ch);
        }
    }

    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode_tools;

    #[test]
    fn test_bidirectional_roundtrip() {
        let original_tools = vec![
            ToolDef::new(FunctionDef {
                name: "create_calendar_event".to_string(),
                description: Some("Create calendar event".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
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
        ];

        let compact = encode_tools(&original_tools).unwrap();
        let decoded = decode_tools(&compact).unwrap();

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].function.name, "create_calendar_event");
        assert_eq!(decoded[0].function.description, Some("Create calendar event".to_string()));

        let params = decoded[0].function.parameters.as_ref().unwrap();
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&json!("title")));
        assert!(required.contains(&json!("start")));
        assert_eq!(params["properties"]["visibility"]["enum"], json!(["public", "private"]));
    }
}
