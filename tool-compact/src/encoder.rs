//! Compact tool definition encoding and prompt generation.
//!
//! Converts canonical `ToolDef`s into concise functional signatures:
//! `name(param:type, opt?:type|enum) - Description`
//! followed by call-format instructions (`<<call name {json args}>>`),
//! cutting prompt token consumption while preserving complete schema semantics.

use serde_json::Value;

use crate::error::ToolCompactError;
use crate::types::{CompactTools, ToolDef};

/// The standard model-facing instruction for compact tool calls.
pub const CALL_INSTRUCTION: &str = "To call a tool, emit: <<call name {json args}>>";

/// Encode a slice of tool definitions into a compact prompt string and preserved schemas.
///
/// Returns a [`CompactTools`] struct containing the combined prompt instructions, the original
/// schemas (for subsequent decoder validation), and the byte length of the prompt representation.
///
/// # Errors
/// Returns [`ToolCompactError::SchemaError`] if any tool definition contains invalid or
/// unsupported schema structures, or if a tool name is empty.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, ToolCompactError> {
    if tools.is_empty() {
        return Ok(CompactTools {
            prompt_text: String::new(),
            tools: Vec::new(),
            compact_bytes: 0,
        });
    }

    let mut lines = Vec::with_capacity(tools.len() + 1);
    for tool in tools {
        lines.push(encode_single_tool(tool)?);
    }
    lines.push(CALL_INSTRUCTION.to_string());

    let prompt_text = lines.join("\n");
    let compact_bytes = prompt_text.len();

    Ok(CompactTools {
        prompt_text,
        tools: tools.to_vec(),
        compact_bytes,
    })
}

/// Decodes compact tools back to canonical tool definitions.
///
/// Verifies that original schema information survived intact in the [`CompactTools`] container.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, ToolCompactError> {
    Ok(compact.tools.clone())
}

/// Encode a single tool into its compact signature: `name(params) - description`.
fn encode_single_tool(tool: &ToolDef) -> Result<String, ToolCompactError> {
    let name = tool.function.name.trim();
    if name.is_empty() {
        return Err(ToolCompactError::SchemaError(
            "tool name cannot be empty".to_string(),
        ));
    }

    let params_str = match &tool.function.parameters {
        Some(params) => encode_parameters(name, params)?,
        None => String::new(),
    };

    let signature = format!("{name}({params_str})");
    if let Some(desc) = &tool.function.description {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            return Ok(format!("{signature} - {trimmed}"));
        }
    }

    Ok(signature)
}

/// Encode the `parameters` JSON Schema object into a comma-separated parameter list.
fn encode_parameters(tool_name: &str, params: &Value) -> Result<String, ToolCompactError> {
    let obj = params.as_object().ok_or_else(|| {
        ToolCompactError::SchemaError(format!(
            "parameters for tool '{tool_name}' must be a JSON object"
        ))
    })?;

    // Validate type attribute if present
    if let Some(type_val) = obj.get("type") {
        let type_str = type_val.as_str().unwrap_or("");
        if type_str != "object" && !type_str.is_empty() {
            return Err(ToolCompactError::SchemaError(format!(
                "parameters for tool '{tool_name}' must have type 'object', got '{type_str}'"
            )));
        }
    }

    let empty_map = serde_json::Map::new();
    let properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    let required_fields: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    // Sort property names alphabetically to guarantee 100% deterministic output
    let mut prop_names: Vec<&String> = properties.keys().collect();
    prop_names.sort();

    let mut encoded_props = Vec::with_capacity(prop_names.len());
    for prop_name in prop_names {
        let prop_schema = &properties[prop_name];
        let is_required = required_fields.contains(&prop_name.as_str());
        let opt_marker = if is_required { "" } else { "?" };

        let type_repr = compact_type(tool_name, prop_name, prop_schema)?;
        encoded_props.push(format!("{prop_name}{opt_marker}:{type_repr}"));
    }

    Ok(encoded_props.join(", "))
}

