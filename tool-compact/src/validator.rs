use crate::types::{Result, ToolCall, ToolCompactError, ToolDef};
use serde_json::Value;

/// Validates a single decoded tool call against the provided schema definitions.
/// Enforces fail-closed semantics: errors are strictly returned; never guess.
pub fn validate_tool_call(call: &ToolCall, tools: &[ToolDef]) -> Result<()> {
    let tool = tools
        .iter()
        .find(|t| t.function.name == call.function.name)
        .ok_or_else(|| ToolCompactError::UnknownTool(call.function.name.clone()))?;

    // Parse JSON arguments
    let args_val: Value = serde_json::from_str(&call.function.arguments).map_err(|e| {
        ToolCompactError::InvalidArguments(
            call.function.name.clone(),
            format!("Malformed JSON args: {}", e),
        )
    })?;

    let args_obj = match args_val {
        Value::Object(map) => map,
        _ => {
            return Err(ToolCompactError::InvalidArguments(
                call.function.name.clone(),
                "Tool call arguments must be a JSON object".to_string(),
            ))
        }
    };

    if let Some(params_val) = &tool.function.parameters {
        if let Some(params_obj) = params_val.as_object() {
            // 1. Check required fields
            if let Some(required) = params_obj.get("required").and_then(|r| r.as_array()) {
                for req_key_val in required {
                    if let Some(req_key) = req_key_val.as_str() {
                        if !args_obj.contains_key(req_key) {
                            return Err(ToolCompactError::InvalidArguments(
                                call.function.name.clone(),
                                format!("Missing required argument '{}'", req_key),
                            ));
                        }
                    }
                }
            }

            // 2. Validate properties against enums and types
            if let Some(properties) = params_obj.get("properties").and_then(|p| p.as_object()) {
                for (arg_key, arg_val) in &args_obj {
                    if let Some(prop_schema) = properties.get(arg_key) {
                        validate_property_value(&call.function.name, arg_key, arg_val, prop_schema)?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_property_value(tool_name: &str, key: &str, val: &Value, schema: &Value) -> Result<()> {
    // Enum validation
    if let Some(enum_vals) = schema.get("enum").and_then(|e| e.as_array()) {
        let is_valid = enum_vals.iter().any(|ev| match (ev, val) {
            (Value::String(s1), Value::String(s2)) => s1 == s2,
            _ => ev == val,
        });

        if !is_valid {
            return Err(ToolCompactError::InvalidArguments(
                tool_name.to_string(),
                format!(
                    "Argument '{}' has invalid value '{:?}'. Must be one of {:?}",
                    key, val, enum_vals
                ),
            ));
        }
    }

    // Type checking
    if let Some(expected_type) = schema.get("type").and_then(|t| t.as_str()) {
        match expected_type {
            "string" if !val.is_string() => {
                return Err(ToolCompactError::InvalidArguments(
                    tool_name.to_string(),
                    format!("Argument '{}' must be a string", key),
                ))
            }
            "integer" if !val.is_i64() && !val.is_u64() => {
                return Err(ToolCompactError::InvalidArguments(
                    tool_name.to_string(),
                    format!("Argument '{}' must be an integer", key),
                ))
            }
            "number" if !val.is_number() => {
                return Err(ToolCompactError::InvalidArguments(
                    tool_name.to_string(),
                    format!("Argument '{}' must be a number", key),
                ))
            }
            "boolean" if !val.is_boolean() => {
                return Err(ToolCompactError::InvalidArguments(
                    tool_name.to_string(),
                    format!("Argument '{}' must be a boolean", key),
                ))
            }
            "array" if !val.is_array() => {
                return Err(ToolCompactError::InvalidArguments(
                    tool_name.to_string(),
                    format!("Argument '{}' must be an array", key),
                ))
            }
            "object" if !val.is_object() => {
                return Err(ToolCompactError::InvalidArguments(
                    tool_name.to_string(),
                    format!("Argument '{}' must be an object", key),
                ))
            }
            _ => {}
        }
    }

    Ok(())
}
