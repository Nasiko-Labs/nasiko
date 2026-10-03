use crate::error::DecodeError;
use crate::types::ToolDef;
use serde_json::Value;

/// Validates a decoded tool call against the original tool schemas.
///
/// Implements fail-closed policy:
/// - Unknown tool name -> `DecodeError::UnknownTool`
/// - Missing required field, wrong type, or invalid enum -> `DecodeError::InvalidArguments`
pub fn validate_call(name: &str, args: &Value, tools: &[ToolDef]) -> Result<(), DecodeError> {
    let tool = tools
        .iter()
        .find(|t| t.name() == name)
        .ok_or_else(|| DecodeError::UnknownTool(name.to_string()))?;

    let args_obj = args
        .as_object()
        .ok_or_else(|| DecodeError::InvalidArguments("arguments must be a JSON object".into()))?;

    let Some(params) = tool.parameters() else {
        return Ok(());
    };

    // 1. Validate required fields
    if let Some(req_array) = params.get("required").and_then(Value::as_array) {
        for req_val in req_array {
            if let Some(field_name) = req_val.as_str() {
                match args_obj.get(field_name) {
                    None | Some(Value::Null) => {
                        return Err(DecodeError::InvalidArguments(format!(
                            "missing required field: '{field_name}'"
                        )));
                    }
                    _ => {}
                }
            }
        }
    }

    // 2. Validate types and enums of supplied properties
    if let Some(properties) = params.get("properties").and_then(Value::as_object) {
        for (arg_key, arg_val) in args_obj {
            if let Some(prop_schema) = properties.get(arg_key) {
                // Check enum constraint
                if let Some(enum_vals) = prop_schema.get("enum").and_then(Value::as_array) {
                    let matches = enum_vals.iter().any(|ev| ev == arg_val);
                    if !matches {
                        return Err(DecodeError::InvalidArguments(format!(
                            "field '{arg_key}' has invalid value {:?}; allowed values: {:?}",
                            arg_val, enum_vals
                        )));
                    }
                }

                // Check type constraint
                if let Some(expected_type) = prop_schema.get("type").and_then(Value::as_str) {
                    match expected_type {
                        "string" => {
                            if !arg_val.is_string() {
                                return Err(DecodeError::InvalidArguments(format!(
                                    "field '{arg_key}' expected string, got {:?}",
                                    arg_val
                                )));
                            }
                        }
                        "integer" => {
                            if !arg_val.is_i64() && !arg_val.is_u64() {
                                return Err(DecodeError::InvalidArguments(format!(
                                    "field '{arg_key}' expected integer, got {:?}",
                                    arg_val
                                )));
                            }
                        }
                        "number" => {
                            if !arg_val.is_number() {
                                return Err(DecodeError::InvalidArguments(format!(
                                    "field '{arg_key}' expected number, got {:?}",
                                    arg_val
                                )));
                            }
                        }
                        "boolean" => {
                            if !arg_val.is_boolean() {
                                return Err(DecodeError::InvalidArguments(format!(
                                    "field '{arg_key}' expected boolean, got {:?}",
                                    arg_val
                                )));
                            }
                        }
                        "array" => {
                            let Some(arr) = arg_val.as_array() else {
                                return Err(DecodeError::InvalidArguments(format!(
                                    "field '{arg_key}' expected array, got {:?}",
                                    arg_val
                                )));
                            };
                            if let Some(item_schema) = prop_schema.get("items") {
                                if let Some(item_type) =
                                    item_schema.get("type").and_then(Value::as_str)
                                {
                                    for item in arr {
                                        if item_type == "string" && !item.is_string() {
                                            return Err(DecodeError::InvalidArguments(format!(
                                                "field '{arg_key}' array item expected string, got {:?}",
                                                item
                                            )));
                                        }
                                        if item_type == "integer"
                                            && !item.is_i64()
                                            && !item.is_u64()
                                        {
                                            return Err(DecodeError::InvalidArguments(format!(
                                                "field '{arg_key}' array item expected integer, got {:?}",
                                                item
                                            )));
                                        }
                                    }
                                }
                            }
                        }
                        "object" => {
                            if !arg_val.is_object() {
                                return Err(DecodeError::InvalidArguments(format!(
                                    "field '{arg_key}' expected object, got {:?}",
                                    arg_val
                                )));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}