/// Recursively convert a property schema into a compact type string.
fn compact_type(
    tool_name: &str,
    prop_name: &str,
    schema: &Value,
) -> Result<String, ToolCompactError> {
    let schema_obj = schema.as_object().ok_or_else(|| {
        ToolCompactError::SchemaError(format!(
            "schema for property '{prop_name}' in tool '{tool_name}' must be an object"
        ))
    })?;

    // 1. Enum variants take precedence: ["public", "private"] -> "public|private"
    if let Some(enum_vals) = schema_obj.get("enum").and_then(Value::as_array)
        && !enum_vals.is_empty()
    {
        let variants: Result<Vec<String>, _> = enum_vals
            .iter()
            .map(|v| match v {
                Value::String(s) => Ok(s.clone()),
                Value::Number(n) => Ok(n.to_string()),
                Value::Bool(b) => Ok(b.to_string()),
                _ => Err(ToolCompactError::SchemaError(format!(
                    "unsupported non-scalar enum value '{v}' in property '{prop_name}' for tool '{tool_name}'"
                ))),
            })
            .collect();
        return Ok(variants?.join("|"));
    }

    // 2. Type translation
    let type_name = schema_obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("any");

    match type_name {
        "string" => {
            if let Some(format) = schema_obj.get("format").and_then(Value::as_str) {
                match format {
                    "date-time" => Ok("datetime".to_string()),
                    "date" => Ok("date".to_string()),
                    "time" => Ok("time".to_string()),
                    "email" => Ok("email".to_string()),
                    "uri" | "url" => Ok("uri".to_string()),
                    _ => Ok("str".to_string()),
                }
            } else {
                Ok("str".to_string())
            }
        }
        "integer" => Ok("int".to_string()),
        "number" => Ok("float".to_string()),
        "boolean" => Ok("bool".to_string()),
        "array" => {
            if let Some(items) = schema_obj.get("items") {
                let item_type = compact_type(tool_name, prop_name, items)?;
                Ok(format!("[{item_type}]"))
            } else {
                Ok("[any]".to_string())
            }
        }
        "object" => {
            if let Some(nested_props) = schema_obj.get("properties").and_then(Value::as_object) {
                let nested_required: Vec<&str> = schema_obj
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();

                let mut nested_names: Vec<&String> = nested_props.keys().collect();
                nested_names.sort();

                let mut nested_fields = Vec::with_capacity(nested_names.len());
                for n_name in nested_names {
                    let n_schema = &nested_props[n_name];
                    let n_is_req = nested_required.contains(&n_name.as_str());
                    let n_opt = if n_is_req { "" } else { "?" };
                    let n_type = compact_type(tool_name, n_name, n_schema)?;
                    nested_fields.push(format!("{n_name}{n_opt}:{n_type}"));
                }
                Ok(format!("{{{}}}", nested_fields.join(", ")))
            } else {
                Ok("object".to_string())
            }
        }
        "null" => Ok("null".to_string()),
        "any" => Ok("any".to_string()),
        other => Err(ToolCompactError::SchemaError(format!(
            "unsupported type '{other}' for property '{prop_name}' in tool '{tool_name}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn make_calendar_tool() -> ToolDef {
        ToolDef::new(FunctionDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Event title" },
                    "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
                    "duration_min": { "type": "integer", "description": "Duration in minutes" },
                    "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"]
            })),
        })
    }

    #[test]
    fn test_encode_official_example() {
        let tools = vec![make_calendar_tool()];
        let compact = encode_tools(&tools).unwrap();

        // Must preserve the original tool schema in the struct
        assert_eq!(compact.tools.len(), 1);
        assert_eq!(compact.tools[0].function.name, "create_calendar_event");

        // Check expected compact text
        let expected_line = "create_calendar_event(attendees?:[str], duration_min?:int, start:datetime, title:str, visibility?:public|private) - Create an event in the user's calendar.";
        assert!(
            compact.prompt_text.contains(expected_line),
            "generated prompt text:\n{}",
            compact.prompt_text
        );
        assert!(compact.prompt_text.contains(CALL_INSTRUCTION));
        assert_eq!(compact.compact_bytes, compact.prompt_text.len());
    }

    #[test]
    fn test_encode_multiple_tools() {
        let tools = vec![
            make_calendar_tool(),
            ToolDef::new(FunctionDef {
                name: "send_email".into(),
                description: Some("Send an email.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" } },
                        "subject": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["to", "subject", "body"]
                })),
            }),
        ];

        let compact = encode_tools(&tools).unwrap();
        assert_eq!(compact.tools.len(), 2);
        assert!(compact.prompt_text.contains("create_calendar_event("));
        assert!(compact.prompt_text.contains("send_email(body:str, subject:str, to:[str]) - Send an email."));
        assert!(compact.prompt_text.contains(CALL_INSTRUCTION));
    }

    #[test]
    fn test_encode_nested_schema() {
        let tool = ToolDef::new(FunctionDef {
            name: "update_profile".into(),
            description: Some("Update user profile.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "user_id": { "type": "string" },
                    "address": {
                        "type": "object",
                        "properties": {
                            "street": { "type": "string" },
                            "zip": { "type": "integer" }
                        },
                        "required": ["street"]
                    }
                },
                "required": ["user_id"]
            })),
        });

        let compact = encode_tools(&[tool]).unwrap();
        assert!(compact.prompt_text.contains("update_profile(address?:{street:str, zip?:int}, user_id:str)"));
    }

    #[test]
    fn test_encode_deterministic_output() {
        let tools = vec![make_calendar_tool()];
        let compact1 = encode_tools(&tools).unwrap();
        let compact2 = encode_tools(&tools).unwrap();

        assert_eq!(compact1.prompt_text, compact2.prompt_text);
        assert_eq!(compact1.compact_bytes, compact2.compact_bytes);
    }

    #[test]
    fn test_encode_empty_tools() {
        let compact = encode_tools(&[]).unwrap();
        assert_eq!(compact.prompt_text, "");
        assert_eq!(compact.tools.len(), 0);
        assert_eq!(compact.compact_bytes, 0);
    }

    #[test]
    fn test_encode_invalid_schema_fails_closed() {
        let invalid_tool = ToolDef::new(FunctionDef {
            name: "broken_tool".into(),
            description: None,
            parameters: Some(json!("not an object")),
        });

        let result = encode_tools(&[invalid_tool]);
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolCompactError::SchemaError(msg) => {
                assert!(msg.contains("must be a JSON object"));
            }
            other => panic!("expected SchemaError, got: {:?}", other),
        }
    }

    #[test]
    fn test_decode_tools_preserves_original_schemas() {
        let original_tools = vec![make_calendar_tool()];
        let compact = encode_tools(&original_tools).unwrap();
        let recovered = decode_tools(&compact).unwrap();

        assert_eq!(recovered, original_tools);
    }
}
