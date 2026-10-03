//! TOON (Token-Oriented Object Notation) Tool Definition Encoder.

use crate::types::{CompactTools, EncodeError, ToolDef};
use serde_json::Value;

pub const DEFAULT_CALL_INSTRUCTIONS: &str = "Use <<call: tool_name {json}>> to invoke tools.";

/// Encodes a list of tool definitions into the high-density TOON compact format.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    let mut lines = Vec::with_capacity(tools.len());
    for tool in tools {
        lines.push(encode_single_tool(tool)?);
    }

    Ok(CompactTools {
        definitions: lines.join("\n"),
        instructions: DEFAULT_CALL_INSTRUCTIONS.to_string(),
    })
}

fn encode_single_tool(tool: &ToolDef) -> Result<String, EncodeError> {
    let fn_def = &tool.function;
    let name = &fn_def.name;

    let params_str = if let Some(params) = &fn_def.parameters {
        encode_parameters(params)?
    } else {
        String::new()
    };

    let sig = format!("{}({})", name, params_str);

    if let Some(desc) = &fn_def.description {
        let clean_desc = desc.trim();
        if !clean_desc.is_empty() {
            Ok(format!("{} - {}", sig, clean_desc))
        } else {
            Ok(sig)
        }
    } else {
        Ok(sig)
    }
}

fn encode_parameters(params: &Value) -> Result<String, EncodeError> {
    let obj = match params {
        Value::Object(map) => map,
        _ => return Ok(String::new()),
    };

    let empty_vec = Vec::new();
    let required_fields: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or(empty_vec);

    let properties = match obj.get("properties").and_then(Value::as_object) {
        Some(props) => props,
        None => return Ok(String::new()),
    };

    let mut param_parts = Vec::new();

    // 1. Emit required fields first, following the order in `required` array
    for &req_field in &required_fields {
        if let Some(prop_val) = properties.get(req_field) {
            let type_repr = encode_type(prop_val)?;
            param_parts.push(format!("{}:{}", req_field, type_repr));
        }
    }

    // 2. Emit remaining (optional) fields
    for (prop_name, prop_val) in properties {
        if !required_fields.contains(&prop_name.as_str()) {
            let type_repr = encode_type(prop_val)?;
            param_parts.push(format!("{}?:{}", prop_name, type_repr));
        }
    }

    Ok(param_parts.join(","))
}

pub fn encode_type(prop_val: &Value) -> Result<String, EncodeError> {
    let obj = match prop_val {
        Value::Object(map) => map,
        _ => return Ok("any".to_string()),
    };

    // Check enum first
    if let Some(variants) = obj.get("enum").and_then(Value::as_array) {
        let variant_strs: Vec<String> = variants
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        if !variant_strs.is_empty() {
            return Ok(variant_strs.join("|"));
        }
    }

    let type_str = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("any");

    match type_str {
        "string" => {
            if let Some(format) = obj.get("format").and_then(Value::as_str) {
                if format == "date-time" || format == "datetime" {
                    return Ok("datetime".to_string());
                }
                if format == "date" {
                    return Ok("date".to_string());
                }
                if format == "uri" || format == "url" {
                    return Ok("uri".to_string());
                }
                if format == "email" {
                    return Ok("email".to_string());
                }
            }
            Ok("str".to_string())
        }
        "integer" => Ok("int".to_string()),
        "number" => Ok("float".to_string()),
        "boolean" => Ok("bool".to_string()),
        "array" => {
            if let Some(items) = obj.get("items") {
                let inner_type = encode_type(items)?;
                Ok(format!("[{}]", inner_type))
            } else {
                Ok("[any]".to_string())
            }
        }
        "object" => {
            if let Some(props) = obj.get("properties").and_then(Value::as_object) {
                let mut inner_parts = Vec::new();
                for (k, v) in props {
                    let inner_t = encode_type(v)?;
                    inner_parts.push(format!("{}:{}", k, inner_t));
                }
                Ok(format!("{{{}}}", inner_parts.join(",")))
            } else {
                Ok("object".to_string())
            }
        }
        other => Ok(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    #[test]
    fn test_encode_calendar_tool() {
        let tool = ToolDef::new(FunctionDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        });

        let compact = encode_tools(&[tool]).unwrap();
        assert_eq!(
            compact.definitions,
            "create_calendar_event(title:str,start:datetime,attendees?:[str],duration_min?:int,visibility?:public|private) - Create an event in the user's calendar."
        );
    }
}
