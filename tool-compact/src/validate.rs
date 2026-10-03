//! Strict Fail-Closed Schema and Argument Validator.

use crate::types::{DecodeError, ToolDef};
use serde_json::Value;

/// Validates that a tool invocation matches the tool's defined schema strictly.
pub fn validate_call(name: &str, raw_args: &str, tools: &[ToolDef]) -> Result<ToolDef, DecodeError> {
    let tool = tools
        .iter()
        .find(|t| t.function.name == name)
        .ok_or_else(|| DecodeError::UnknownTool(format!("unknown tool '{}'", name)))?;

    let parsed_args: Value = serde_json::from_str(raw_args)
        .map_err(|e| DecodeError::InvalidArguments(format!("malformed json arguments: {}", e)))?;

    if let Some(params_schema) = &tool.function.parameters {
        validate_json_schema(&parsed_args, params_schema)?;
    }

    Ok(tool.clone())
}

pub fn validate_json_schema(args: &Value, schema: &Value) -> Result<(), DecodeError> {
    let schema_obj = match schema {
        Value::Object(map) => map,
        _ => return Ok(()),
    };

    let args_obj = match args {
        Value::Object(map) => map,
        _ => {
            return Err(DecodeError::InvalidArguments(
                "arguments must be a JSON object".to_string(),
            ))
        }
    };

    // Check required fields
    if let Some(Value::Array(req_fields)) = schema_obj.get("required") {
        for req in req_fields {
            if let Some(field_name) = req.as_str() {
                if !args_obj.contains_key(field_name) || args_obj[field_name].is_null() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "missing required field '{}'",
                        field_name
                    )));
                }
            }
        }
    }

    // Check properties types and enum constraints
    if let Some(Value::Object(properties)) = schema_obj.get("properties") {
        for (field_name, field_val) in args_obj {
            if let Some(prop_schema) = properties.get(field_name) {
                validate_property_value(field_name, field_val, prop_schema)?;
            }
        }
    }

    Ok(())
}

fn validate_property_value(
    field_name: &str,
    val: &Value,
    prop_schema: &Value,
) -> Result<(), DecodeError> {
    let prop_obj = match prop_schema {
        Value::Object(map) => map,
        _ => return Ok(()),
    };

    // 1. Validate enum values if present
    if let Some(Value::Array(variants)) = prop_obj.get("enum") {
        let matched = variants.iter().any(|v| match (v, val) {
            (Value::String(s1), Value::String(s2)) => s1 == s2,
            (Value::Number(n1), Value::Number(n2)) => n1 == n2,
            (Value::Bool(b1), Value::Bool(b2)) => b1 == b2,
            _ => false,
        });

        if !matched {
            return Err(DecodeError::InvalidArguments(format!(
                "field '{}' value {:?} violates enum constraint",
                field_name, val
            )));
        }
    }

    // 2. Validate type
    if let Some(Value::String(expected_type)) = prop_obj.get("type") {
        match expected_type.as_str() {
            "string" => {
                if !val.is_string() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "field '{}' expected string, got {:?}",
                        field_name, val
                    )));
                }
            }
            "integer" => {
                if !val.is_i64() && !val.is_u64() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "field '{}' expected integer, got {:?}",
                        field_name, val
                    )));
                }
            }
            "number" => {
                if !val.is_number() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "field '{}' expected number, got {:?}",
                        field_name, val
                    )));
                }
            }
            "boolean" => {
                if !val.is_boolean() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "field '{}' expected boolean, got {:?}",
                        field_name, val
                    )));
                }
            }
            "array" => {
                let arr = val.as_array().ok_or_else(|| {
                    DecodeError::InvalidArguments(format!(
                        "field '{}' expected array, got {:?}",
                        field_name, val
                    ))
                })?;

                if let Some(item_schema) = prop_obj.get("items") {
                    for item in arr {
                        validate_property_value(&format!("{item}"), item, item_schema)?;
                    }
                }
            }
            "object" => {
                if !val.is_object() {
                    return Err(DecodeError::InvalidArguments(format!(
                        "field '{}' expected object, got {:?}",
                        field_name, val
                    )));
                }
            }
            _ => {}
        }
    }

    Ok(())
}
