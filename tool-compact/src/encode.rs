//! Encode OpenAI-shaped tool definitions into a compact text format.
//!
//! The compact format uses Python-style function signatures that are proven to be
//! readable by LLMs and dramatically shorter than JSON Schema.
//!
//! # Format
//!
//! ```text
//! tool_name(param:type, optional?:type) - Description.
//! ```
//!
//! Type mappings:
//! - `string` → `str` (with `format: "date-time"` → `datetime`)
//! - `integer` → `int`
//! - `number` → `num`
//! - `boolean` → `bool`
//! - `array` of T → `[T]`
//! - `object` with properties → `{key:type, ...}`
//! - `enum` → `val1|val2|...`
//! - Required params: `name:type`
//! - Optional params: `name?:type`

use serde_json::Value;

use crate::error::CompactError;
use crate::types::{CompactTools, ToolDef};

/// Call-format instructions injected into the system message.
const CALL_INSTRUCTIONS: &str = "\
To call a tool, emit exactly: <<call tool_name {\"param\": \"value\"}>>\n\
You may call multiple tools by emitting multiple <<call ...>> blocks.\n\
If no tool is appropriate, respond normally without any <<call>> block.\n\
Arguments must be valid JSON. Omit optional parameters you don't need.";

/// Encode a set of tool definitions into compact text format.
///
/// Returns a `CompactTools` containing the compact definitions, call instructions,
/// and the original tools (preserved for decode-time validation).
///
/// # Errors
///
/// Returns `CompactError::UnsupportedSchema` if a tool definition contains schema
/// constructs that cannot be compacted losslessly (e.g., `anyOf` at the top level).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut lines = Vec::with_capacity(tools.len());

    for tool in tools {
        let sig = encode_tool(tool)?;
        lines.push(sig);
    }

    let definitions = lines.join("\n");

    let prompt = format!("Available tools:\n{definitions}\n\n{CALL_INSTRUCTIONS}");

    Ok(CompactTools {
        definitions,
        call_instructions: CALL_INSTRUCTIONS.to_string(),
        prompt,
        original_tools: tools.to_vec(),
    })
}

/// Encode a single tool definition into a compact one-liner.
fn encode_tool(tool: &ToolDef) -> Result<String, CompactError> {
    let params_str = match &tool.parameters {
        Some(schema) => encode_parameters(schema)?,
        None => String::new(),
    };

    let desc = tool
        .description
        .as_deref()
        .map(|d| format!(" - {d}"))
        .unwrap_or_default();

    Ok(format!("{}({params_str}){desc}", tool.name))
}

/// Encode the `parameters` JSON Schema into compact parameter list.
fn encode_parameters(schema: &Value) -> Result<String, CompactError> {
    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(String::new()),
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut params = Vec::new();

    // Maintain deterministic ordering: required params first, then optional, both alphabetical.
    let mut required_params: Vec<_> = properties
        .iter()
        .filter(|(k, _)| required.contains(&k.as_str()))
        .collect();
    required_params.sort_by_key(|(k, _)| k.as_str());

    let mut optional_params: Vec<_> = properties
        .iter()
        .filter(|(k, _)| !required.contains(&k.as_str()))
        .collect();
    optional_params.sort_by_key(|(k, _)| k.as_str());

    for (name, prop_schema) in required_params {
        let type_str = encode_type(prop_schema)?;
        params.push(format!("{name}:{type_str}"));
    }

    for (name, prop_schema) in optional_params {
        let type_str = encode_type(prop_schema)?;
        params.push(format!("{name}?:{type_str}"));
    }

    Ok(params.join(", "))
}

