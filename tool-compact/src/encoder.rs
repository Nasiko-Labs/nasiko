use crate::types::ToolDef;
use crate::{Error, Result};
use serde_json::Value;

/// The compact textual representation of a set of tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub text: String,
}

/// Encode tool definitions into the compact tool format.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut output = String::new();

    for (index, tool) in tools.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }

        output.push_str(&tool.name);
        output.push('(');

        if let Some(parameters) = &tool.parameters {
            output.push_str(&encode_parameters(parameters)?);
        }

        output.push(')');

        if let Some(description) = &tool.description {
            output.push_str(" - ");
            output.push_str(description);
        }
    }

    Ok(CompactTools { text: output })
}

/// Encode the top-level parameter object.
fn encode_parameters(schema: &Value) -> Result<String> {
    let object = schema.as_object().ok_or_else(|| {
        Error::InvalidToolDefinition("parameters must be a JSON object".to_string())
    })?;

    let properties = match object.get("properties") {
        Some(Value::Object(properties)) => properties,

        Some(_) => {
            return Err(Error::InvalidToolDefinition(
                "properties must be a JSON object".to_string(),
            ));
        }

        None => return Ok(String::new()),
    };

    let required = match object.get("required") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<std::collections::HashSet<_>>(),

        Some(_) => {
            return Err(Error::InvalidToolDefinition(
                "required must be an array".to_string(),
            ));
        }

        None => std::collections::HashSet::new(),
    };

    // Sort property names so output is deterministic.
    let mut property_names: Vec<&String> = properties.keys().collect();
    property_names.sort();

    let mut parameters = Vec::new();

    for name in property_names {
        let schema = &properties[name];

        let type_text = encode_schema_type(schema)?;

        let optional = if required.contains(name.as_str()) {
            ""
        } else {
            "?"
        };

        parameters.push(format!("{name}{optional}:{type_text}"));
    }

    Ok(parameters.join(", "))
}

