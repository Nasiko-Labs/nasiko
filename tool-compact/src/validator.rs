//! Validation of parsed tool call arguments against canonical JSON Schemas.

use serde_json::Value;

use crate::error::ToolCompactError;
use crate::types::ToolDef;

/// Validate a JSON arguments object against a tool's JSON Schema definition.
///
/// Enforces:
/// 1. Arguments must be a JSON object.
/// 2. All fields listed in `required` must be present.
/// 3. Argument values must match the schema `type` (recursing into nested objects and arrays).
/// 4. Enum constraints are strictly enforced.
/// 5. Unknown properties are rejected when `additionalProperties: false`.
pub fn validate_call_arguments(tool: &ToolDef, args: &Value) -> Result<(), ToolCompactError> {
    let tool_name = &tool.function.name;
    let args_obj = args.as_object().ok_or_else(|| {
        ToolCompactError::InvalidArguments {
            tool: tool_name.clone(),
            details: "arguments must be a JSON object".to_string(),
        }
    })?;

    let Some(params) = &tool.function.parameters else {
        // If the tool has no parameters defined, any non-empty arguments object is rejected
        if !args_obj.is_empty() {
            return Err(ToolCompactError::InvalidArguments {
                tool: tool_name.clone(),
                details: "tool expects no arguments, but arguments were provided".to_string(),
            });
        }
        return Ok(());
    };

    let schema_obj = params.as_object().ok_or_else(|| {
        ToolCompactError::SchemaError(format!(
            "parameters schema for tool '{tool_name}' must be an object"
        ))
    })?;

    validate_object(tool_name, args_obj, schema_obj)
}

/// Validate a JSON object against an object schema node (handles root parameters and nested objects).
fn validate_object(
    tool_name: &str,
    args_obj: &serde_json::Map<String, Value>,
    schema_obj: &serde_json::Map<String, Value>,
) -> Result<(), ToolCompactError> {
    // 1. Validate required fields
    if let Some(req_arr) = schema_obj.get("required").and_then(Value::as_array) {
        for req_val in req_arr {
            if let Some(req_field) = req_val.as_str()
                && !args_obj.contains_key(req_field)
            {
                return Err(ToolCompactError::MissingRequiredField {
                    tool: tool_name.to_string(),
                    field: req_field.to_string(),
                });
            }
        }
    }

    let empty_map = serde_json::Map::new();
    let properties = schema_obj
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    let allow_additional = match schema_obj.get("additionalProperties") {
        Some(Value::Bool(b)) => *b,
        _ => true,
    };

    // 2. Validate each provided argument against properties schema
    for (arg_key, arg_val) in args_obj {
        if let Some(prop_schema) = properties.get(arg_key) {
            validate_value(tool_name, arg_key, arg_val, prop_schema)?;
        } else if !allow_additional {
            return Err(ToolCompactError::InvalidArguments {
                tool: tool_name.to_string(),
                details: format!("unexpected argument '{arg_key}' not permitted by schema"),
            });
        }
    }

    Ok(())
}

