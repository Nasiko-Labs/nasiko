use serde_json::Value;

use crate::error::DecodeError;
use crate::types::ToolDef;

/// Validate a decoded tool call against the available tool schemas.
/// Enforces strict fail-closed validation:
/// - Rejects unknown tools with DecodeError::UnknownTool
/// - Rejects missing required parameters with DecodeError::InvalidArguments
/// - Rejects null values for required or non-nullable parameters with DecodeError::InvalidArguments
/// - Rejects invalid enum values with DecodeError::InvalidArguments
/// - Rejects type mismatches (distinguishing int, num, bool, str, arr, obj) with DecodeError::InvalidArguments
pub fn validate_call(tool_name: &str, args: &Value, tools: &[ToolDef]) -> Result<(), DecodeError> {
    // 1. Locate tool definition
    let tool = tools
        .iter()
        .find(|t| t.function.name == tool_name)
        .ok_or_else(|| DecodeError::UnknownTool(tool_name.to_string()))?;

    let Some(params_schema) = &tool.function.parameters else {
        return Ok(());
    };

    let Some(schema_obj) = params_schema.as_object() else {
        return Ok(());
    };

    // Args must be an object
    let Some(args_obj) = args.as_object() else {
        return Err(DecodeError::InvalidArguments(
            "arguments must be a JSON object".to_string(),
        ));
    };

    // 2. Check required fields
    if let Some(required) = schema_obj.get("required").and_then(Value::as_array) {
        for req_field in required {
            if let Some(field_name) = req_field.as_str() {
                match args_obj.get(field_name) {
                    None => {
                        return Err(DecodeError::InvalidArguments(format!(
                            "missing required argument '{field_name}'"
                        )));
                    }
                    Some(Value::Null) => {
                        return Err(DecodeError::InvalidArguments(format!(
                            "required argument '{field_name}' cannot be null"
                        )));
                    }
                    _ => {}
                }
            }
        }
    }

    // 3. Check property schemas (types, enums, etc.)
    if let Some(properties) = schema_obj.get("properties").and_then(Value::as_object) {
        let additional_allowed = schema_obj
            .get("additionalProperties")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        for (arg_key, arg_val) in args_obj {
            if let Some(prop_schema) = properties.get(arg_key) {
                validate_property_value(arg_key, arg_val, prop_schema)?;
            } else if !additional_allowed {
                return Err(DecodeError::InvalidArguments(format!(
                    "unexpected argument '{arg_key}'"
                )));
            }
        }
    }

    Ok(())
}

fn validate_property_value(key: &str, val: &Value, schema: &Value) -> Result<(), DecodeError> {
    // Null handling
    if val.is_null() {
        let is_nullable = schema
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || match schema.get("type") {
                Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("null")),
                _ => false,
            };
        if !is_nullable {
            return Err(DecodeError::InvalidArguments(format!(
                "parameter '{key}' cannot be null"
            )));
        }
        return Ok(());
    }

    // Check enum constraints
    if let Some(enum_vals) = schema.get("enum").and_then(Value::as_array) {
        if !enum_vals.contains(val) {
            return Err(DecodeError::InvalidArguments(format!(
                "invalid enum value '{val}' for parameter '{key}'"
            )));
        }
    }

    // Check type constraints
    if let Some(expected_type) = schema.get("type").and_then(Value::as_str) {
        match expected_type {
            "string" => {
                if !val.is_string() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "parameter '{key}' expected string, got {val}"
                    )));
                }
            }
            "integer" => {
                if !val.is_i64() && !val.is_u64() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "parameter '{key}' expected integer, got {val}"
                    )));
                }
            }
            "number" => {
                if !val.is_number() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "parameter '{key}' expected number, got {val}"
                    )));
                }
            }
            "boolean" => {
                if !val.is_boolean() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "parameter '{key}' expected boolean, got {val}"
                    )));
                }
            }
            "array" => {
                let Some(arr) = val.as_array() else {
                    return Err(DecodeError::InvalidArguments(format!(
                        "parameter '{key}' expected array, got {val}"
                    )));
                };
                if let Some(items_schema) = schema.get("items") {
                    for (i, item) in arr.iter().enumerate() {
                        validate_property_value(&format!("{key}[{i}]"), item, items_schema)?;
                    }
                }
            }
            "object" => {
                let Some(nested_obj) = val.as_object() else {
                    return Err(DecodeError::InvalidArguments(format!(
                        "parameter '{key}' expected object, got {val}"
                    )));
                };

                // Validate nested required
                if let Some(nested_req) = schema.get("required").and_then(Value::as_array) {
                    for req_f in nested_req {
                        if let Some(name) = req_f.as_str() {
                            match nested_obj.get(name) {
                                None => {
                                    return Err(DecodeError::InvalidArguments(format!(
                                        "missing required argument '{key}.{name}'"
                                    )));
                                }
                                Some(Value::Null) => {
                                    return Err(DecodeError::InvalidArguments(format!(
                                        "required argument '{key}.{name}' cannot be null"
                                    )));
                                }
                                _ => {}
                            }
                        }
                    }
                }

                // Validate nested properties
                if let Some(nested_props) = schema.get("properties").and_then(Value::as_object) {
                    let nested_additional = schema
                        .get("additionalProperties")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);

                    for (sub_k, sub_v) in nested_obj {
                        if let Some(sub_schema) = nested_props.get(sub_k) {
                            validate_property_value(&format!("{key}.{sub_k}"), sub_v, sub_schema)?;
                        } else if !nested_additional {
                            return Err(DecodeError::InvalidArguments(format!(
                                "unexpected argument '{key}.{sub_k}'"
                            )));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}
