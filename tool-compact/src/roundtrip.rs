//! Reverse-decode compact text back to `ToolDef` for schema preservation checks.

use crate::error::CompactError;
use crate::types::ToolDef;
use serde_json::{Map, Value, json};

/// Decode compact tool text back to full `ToolDef` definitions.
///
/// This allows automated verification that schema information survives
/// the compact encoding (required vs optional, types, enums, nested objects).
pub fn decode_tools(compact_text: &str) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools = Vec::new();

    for line in compact_text.lines() {
        let line = line.trim();
        // Skip non-tool lines (instructions, headers, blank lines)
        if line.is_empty()
            || line.starts_with("Tools:")
            || line.starts_with("Available tools:")
            || line.starts_with("Call:")
            || line.starts_with("To call")
            || line.starts_with("Multiple")
            || line.starts_with("If no tool")
        {
            continue;
        }

        // A tool line looks like: name(params) - Description
        if let Some(tool) = parse_tool_line(line) {
            tools.push(tool);
        }
    }

    Ok(tools)
}

fn parse_tool_line(line: &str) -> Option<ToolDef> {
    // Find the opening paren
    let paren_open = line.find('(')?;
    let name = line[..paren_open].trim().to_string();

    // Find the matching closing paren
    let paren_close = find_matching_paren(line, paren_open)?;

    let params_str = &line[paren_open + 1..paren_close];

    // Description is after ") - "
    let description = line[paren_close + 1..]
        .trim()
        .strip_prefix("- ")
        .or_else(|| line[paren_close + 1..].trim().strip_prefix("— "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let parameters = if params_str.trim().is_empty() {
        None
    } else {
        Some(parse_params_to_schema(params_str))
    };

    Some(ToolDef {
        name,
        description,
        parameters,
    })
}

fn find_matching_paren(text: &str, open_pos: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, ch) in text[open_pos..].char_indices() {
        match ch {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open_pos + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_params_to_schema(params_str: &str) -> Value {
    let params = split_params(params_str);
    let mut properties = Map::new();
    let mut required = Vec::new();

    for param in &params {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }

        // Split on first ':' — name?:type or name:type
        let (name_part, type_str) = match param.find(':') {
            Some(pos) => (&param[..pos], param[pos + 1..].trim()),
            None => continue,
        };

        let is_optional = name_part.ends_with('?');
        let name = if is_optional {
            &name_part[..name_part.len() - 1]
        } else {
            name_part
        };
        let name = name.trim();

        if !is_optional {
            required.push(Value::String(name.to_string()));
        }

        let schema = type_str_to_schema(type_str);
        properties.insert(name.to_string(), schema);
    }

    let mut result = json!({
        "type": "object",
        "properties": properties,
    });
    if !required.is_empty() {
        result["required"] = Value::Array(required);
    }
    result
}

/// Split top-level params by comma, respecting nested brackets.
fn split_params(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for ch in s.chars() {
        match ch {
            '(' | '{' | '[' => {
                depth += 1;
                current.push(ch);
            }
            ')' | '}' | ']' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn type_str_to_schema(type_str: &str) -> Value {
    let type_str = type_str.trim();

    // Array: [inner_type]
    if type_str.starts_with('[') && type_str.ends_with(']') {
        let inner = &type_str[1..type_str.len() - 1];
        return json!({
            "type": "array",
            "items": type_str_to_schema(inner)
        });
    }

    // Object: {key:type, key?:type}
    if type_str.starts_with('{') && type_str.ends_with('}') {
        let inner = &type_str[1..type_str.len() - 1];
        return parse_params_to_schema(inner);
    }

    // Enum: val1|val2|val3
    if type_str.contains('|') && !type_str.contains(':') {
        let variants: Vec<Value> = type_str
            .split('|')
            .map(|v| Value::String(v.trim().to_string()))
            .collect();
        return json!({
            "type": "string",
            "enum": variants
        });
    }

    // Primitive types
    match type_str {
        "str" => json!({"type": "string"}),
        "int" => json!({"type": "integer"}),
        "float" => json!({"type": "number"}),
        "bool" => json!({"type": "boolean"}),
        "dt" | "datetime" => json!({"type": "string", "format": "date-time"}),
        "any" => json!({}),
        _ => json!({"type": "string"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode_tools;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    #[test]
    fn roundtrip_schema_preservation() {
        let tools = vec![calendar_tool()];
        let compact = encode_tools(&tools).unwrap();
        let decoded = decode_tools(&compact.text).unwrap();

        assert_eq!(decoded.len(), 1);
        let d = &decoded[0];
        assert_eq!(d.name, "create_calendar_event");
        // Description may be dropped when tool name is self-describing

        let params = d.parameters.as_ref().unwrap();
        assert_eq!(params["type"], "object");

        let props = params["properties"].as_object().unwrap();
        assert!(props.contains_key("title"));
        assert!(props.contains_key("start"));
        assert!(props.contains_key("duration_min"));
        assert!(props.contains_key("attendees"));
        assert!(props.contains_key("visibility"));

        // Check types survived
        assert_eq!(props["title"]["type"], "string");
        assert_eq!(props["start"]["type"], "string");
        assert_eq!(props["start"]["format"], "date-time");
        assert_eq!(props["duration_min"]["type"], "integer");
        assert_eq!(props["attendees"]["type"], "array");
        assert_eq!(props["attendees"]["items"]["type"], "string");

        // Check enum survived
        let vis_enum = props["visibility"]["enum"].as_array().unwrap();
        assert!(vis_enum.contains(&json!("public")));
        assert!(vis_enum.contains(&json!("private")));

        // Check required survived
        let req = params["required"].as_array().unwrap();
        assert!(req.contains(&json!("title")));
        assert!(req.contains(&json!("start")));
        assert!(!req.contains(&json!("duration_min")));
    }

    #[test]
    fn roundtrip_no_params() {
        let tool = ToolDef {
            name: "get_time".to_string(),
            description: Some("Get current time.".to_string()),
            parameters: None,
        };
        let compact = encode_tools(&[tool]).unwrap();
        let decoded = decode_tools(&compact.text).unwrap();

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "get_time");
        assert!(decoded[0].parameters.is_none());
    }

    #[test]
    fn roundtrip_multiple_tools() {
        let tools = vec![
            calendar_tool(),
            ToolDef {
                name: "send_email".to_string(),
                description: Some("Send an email.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        ];
        let compact = encode_tools(&tools).unwrap();
        let decoded = decode_tools(&compact.text).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].name, "create_calendar_event");
        assert_eq!(decoded[1].name, "send_email");
    }
}
