//! Argument validation against a tool's JSON Schema.

use serde_json::Value;

use crate::error::DecodeError;
use crate::schema::{ParamSchema, parse_schema, required_fields};
use crate::types::ToolDef;

/// Validate `args` against the schema in `tool`.
///
/// Returns `Ok(())` when:
/// - all required fields are present
/// - all provided fields have correct types
/// - all enum fields have values within the allowed set
///
/// Returns `Err(DecodeError::InvalidArguments { .. })` on any violation.
/// Returns `Ok(())` for tools with no parameters schema (fully opaque).
pub fn validate_args(tool: &ToolDef, args: &Value) -> Result<(), DecodeError> {
    let params = match &tool.function.parameters {
        Some(p) => p,
        None => return Ok(()), // No schema: accept any object.
    };

    let args_obj = match args.as_object() {
        Some(o) => o,
        None => {
            return Err(DecodeError::InvalidArguments {
                tool: tool.function.name.clone(),
                reason: "arguments must be a JSON object".to_string(),
            });
        }
    };

    // Check required fields.
    let required = required_fields(params);
    for req in &required {
        if !args_obj.contains_key(req) {
            return Err(DecodeError::InvalidArguments {
                tool: tool.function.name.clone(),
                reason: format!("missing required field: {}", req),
            });
        }
    }

    // Type-check and enum-check each provided field.
    let props = params.get("properties").and_then(Value::as_object);
    if let Some(props) = props {
        for (key, val) in args_obj {
            if let Some(schema_val) = props.get(key.as_str()) {
                let ps = parse_schema(schema_val);
                validate_value(&tool.function.name, key, val, &ps)?;
            }
            // Unknown/extra fields are silently accepted: JSON Schema
            // `additionalProperties` defaults to true.
        }
    }

    Ok(())
}

fn validate_value(
    tool_name: &str,
    field: &str,
    val: &Value,
    schema: &ParamSchema,
) -> Result<(), DecodeError> {
    match schema {
        ParamSchema::Str { enum_values } => match val {
            Value::String(s) => {
                if let Some(allowed) = enum_values.as_ref().filter(|allowed| !allowed.contains(s)) {
                    return Err(DecodeError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!(
                            "field '{}' value '{}' not in allowed enum [{}]",
                            field,
                            s,
                            allowed.join(", ")
                        ),
                    });
                }
            }
            _ => {
                return Err(DecodeError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reason: format!("field '{}' must be a string", field),
                });
            }
        },
        ParamSchema::Integer => {
            if !val.is_i64() && !val.is_u64() {
                return Err(DecodeError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reason: format!("field '{}' must be an integer", field),
                });
            }
        }
        ParamSchema::Number => {
            if !val.is_number() {
                return Err(DecodeError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reason: format!("field '{}' must be a number", field),
                });
            }
        }
        ParamSchema::Boolean => {
            if !val.is_boolean() {
                return Err(DecodeError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reason: format!("field '{}' must be a boolean", field),
                });
            }
        }
        ParamSchema::Array { item_type } => {
            let arr = match val.as_array() {
                Some(a) => a,
                None => {
                    return Err(DecodeError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("field '{}' must be an array", field),
                    });
                }
            };
            if let Some(item_schema) = item_type {
                for (i, item) in arr.iter().enumerate() {
                    validate_value(tool_name, &format!("{}[{}]", field, i), item, item_schema)?;
                }
            }
        }
        ParamSchema::Object { props } => {
            let obj = match val.as_object() {
                Some(o) => o,
                None => {
                    return Err(DecodeError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("field '{}' must be an object", field),
                    });
                }
            };
            // Check required nested fields.
            for (name, sub_schema, required) in props {
                if *required && !obj.contains_key(name.as_str()) {
                    return Err(DecodeError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("required nested field '{}.{}' is missing", field, name),
                    });
                }
                if let Some(nested_val) = obj.get(name.as_str()) {
                    validate_value(
                        tool_name,
                        &format!("{}.{}", field, name),
                        nested_val,
                        sub_schema,
                    )?;
                }
            }
        }
        ParamSchema::Opaque => {
            // Opaque schema: any JSON value is acceptable.
        }
    }
    Ok(())
}