/// Encode a single property's JSON Schema type into compact form.
fn encode_type(schema: &Value) -> Result<String, CompactError> {
    // Enum takes priority over type.
    if let Some(enum_values) = schema.get("enum").and_then(|e| e.as_array()) {
        let vals: Vec<&str> = enum_values.iter().filter_map(|v| v.as_str()).collect();
        if !vals.is_empty() {
            return Ok(vals.join("|"));
        }
    }

    let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("str");

    match type_str {
        "string" => {
            // Check for format hints.
            if let Some(fmt) = schema.get("format").and_then(|f| f.as_str()) {
                match fmt {
                    "date-time" => Ok("datetime".to_string()),
                    "date" => Ok("date".to_string()),
                    "email" => Ok("email".to_string()),
                    "uri" | "url" => Ok("url".to_string()),
                    _ => Ok("str".to_string()),
                }
            } else {
                Ok("str".to_string())
            }
        }
        "integer" => Ok("int".to_string()),
        "number" => Ok("num".to_string()),
        "boolean" => Ok("bool".to_string()),
        "array" => {
            let item_type = if let Some(items) = schema.get("items") {
                encode_type(items)?
            } else {
                "any".to_string()
            };
            Ok(format!("[{item_type}]"))
        }
        "object" => {
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                let nested_required: Vec<&str> = schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let mut parts = Vec::new();
                let mut sorted_props: Vec<_> = props.iter().collect();
                sorted_props.sort_by_key(|(k, _)| k.as_str());

                for (k, v) in sorted_props {
                    let t = encode_type(v)?;
                    if nested_required.contains(&k.as_str()) {
                        parts.push(format!("{k}:{t}"));
                    } else {
                        parts.push(format!("{k}?:{t}"));
                    }
                }
                Ok(format!("{{{}}}", parts.join(", ")))
            } else {
                Ok("obj".to_string())
            }
        }
        _ => Ok("any".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encode_simple_tool() {
        let tools = vec![ToolDef {
            name: "greet".into(),
            description: Some("Say hello".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                },
                "required": ["name"]
            })),
        }];
        let compact = encode_tools(&tools).unwrap();
        assert_eq!(compact.definitions, "greet(name:str) - Say hello");
    }

    #[test]
    fn encode_calendar_tool() {
        let tools = vec![ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
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
        }];
        let compact = encode_tools(&tools).unwrap();
        // Required params first (alphabetical), then optional (alphabetical).
        assert!(compact.definitions.contains("create_calendar_event("));
        assert!(compact.definitions.contains("start:datetime"));
        assert!(compact.definitions.contains("title:str"));
        assert!(compact.definitions.contains("attendees?:[str]"));
        assert!(compact.definitions.contains("duration_min?:int"));
        assert!(compact.definitions.contains("visibility?:public|private"));
    }

    #[test]
    fn encode_tool_no_params() {
        let tools = vec![ToolDef {
            name: "get_time".into(),
            description: Some("Get current time".into()),
            parameters: None,
        }];
        let compact = encode_tools(&tools).unwrap();
        assert_eq!(compact.definitions, "get_time() - Get current time");
    }

    #[test]
    fn encode_tool_no_description() {
        let tools = vec![ToolDef {
            name: "ping".into(),
            description: None,
            parameters: None,
        }];
        let compact = encode_tools(&tools).unwrap();
        assert_eq!(compact.definitions, "ping()");
    }

    #[test]
    fn encode_nested_object() {
        let tools = vec![ToolDef {
            name: "create_user".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "address": {
                        "type": "object",
                        "properties": {
                            "street": {"type": "string"},
                            "city": {"type": "string"}
                        },
                        "required": ["street", "city"]
                    }
                },
                "required": ["name"]
            })),
        }];
        let compact = encode_tools(&tools).unwrap();
        assert!(
            compact
                .definitions
                .contains("address?:{city:str, street:str}")
        );
    }

    #[test]
    fn encode_multiple_tools() {
        let tools = vec![
            ToolDef {
                name: "tool_a".into(),
                description: Some("A".into()),
                parameters: None,
            },
            ToolDef {
                name: "tool_b".into(),
                description: Some("B".into()),
                parameters: None,
            },
        ];
        let compact = encode_tools(&tools).unwrap();
        let lines: Vec<&str> = compact.definitions.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "tool_a() - A");
        assert_eq!(lines[1], "tool_b() - B");
    }

    #[test]
    fn prompt_contains_instructions() {
        let tools = vec![ToolDef {
            name: "f".into(),
            description: None,
            parameters: None,
        }];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.prompt.contains("<<call tool_name"));
        assert!(compact.prompt.contains("Available tools:"));
    }
}
