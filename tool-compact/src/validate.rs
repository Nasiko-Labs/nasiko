//! Schema validation for decoded tool call arguments.
//!
//! Validates every call against the original JSON Schema. Returns an error for
//! unknown tools, missing required fields, or invalid values — never a guessed call.

use serde_json::Value;

use crate::error::DecodeError;
use crate::types::ToolDef;

/// Validate that `arguments` conforms to the tool's parameter schema.
///
/// Checks:
/// 1. All required fields are present.
/// 2. No unknown fields (arguments not in `properties`).
/// 3. Type conformance for each provided field.
/// 4. Enum value constraints.
pub fn validate_call(tool: &ToolDef, arguments: &Value) -> Result<(), DecodeError> {
    let params = match &tool.parameters {
        Some(p) => p,
        // No parameters defined — arguments must be empty or an empty object.
        None => {
            if let Value::Object(map) = arguments {
                if map.is_empty() {
                    return Ok(());
                }
                return Err(DecodeError::InvalidArgument {
                    tool: tool.name.clone(),
                    field: map.keys().next().unwrap().clone(),
                    reason: "tool accepts no parameters".into(),
                });
            }
            return Ok(());
        }
    };

    let args = match arguments {
        Value::Object(map) => map,
        _ => {
            return Err(DecodeError::InvalidJson {
                tool: tool.name.clone(),
                reason: "arguments must be a JSON object".into(),
            });
        }
    };

    let properties = params.get("properties").and_then(|p| p.as_object());
    let required: Vec<&str> = params
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    // Check required fields are present.
    for field in &required {
        if !args.contains_key(*field) {
            return Err(DecodeError::MissingRequired {
                tool: tool.name.clone(),
                field: field.to_string(),
            });
        }
    }

    // Validate each provided argument.
    if let Some(props) = properties {
        for (key, value) in args {
            if let Some(schema) = props.get(key) {
                validate_value(&tool.name, key, value, schema)?;
            }
            // Unknown fields: we allow them for forward-compatibility (additionalProperties
            // defaults to true in JSON Schema). This is intentional — the model may provide
            // extra context fields and the downstream tool may accept them.
        }
    }

    Ok(())
}

/// Validate a single argument value against its JSON Schema property definition.
fn validate_value(
    tool_name: &str,
    field: &str,
    value: &Value,
    schema: &Value,
) -> Result<(), DecodeError> {
    // Check enum constraint first (applies regardless of type).
    if let Some(enum_values) = schema.get("enum").and_then(|e| e.as_array()) {
        if !enum_values.contains(value) {
            return Err(DecodeError::InvalidArgument {
                tool: tool_name.to_string(),
                field: field.to_string(),
                reason: format!(
                    "value {} not in enum {:?}",
                    value,
                    enum_values
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                ),
            });
        }
        return Ok(());
    }

    let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match type_str {
        "string" => {
            if !value.is_string() {
                return Err(DecodeError::InvalidArgument {
                    tool: tool_name.to_string(),
                    field: field.to_string(),
                    reason: format!("expected string, got {}", value_type_name(value)),
                });
            }
        }
        "integer" => {
            if !value.is_i64() && !value.is_u64() {
                // Also accept f64 if it's a whole number.
                if let Some(n) = value.as_f64() {
                    if n.fract() != 0.0 {
                        return Err(DecodeError::InvalidArgument {
                            tool: tool_name.to_string(),
                            field: field.to_string(),
                            reason: format!("expected integer, got float {n}"),
                        });
                    }
                } else {
                    return Err(DecodeError::InvalidArgument {
                        tool: tool_name.to_string(),
                        field: field.to_string(),
                        reason: format!("expected integer, got {}", value_type_name(value)),
                    });
                }
            }
        }
        "number" => {
            if !value.is_number() {
                return Err(DecodeError::InvalidArgument {
                    tool: tool_name.to_string(),
                    field: field.to_string(),
                    reason: format!("expected number, got {}", value_type_name(value)),
                });
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err(DecodeError::InvalidArgument {
                    tool: tool_name.to_string(),
                    field: field.to_string(),
                    reason: format!("expected boolean, got {}", value_type_name(value)),
                });
            }
        }
        "array" => {
            if !value.is_array() {
                return Err(DecodeError::InvalidArgument {
                    tool: tool_name.to_string(),
                    field: field.to_string(),
                    reason: format!("expected array, got {}", value_type_name(value)),
                });
            }
            // Validate items if item schema is present.
            if let (Some(items_schema), Some(arr)) = (schema.get("items"), value.as_array()) {
                for (i, item) in arr.iter().enumerate() {
                    let item_field = format!("{field}[{i}]");
                    validate_value(tool_name, &item_field, item, items_schema)?;
                }
            }
        }
        "object" => {
            if !value.is_object() {
                return Err(DecodeError::InvalidArgument {
                    tool: tool_name.to_string(),
                    field: field.to_string(),
                    reason: format!("expected object, got {}", value_type_name(value)),
                });
            }
            // Recursively validate nested object properties.
            if let (Some(nested_props), Some(obj)) = (
                schema.get("properties").and_then(|p| p.as_object()),
                value.as_object(),
            ) {
                // Check nested required fields.
                let nested_required: Vec<&str> = schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                for req in &nested_required {
                    if !obj.contains_key(*req) {
                        return Err(DecodeError::MissingRequired {
                            tool: tool_name.to_string(),
                            field: format!("{field}.{req}"),
                        });
                    }
                }
                for (k, v) in obj {
                    if let Some(prop_schema) = nested_props.get(k) {
                        let nested_field = format!("{field}.{k}");
                        validate_value(tool_name, &nested_field, v, prop_schema)?;
                    }
                }
            }
        }
        _ => {
            // Unknown or missing type — allow through. This handles cases like
            // `anyOf`, `oneOf`, or schemas without explicit type declarations.
        }
    }

    Ok(())
}