/// Convert a JSON Schema parameter into compact type syntax.
fn encode_schema_type(schema: &Value) -> Result<String> {
    let object = schema.as_object().ok_or_else(|| {
        Error::InvalidToolDefinition("parameter schema must be an object".to_string())
    })?;

    // Enum takes precedence over the normal type representation.
    if let Some(Value::Array(enum_values)) = object.get("enum") {
        let values = enum_values
            .iter()
            .map(|value| {
                value.as_str().map(str::to_string).ok_or_else(|| {
                    Error::InvalidToolDefinition("enum values must be strings".to_string())
                })
            })
            .collect::<Result<Vec<_>>>()?;

        return Ok(values.join("|"));
    }

    match object.get("type").and_then(Value::as_str) {
        Some("string") => Ok("str".to_string()),

        Some("integer") => Ok("int".to_string()),

        Some("number") => Ok("num".to_string()),

        Some("boolean") => Ok("bool".to_string()),

        Some("array") => {
            let items = object.get("items").ok_or_else(|| {
                Error::InvalidToolDefinition("array parameter is missing items".to_string())
            })?;

            Ok(format!("[{}]", encode_schema_type(items)?))
        }

        Some("object") => {
            let properties = match object.get("properties") {
                Some(Value::Object(properties)) => properties,

                Some(_) => {
                    return Err(Error::InvalidToolDefinition(
                        "object properties must be an object".to_string(),
                    ));
                }

                None => {
                    return Err(Error::InvalidToolDefinition(
                        "object parameter is missing properties".to_string(),
                    ));
                }
            };

            let required = match object.get("required") {
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<std::collections::HashSet<_>>(),

                Some(_) => {
                    return Err(Error::InvalidToolDefinition(
                        "required must be an array".to_string(),
                    ));
                }

                None => std::collections::HashSet::new(),
            };

            // Sort nested property names too.
            let mut property_names: Vec<&String> = properties.keys().collect();
            property_names.sort();

            let mut fields = Vec::new();

            for name in property_names {
                let field_schema = &properties[name];

                let field_type = encode_schema_type(field_schema)?;

                let optional = if required.contains(name.as_str()) {
                    ""
                } else {
                    "?"
                };

                fields.push(format!("{name}{optional}:{field_type}"));
            }

            Ok(format!("{{{}}}", fields.join(", ")))
        }

        Some(other) => Err(Error::InvalidToolDefinition(format!(
            "unsupported parameter type: {other}"
        ))),

        None => Err(Error::InvalidToolDefinition(
            "parameter is missing type".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_tool_name_and_description() {
        let tools = vec![ToolDef {
            name: "get_weather".to_string(),
            description: Some("Get the current weather".to_string()),
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

        let result = encode_tools(&tools).unwrap();

        assert_eq!(
            result.text,
            "get_weather(city:str) - Get the current weather"
        );
    }

    #[test]
    fn preserves_required_and_optional_parameters() {
        let tools = vec![ToolDef {
            name: "get_weather".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string"
                    },
                    "units": {
                        "type": "string"
                    }
                },
                "required": ["city"]
            })),
        }];

        let result = encode_tools(&tools).unwrap();

        assert_eq!(result.text, "get_weather(city:str, units?:str)");
    }

    #[test]
    fn encodes_enums() {
        let tools = vec![ToolDef {
            name: "get_weather".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "units": {
                        "type": "string",
                        "enum": ["celsius", "fahrenheit"]
                    }
                },
                "required": ["units"]
            })),
        }];

        let result = encode_tools(&tools).unwrap();

        assert_eq!(result.text, "get_weather(units:celsius|fahrenheit)");
    }

    #[test]
    fn encodes_arrays() {
        let tools = vec![ToolDef {
            name: "send_email".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "recipients": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        }
                    }
                },
                "required": ["recipients"]
            })),
        }];

        let result = encode_tools(&tools).unwrap();

        assert_eq!(result.text, "send_email(recipients:[str])");
    }

    #[test]
    fn encodes_nested_objects() {
        let tools = vec![ToolDef {
            name: "create_user".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "profile": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string"
                            },
                            "age": {
                                "type": "integer"
                            }
                        },
                        "required": ["name"]
                    }
                },
                "required": ["profile"]
            })),
        }];

        let result = encode_tools(&tools).unwrap();

        assert_eq!(result.text, "create_user(profile:{age?:int, name:str})");
    }

    #[test]
    fn rejects_object_without_properties() {
        let tools = vec![ToolDef {
            name: "bad_tool".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "profile": {
                        "type": "object"
                    }
                },
                "required": ["profile"]
            })),
        }];

        let result = encode_tools(&tools);

        assert!(result.is_err());
    }

    #[test]
    fn encodes_multiple_tools() {
        let tools = vec![
            ToolDef {
                name: "get_weather".to_string(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string"
                        }
                    },
                    "required": ["city"]
                })),
            },
            ToolDef {
                name: "get_time".to_string(),
                description: None,
                parameters: None,
            },
        ];

        let result = encode_tools(&tools).unwrap();

        assert_eq!(result.text, "get_weather(city:str)\nget_time()");
    }

    #[test]
    fn encodes_tool_without_parameters() {
        let tools = vec![ToolDef {
            name: "get_time".to_string(),
            description: Some("Get the current time".to_string()),
            parameters: None,
        }];

        let result = encode_tools(&tools).unwrap();

        assert_eq!(result.text, "get_time() - Get the current time");
    }

    #[test]
    fn rejects_unsupported_parameter_type() {
        let tools = vec![ToolDef {
            name: "bad_tool".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "value": {
                        "type": "null"
                    }
                },
                "required": ["value"]
            })),
        }];

        let result = encode_tools(&tools);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_non_string_enum_values() {
        let tools = vec![ToolDef {
            name: "bad_tool".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "value": {
                        "type": "integer",
                        "enum": [1, 2, 3]
                    }
                },
                "required": ["value"]
            })),
        }];

        let result = encode_tools(&tools);

        assert!(result.is_err());
    }
}
