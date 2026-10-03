//! Fail-closed schema and type validation for tool invocations.

use serde_json::Value;

use crate::encoder::parse_json_schema;
use crate::error::CompactToolError;
use crate::types::{ToolDefinition, TypeSchema};

/// Validates that a parsed tool invocation matches its registered `ToolDefinition`.
///
/// Returns `Ok(())` on success, or an explicit `CompactToolError` on any mismatch.
pub fn validate_tool_call(
    tool_name: &str,
    args: &Value,
    tools: &[ToolDefinition],
) -> Result<(), CompactToolError> {
    let Some(tool) = tools.iter().find(|t| t.name == tool_name) else {
        return Err(CompactToolError::UnknownTool {
            tool_name: tool_name.to_string(),
        });
    };

    let Some(params) = &tool.parameters else {
        // No parameters schema defined; must be an object
        if !args.is_object() {
            return Err(CompactToolError::InvalidFieldType {
                tool_name: tool_name.to_string(),
                field: "root".to_string(),
                expected: "object",
                found: value_type_name(args).to_string(),
            });
        }
        return Ok(());
    };

    let type_schema = parse_json_schema(tool_name, params)?;
    validate_value(tool_name, "", args, &type_schema)
}

/// Recursively validates a JSON `Value` against a `TypeSchema`.
fn validate_value(
    tool_name: &str,
    field_path: &str,
    value: &Value,
    schema: &TypeSchema,
) -> Result<(), CompactToolError> {
    match schema {
        TypeSchema::String => {
            if !value.is_string() {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "string",
                    found: value_type_name(value).to_string(),
                });
            }
        }
        TypeSchema::Integer => {
            if !(value.is_i64() || value.is_u64()) {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "integer",
                    found: value_type_name(value).to_string(),
                });
            }
        }
        TypeSchema::Number => {
            if !value.is_number() {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "number",
                    found: value_type_name(value).to_string(),
                });
            }
        }
        TypeSchema::Boolean => {
            if !value.is_boolean() {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "boolean",
                    found: value_type_name(value).to_string(),
                });
            }
        }
        TypeSchema::Null => {
            if !value.is_null() {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "null",
                    found: value_type_name(value).to_string(),
                });
            }
        }
        TypeSchema::Enum(variants) => {
            let Some(s) = value.as_str() else {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "string",
                    found: value_type_name(value).to_string(),
                });
            };
            if !variants.iter().any(|v| v == s) {
                return Err(CompactToolError::InvalidEnumValue {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: variants.clone(),
                    found: s.to_string(),
                });
            }
        }
        TypeSchema::Array(inner) => {
            let Some(arr) = value.as_array() else {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "array",
                    found: value_type_name(value).to_string(),
                });
            };
            for (idx, item) in arr.iter().enumerate() {
                let elem_path = if field_path.is_empty() {
                    format!("[{}]", idx)
                } else {
                    format!("{}[{}]", field_path, idx)
                };
                validate_value(tool_name, &elem_path, item, inner)?;
            }
        }
        TypeSchema::Object {
            properties,
            required,
            additional_properties,
        } => {
            let Some(map) = value.as_object() else {
                return Err(CompactToolError::InvalidFieldType {
                    tool_name: tool_name.to_string(),
                    field: display_field_path(field_path),
                    expected: "object",
                    found: value_type_name(value).to_string(),
                });
            };

            // Check required fields
            for req_field in required {
                if !map.contains_key(req_field) || map.get(req_field).is_none() {
                    let missing_path = if field_path.is_empty() {
                        req_field.clone()
                    } else {
                        format!("{}.{}", field_path, req_field)
                    };
                    return Err(CompactToolError::MissingRequiredField {
                        tool_name: tool_name.to_string(),
                        field: missing_path,
                    });
                }
            }

            // Check property schemas and unexpected fields
            for (key, val) in map {
                let child_path = if field_path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", field_path, key)
                };

                if let Some(prop_schema) = properties.get(key) {
                    validate_value(tool_name, &child_path, val, &prop_schema.schema)?;
                } else if !additional_properties {
                    return Err(CompactToolError::UnexpectedField {
                        tool_name: tool_name.to_string(),
                        field: child_path,
                    });
                }
            }
        }
    }

    Ok(())
}

fn display_field_path(path: &str) -> String {
    if path.is_empty() {
        "root".to_string()
    } else {
        path.to_string()
    }
}

fn value_type_name(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