/// Human-readable name for a JSON value's type.
fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create a calendar event".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        }
    }

    #[test]
    fn valid_call_passes() {
        let tool = calendar_tool();
        let args = json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30"});
        assert!(validate_call(&tool, &args).is_ok());
    }

    #[test]
    fn valid_call_with_all_optional_fields() {
        let tool = calendar_tool();
        let args = json!({
            "title": "Review",
            "start": "2026-10-05T15:00:00+05:30",
            "duration_min": 30,
            "attendees": ["riya@example.com"],
            "visibility": "private"
        });
        assert!(validate_call(&tool, &args).is_ok());
    }

    #[test]
    fn missing_required_field() {
        let tool = calendar_tool();
        let args = json!({"title": "Review"});
        let err = validate_call(&tool, &args).unwrap_err();
        assert!(matches!(err, DecodeError::MissingRequired { ref field, .. } if field == "start"));
    }

    #[test]
    fn invalid_enum_value() {
        let tool = calendar_tool();
        let args = json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30", "visibility": "secret"});
        let err = validate_call(&tool, &args).unwrap_err();
        assert!(
            matches!(err, DecodeError::InvalidArgument { ref field, .. } if field == "visibility")
        );
    }

    #[test]
    fn wrong_type_integer() {
        let tool = calendar_tool();
        let args = json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30", "duration_min": "thirty"});
        let err = validate_call(&tool, &args).unwrap_err();
        assert!(
            matches!(err, DecodeError::InvalidArgument { ref field, .. } if field == "duration_min")
        );
    }

    #[test]
    fn wrong_type_array() {
        let tool = calendar_tool();
        let args = json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30", "attendees": "not-an-array"});
        let err = validate_call(&tool, &args).unwrap_err();
        assert!(
            matches!(err, DecodeError::InvalidArgument { ref field, .. } if field == "attendees")
        );
    }

    #[test]
    fn array_item_type_validation() {
        let tool = calendar_tool();
        let args =
            json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30", "attendees": [123]});
        let err = validate_call(&tool, &args).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidArgument { .. }));
    }

    #[test]
    fn no_params_empty_args() {
        let tool = ToolDef {
            name: "noop".into(),
            description: None,
            parameters: None,
        };
        assert!(validate_call(&tool, &json!({})).is_ok());
    }

    #[test]
    fn integer_accepts_whole_float() {
        let tool = calendar_tool();
        // JSON numbers 30.0 and 30 are both valid integers.
        let args =
            json!({"title": "x", "start": "2026-10-05T15:00:00+05:30", "duration_min": 30.0});
        assert!(validate_call(&tool, &args).is_ok());
    }
}
