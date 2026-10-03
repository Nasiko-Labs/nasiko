use crate::error::Error;
use crate::model::{CompactTools, ToolDef};
use serde_json::Value;
use std::collections::HashSet;

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    if tools.is_empty() {
        return Err(Error::InvalidFormat(
            "at least one tool is required".to_string(),
        ));
    }

    let mut seen = HashSet::new();

    for tool in tools {
        if tool.name.trim().is_empty() {
            return Err(Error::InvalidFormat(
                "tool name cannot be empty".to_string(),
            ));
        }

        if !seen.insert(tool.name.clone()) {
            return Err(Error::InvalidFormat(format!(
                "duplicate tool name: {}",
                tool.name
            )));
        }

        if let Some(parameters) = &tool.parameters {
            validate_schema(parameters, &tool.name)?;
        }
    }

    let lookup = tools
        .iter()
        .enumerate()
        .map(|(index, tool)| (tool.name.clone(), index))
        .collect();

    let compact = CompactTools {
        tools: tools.to_vec(),
        lookup,
        rendered: render_tools(tools),
    };

    Ok(compact)
}

/// Returns the compact tool definitions that can be injected into a prompt.
pub fn render_compact_tools(compact: &CompactTools) -> &str {
    &compact.rendered
}

fn render_tools(tools: &[ToolDef]) -> String {
    let mut output = String::new();

    for tool in tools {
        output.push_str(&tool.name);

        if let Some(parameters) = &tool.parameters {
            output.push_str(&render_parameters(parameters));
        } else {
            output.push_str("()");
        }

        if let Some(description) = &tool.description {
            let description = description.replace('\n', " ");
            if !description.trim().is_empty() {
                output.push_str(" - ");
                output.push_str(description.trim());
            }
        }

        output.push('\n');
    }

    output.push_str("To call a tool, emit: <<call name {json args}>>");

    output
}

fn render_parameters(schema: &Value) -> String {
    let Some(object) = schema.as_object() else {
        return "()".to_string();
    };

    let Some(properties) = object.get("properties").and_then(Value::as_object) else {
        return "()".to_string();
    };

    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let fields = properties
        .iter()
        .map(|(name, property)| {
            let optional = if required.contains(name.as_str()) {
                ""
            } else {
                "?"
            };

            format!(
                "{}{}:{}",
                name,
                optional,
                compact_type(property)
            )
        })
        .collect::<Vec<_>>();

    format!("({})", fields.join(", "))
}

fn compact_type(schema: &Value) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let values = values
            .iter()
            .map(compact_enum_value)
            .collect::<Vec<_>>();

        if !values.is_empty() {
            return values.join("|");
        }
    }

    match schema.get("type").and_then(Value::as_str) {
        Some("string") => {
            match schema.get("format").and_then(Value::as_str) {
                Some("date-time") => "datetime".to_string(),
                Some("date") => "date".to_string(),
                Some("time") => "time".to_string(),
                _ => "str".to_string(),
            }
        }

        Some("integer") => "int".to_string(),

        Some("number") => "number".to_string(),

        Some("boolean") => "bool".to_string(),

        Some("null") => "null".to_string(),

        Some("array") => {
            let item_type = schema
                .get("items")
                .map(compact_type)
                .unwrap_or_else(|| "any".to_string());

            format!("[{}]", item_type)
        }

        Some("object") => {
            let Some(properties) =
                schema.get("properties").and_then(Value::as_object)
            else {
                return "object".to_string();
            };

            let required = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default();

            let fields = properties
                .iter()
                .map(|(name, property)| {
                    let optional = if required.contains(name.as_str()) {
                        ""
                    } else {
                        "?"
                    };

                    format!(
                        "{}{}:{}",
                        name,
                        optional,
                        compact_type(property)
                    )
                })
                .collect::<Vec<_>>();

            if fields.is_empty() {
                "object".to_string()
            } else {
                format!("{{{}}}", fields.join(","))
            }
        }

        _ => "any".to_string(),
    }
}

fn compact_enum_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
        _ => value.to_string(),
    }
}

fn validate_schema(value: &Value, tool_name: &str) -> Result<(), Error> {
    let Some(object) = value.as_object() else {
        return Err(Error::UnsupportedSchema(format!(
            "tool `{tool_name}` parameters must be a JSON object"
        )));
    };

    if let Some(properties) = object.get("properties") {
        if !properties.is_object() {
            return Err(Error::UnsupportedSchema(format!(
                "tool `{tool_name}` properties must be an object"
            )));
        }
    }

    if let Some(required) = object.get("required") {
        if !required.is_array() {
            return Err(Error::UnsupportedSchema(format!(
                "tool `{tool_name}` required must be an array"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_valid_tools() {
        let tools = vec![ToolDef {
            name: "get_weather".to_string(),
            description: Some("Get weather for a city".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string"
                    }
                },
                "required": ["city"]
            })),
        }];

        let compact = encode_tools(&tools).unwrap();

        assert_eq!(compact.tools, tools);
        assert_eq!(compact.lookup.get("get_weather"), Some(&0));
    }

    #[test]
    fn renders_compact_signature() {
        let tools = vec![ToolDef {
            name: "create_event".to_string(),
            description: Some("Create an event".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration": {"type": "integer"},
                    "visibility": {
                        "type": "string",
                        "enum": ["public", "private"]
                    }
                },
                "required": ["title", "start"]
            })),
        }];

        let compact = encode_tools(&tools).unwrap();

        assert!(compact.rendered.contains("create_event("));
        assert!(compact.rendered.contains("title:str"));
        assert!(compact.rendered.contains("start:datetime"));
        assert!(compact.rendered.contains("duration?:int"));
        assert!(compact.rendered.contains("visibility?:public|private"));
        assert!(compact.rendered.contains(" - Create an event"));
        assert!(compact.rendered.contains(
            "To call a tool, emit: <<call name {json args}>>"
        ));
    }

    #[test]
    fn renders_nested_arrays_and_objects() {
        let tools = vec![ToolDef {
            name: "test".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "integer"},
                                "name": {"type": "string"}
                            },
                            "required": ["id"]
                        }
                    }
                },
                "required": ["items"]
            })),
        }];

        let compact = encode_tools(&tools).unwrap();

        assert_eq!(
            compact.rendered,
            "test(items:[{id:int,name?:str}])\nTo call a tool, emit: <<call name {json args}>>"
        );
    }

    #[test]
    fn rejects_duplicate_tools() {
        let tools = vec![
            ToolDef {
                name: "search".to_string(),
                description: None,
                parameters: None,
            },
            ToolDef {
                name: "search".to_string(),
                description: None,
                parameters: None,
            },
        ];

        let result = encode_tools(&tools);

        assert!(matches!(result, Err(Error::InvalidFormat(_))));
    }

    #[test]
    fn rejects_empty_tool_name() {
        let tools = vec![ToolDef {
            name: "".to_string(),
            description: None,
            parameters: None,
        }];

        let result = encode_tools(&tools);

        assert!(matches!(result, Err(Error::InvalidFormat(_))));
    }
}
