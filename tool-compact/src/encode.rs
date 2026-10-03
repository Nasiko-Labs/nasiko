use crate::error::{CompactError, Result};
use crate::grammar::{CLOSE, OPEN};
use crate::schema::is_schema_supported;
use crate::types::{CompactTools, ToolDef};
use serde_json::Value;

/// Encodes a list of `ToolDef`s into a compact representation.
/// If any tool has an unsupported schema keyword, returns `CompactError::UnsupportedSchema`.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut definitions = Vec::with_capacity(tools.len());

    for tool in tools {
        let def_str = encode_single_tool(tool)?;
        definitions.push(def_str);
    }

    let definitions_rendered = definitions.join("\n");
    // Every word here is paid on every request, so it states only what models get
    // wrong without it: act rather than ask, and one marker per requested action.
    // `render` puts it first, so it also introduces the definitions that follow.
    let instructions = format!(
        "Tools; per requested action emit {OPEN}name {{json args}}{CLOSE} without asking, else reply normally:"
    );

    Ok(CompactTools {
        definitions: definitions_rendered,
        instructions,
    })
}

fn encode_single_tool(tool: &ToolDef) -> Result<String> {
    let mut sig = format!("{}(", tool.name);

    if let Some(params) = &tool.parameters {
        // Validate support first
        if let Err(reason) = is_schema_supported(params) {
            return Err(CompactError::UnsupportedSchema {
                tool: tool.name.clone(),
                reason,
            });
        }

        let params_str = encode_object_properties(params)?;
        sig.push_str(&params_str);
    }

    sig.push(')');

    if let Some(desc) = &tool.description {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            let collapsed = collapse_whitespace(trimmed);
            sig.push_str(": ");
            sig.push_str(&collapsed);
        }
    }

    Ok(sig)
}

fn encode_object_properties(schema: &Value) -> Result<String> {
    let obj = match schema {
        Value::Object(map) => map,
        _ => return Ok(String::new()),
    };

    let properties = match obj.get("properties") {
        Some(Value::Object(props)) => props,
        _ => return Ok(String::new()),
    };

    let required_set: Vec<&str> = obj
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut parts = Vec::new();

    for (prop_name, prop_schema) in properties {
        let is_required = required_set.contains(&prop_name.as_str());
        let type_str = encode_type(prop_schema)?;
        let opt_mark = if is_required { "" } else { "?" };

        let desc_str = if let Some(desc) = prop_schema.get("description").and_then(|d| d.as_str()) {
            let collapsed = collapse_whitespace(desc);
            format!(" ({collapsed})")
        } else {
            String::new()
        };

        parts.push(format!("{prop_name}{opt_mark}:{type_str}{desc_str}"));
    }

    Ok(parts.join(", "))
}

fn encode_type(schema: &Value) -> Result<String> {
    let obj = match schema {
        Value::Object(map) => map,
        _ => return Ok("str".to_string()),
    };

    if let Some(enum_vals) = obj.get("enum").and_then(|e| e.as_array()) {
        let variants: Vec<&str> = enum_vals.iter().filter_map(|v| v.as_str()).collect();
        return Ok(variants.join("|"));
    }

    let type_name = obj.get("type").and_then(|t| t.as_str()).unwrap_or("string");

    match type_name {
        "string" => {
            if let Some(format) = obj.get("format").and_then(|f| f.as_str())
                && format == "date-time"
            {
                return Ok("datetime".to_string());
            }
            Ok("str".to_string())
        }
        "integer" => Ok("int".to_string()),
        "number" => Ok("num".to_string()),
        "boolean" => Ok("bool".to_string()),
        "array" => {
            if let Some(items) = obj.get("items") {
                let inner = encode_type(items)?;
                Ok(format!("[{inner}]"))
            } else {
                Ok("[str]".to_string())
            }
        }
        "object" => {
            let inner_props = encode_object_properties(schema)?;
            Ok(format!("{{{inner_props}}}"))
        }
        _ => Ok("str".to_string()),
    }
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_encode_tools_sample() {
        let tool = ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Event title"
                    },
                    "start": {
                        "type": "string",
                        "format": "date-time",
                        "description": "Start time, ISO 8601"
                    },
                    "duration_min": {
                        "type": "integer",
                        "description": "Duration in minutes"
                    },
                    "attendees": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Attendee emails"
                    },
                    "visibility": {
                        "type": "string",
                        "enum": ["public", "private"]
                    }
                },
                "required": ["title", "start"]
            })),
        };

        let compact = encode_tools(&[tool]).unwrap();
        assert!(compact.definitions.contains("create_calendar_event("));
        assert!(compact.definitions.contains("title:str (Event title)"));
        assert!(
            compact
                .definitions
                .contains("start:datetime (Start time, ISO 8601)")
        );
        assert!(
            compact
                .definitions
                .contains("duration_min?:int (Duration in minutes)")
        );
        assert!(
            compact
                .definitions
                .contains("attendees?:[str] (Attendee emails)")
        );
        assert!(compact.definitions.contains("visibility?:public|private"));
        assert!(
            compact
                .definitions
                .contains("): Create an event in the user's calendar.")
        );
        assert!(compact.instructions.contains("<<call "));
        assert!(compact.instructions.contains("per requested action"));
        assert!(compact.instructions.contains("without asking"));
        assert!(compact.render().starts_with("Tools; per requested action"));
    }

    #[test]
    fn test_encode_unsupported_schema_fails() {
        let tool = ToolDef {
            name: "bad_tool".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "$ref": "#/defs/bad"
            })),
        };

        let res = encode_tools(&[tool]);
        assert!(matches!(res, Err(CompactError::UnsupportedSchema { .. })));
    }
}