/// Validate a specific field value against its schema definition.
fn validate_value(
    tool_name: &str,
    field_name: &str,
    val: &Value,
    schema: &Value,
) -> Result<(), ToolCompactError> {
    let schema_obj = schema.as_object().ok_or_else(|| {
        ToolCompactError::SchemaError(format!(
            "property schema for field '{field_name}' in tool '{tool_name}' must be an object"
        ))
    })?;

    // 1. Enum validation (takes precedence)
    if let Some(enum_arr) = schema_obj.get("enum").and_then(Value::as_array) {
        let is_match = enum_arr.iter().any(|expected| expected == val);
        if !is_match {
            let allowed: Vec<String> = enum_arr
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            let value_str = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            return Err(ToolCompactError::InvalidEnumValue {
                tool: tool_name.to_string(),
                field: field_name.to_string(),
                value: value_str,
                allowed,
            });
        }
    }

    // 2. Type validation
    if let Some(type_val) = schema_obj.get("type") {
        match type_val {
            Value::String(type_str) => {
                validate_single_type(tool_name, field_name, type_str.as_str(), val, schema_obj)?;
            }
            Value::Array(type_arr) => {
                let matches_any = type_arr.iter().any(|t| {
                    t.as_str().is_some_and(|ts| is_primitive_type_match(ts, val))
                });
                if !matches_any {
                    let expected: Vec<&str> = type_arr.iter().filter_map(Value::as_str).collect();
                    return Err(type_mismatch(tool_name, field_name, &expected.join(" | "), val));
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn validate_single_type(
    tool_name: &str,
    field_name: &str,
    type_str: &str,
    val: &Value,
    schema_obj: &serde_json::Map<String, Value>,
) -> Result<(), ToolCompactError> {
    match type_str {
        "string" => {
            if !val.is_string() {
                return Err(type_mismatch(tool_name, field_name, "string", val));
            }
        }
        "integer" => {
            if !val.is_i64() && !val.is_u64() {
                return Err(type_mismatch(tool_name, field_name, "integer", val));
            }
        }
        "number" => {
            if !val.is_number() {
                return Err(type_mismatch(tool_name, field_name, "number", val));
            }
        }
        "boolean" => {
            if !val.is_boolean() {
                return Err(type_mismatch(tool_name, field_name, "boolean", val));
            }
        }
        "array" => {
            let items_arr = val
                .as_array()
                .ok_or_else(|| type_mismatch(tool_name, field_name, "array", val))?;

            if let Some(item_schema) = schema_obj.get("items") {
                for (i, item_val) in items_arr.iter().enumerate() {
                    let indexed_field = format!("{field_name}[{i}]");
                    validate_value(tool_name, &indexed_field, item_val, item_schema)?;
                }
            }
        }
        "object" => {
            let nested_obj = val
                .as_object()
                .ok_or_else(|| type_mismatch(tool_name, field_name, "object", val))?;

            validate_object(tool_name, nested_obj, schema_obj)?;
        }
        "null" if !val.is_null() => {
            return Err(type_mismatch(tool_name, field_name, "null", val));
        }
        _ => {}
    }

    Ok(())
}

fn is_primitive_type_match(expected_type: &str, val: &Value) -> bool {
    match expected_type {
        "string" => val.is_string(),
        "integer" => val.is_i64() || val.is_u64(),
        "number" => val.is_number(),
        "boolean" => val.is_boolean(),
        "array" => val.is_array(),
        "object" => val.is_object(),
        "null" => val.is_null(),
        _ => true,
    }
}

fn type_mismatch(
    tool_name: &str,
    field_name: &str,
    expected: &str,
    actual: &Value,
) -> ToolCompactError {
    let actual_type = match actual {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };

    ToolCompactError::InvalidArguments {
        tool: tool_name.to_string(),
        details: format!("expected type '{expected}' for field '{field_name}', got '{actual_type}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn sample_tool() -> ToolDef {
        ToolDef::new(FunctionDef {
            name: "create_event".into(),
            description: Some("Create calendar event".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "duration_min": { "type": "integer" },
                    "visibility": { "type": "string", "enum": ["public", "private"] },
                    "metadata": {
                        "type": "object",
                        "properties": {
                            "priority": { "type": "integer" }
                        },
                        "required": ["priority"]
                    }
                },
                "required": ["title", "visibility"],
                "additionalProperties": false
            })),
        })
    }

    #[test]
    fn test_validate_valid_arguments() {
        let tool = sample_tool();
        let args = json!({
            "title": "Planning",
            "duration_min": 60,
            "visibility": "public"
        });
        assert!(validate_call_arguments(&tool, &args).is_ok());
    }

    #[test]
    fn test_validate_missing_required_field() {
        let tool = sample_tool();
        let args = json!({
            "duration_min": 60,
            "visibility": "public"
        });
        let err = validate_call_arguments(&tool, &args).unwrap_err();
        match err {
            ToolCompactError::MissingRequiredField { tool, field } => {
                assert_eq!(tool, "create_event");
                assert_eq!(field, "title");
            }
            other => panic!("expected MissingRequiredField, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_invalid_enum() {
        let tool = sample_tool();
        let args = json!({
            "title": "Secret Meeting",
            "visibility": "secret"
        });
        let err = validate_call_arguments(&tool, &args).unwrap_err();
        match err {
            ToolCompactError::InvalidEnumValue {
                tool,
                field,
                value,
                allowed,
            } => {
                assert_eq!(tool, "create_event");
                assert_eq!(field, "visibility");
                assert_eq!(value, "secret");
                assert_eq!(allowed, vec!["public", "private"]);
            }
            other => panic!("expected InvalidEnumValue, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_type_mismatch() {
        let tool = sample_tool();
        let args = json!({
            "title": "Retro",
            "visibility": "public",
            "duration_min": "sixty"
        });
        let err = validate_call_arguments(&tool, &args).unwrap_err();
        match err {
            ToolCompactError::InvalidArguments { details, .. } => {
                assert!(details.contains("expected type 'integer'"));
            }
            other => panic!("expected InvalidArguments, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_disallowed_additional_properties() {
        let tool = sample_tool();
        let args = json!({
            "title": "Retro",
            "visibility": "public",
            "unexpected_field": "xyz"
        });
        let err = validate_call_arguments(&tool, &args).unwrap_err();
        match err {
            ToolCompactError::InvalidArguments { details, .. } => {
                assert!(details.contains("unexpected argument 'unexpected_field'"));
            }
            other => panic!("expected InvalidArguments, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_nested_object_required_field() {
        let tool = sample_tool();
        let args = json!({
            "title": "Retro",
            "visibility": "public",
            "metadata": {}
        });
        let err = validate_call_arguments(&tool, &args).unwrap_err();
        match err {
            ToolCompactError::MissingRequiredField { field, .. } => {
                assert_eq!(field, "priority");
            }
            other => panic!("expected MissingRequiredField, got {:?}", other),
        }
    }
}
