use crate::{
    schema::{Schema, SchemaType, ToolCall, ToolDef},
    CompactError,
};
use serde_json::Value;

pub fn validate_call(
    call: &ToolCall,
    tools: &[ToolDef],
) -> Result<(), CompactError> {
    let tool = tools
        .iter()
        .find(|tool| tool.name == call.name)
        .ok_or_else(|| CompactError::UnknownTool(call.name.clone()))?;

    validate_schema(&tool.parameters, &call.arguments, "$")
}

fn validate_schema(
    schema: &Schema,
    value: &Value,
    path: &str,
) -> Result<(), CompactError> {
    match &schema.schema_type {
        SchemaType::String => {
            if !value.is_string() {
                return Err(invalid(path, "expected string"));
            }

            if !schema.enum_values.is_empty() {
                let actual = value.as_str().unwrap();

                if !schema.enum_values.iter().any(|v| v == actual) {
                    return Err(invalid(
                        path,
                        &format!(
                            "expected one of [{}]",
                            schema.enum_values.join(", ")
                        ),
                    ));
                }
            }
        }

        SchemaType::Integer => {
            if !value.is_i64() && !value.is_u64() {
                return Err(invalid(path, "expected integer"));
            }
        }

        SchemaType::Number => {
            if !value.is_number() {
                return Err(invalid(path, "expected number"));
            }
        }

        SchemaType::Boolean => {
            if !value.is_boolean() {
                return Err(invalid(path, "expected boolean"));
            }
        }

        SchemaType::Object => {
            let object = value
                .as_object()
                .ok_or_else(|| invalid(path, "expected object"))?;

            for required in &schema.required {
                if !object.contains_key(required) {
                    return Err(invalid(
                        path,
                        &format!("missing required argument '{required}'"),
                    ));
                }
            }

            for (name, child_schema) in &schema.properties {
                if let Some(child_value) = object.get(name) {
                    let child_path = format!("{path}.{name}");
                    validate_schema(child_schema, child_value, &child_path)?;
                }
            }
        }

        SchemaType::Array => {
            let array = value
                .as_array()
                .ok_or_else(|| invalid(path, "expected array"))?;

            if let Some(item_schema) = &schema.items {
                for (index, item) in array.iter().enumerate() {
                    validate_schema(
                        item_schema,
                        item,
                        &format!("{path}[{index}]"),
                    )?;
                }
            }
        }

        SchemaType::Unknown(_) => {
            return Err(CompactError::UnsupportedSchema(
                "unknown schema type".into(),
            ));
        }
    }

    Ok(())
}

fn invalid(path: &str, message: &str) -> CompactError {
    CompactError::InvalidArguments(format!("{path}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_schema;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event".into()),
            parameters: parse_schema(&json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string"},
                    "duration_min": {"type": "integer"},
                    "visibility": {
                        "type": "string",
                        "enum": ["public", "private"]
                    }
                },
                "required": ["title", "start"]
            }))
            .unwrap(),
        }
    }

    #[test]
    fn accepts_valid_call() {
        let call = ToolCall {
            name: "create_calendar_event".into(),
            arguments: json!({
                "title": "Design review",
                "start": "2026-10-03T15:00:00Z",
                "duration_min": 30,
                "visibility": "private"
            }),
        };

        assert!(validate_call(&call, &[calendar_tool()]).is_ok());
    }

    #[test]
    fn rejects_unknown_tool() {
        let call = ToolCall {
            name: "delete_everything".into(),
            arguments: json!({}),
        };

        assert!(matches!(
            validate_call(&call, &[calendar_tool()]),
            Err(CompactError::UnknownTool(_))
        ));
    }

    #[test]
    fn rejects_missing_required_argument() {
        let call = ToolCall {
            name: "create_calendar_event".into(),
            arguments: json!({
                "start": "2026-10-03T15:00:00Z"
            }),
        };

        assert!(validate_call(&call, &[calendar_tool()]).is_err());
    }

    #[test]
    fn rejects_invalid_enum() {
        let call = ToolCall {
            name: "create_calendar_event".into(),
            arguments: json!({
                "title": "Review",
                "start": "2026-10-03T15:00:00Z",
                "visibility": "secret"
            }),
        };

        assert!(validate_call(&call, &[calendar_tool()]).is_err());
    }

    #[test]
    fn rejects_wrong_type() {
        let call = ToolCall {
            name: "create_calendar_event".into(),
            arguments: json!({
                "title": 123,
                "start": "2026-10-03T15:00:00Z"
            }),
        };

        assert!(validate_call(&call, &[calendar_tool()]).is_err());
    }
}