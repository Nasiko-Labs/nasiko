use crate::error::{Result, ToolCompactError};
use crate::schema::{ParameterSchema, ToolRegistry, ToolSchema, ValueType};
use serde_json::Value;

/// Validates a tool call name and arguments against a `ToolRegistry`.
///
/// Returns the arguments exactly as given on success (no defaults are injected,
/// nothing is altered), or a `ToolCompactError` on failure.
pub fn validate_call(tool_name: &str, args: &Value, registry: &ToolRegistry) -> Result<Value> {
    let schema = registry
        .get(tool_name)
        .ok_or_else(|| ToolCompactError::UnknownTool(tool_name.to_string()))?;

    validate_call_with_schema(schema, args)
}

/// Validates arguments directly against a `ToolSchema`.
pub fn validate_call_with_schema(schema: &ToolSchema, args: &Value) -> Result<Value> {
    let map = match args {
        Value::Object(m) => m,
        _ => {
            return Err(ToolCompactError::InvalidArguments {
                tool: schema.name.clone(),
                reasoning: "Tool arguments must be a JSON object".to_string(),
            });
        }
    };

    validate_object(&schema.name, &schema.parameters, map)?;
    Ok(Value::Object(map.clone()))
}

/// Shared by the top level and nested objects: required fields, types, unknown keys.
fn validate_object(
    tool: &str,
    params: &[ParameterSchema],
    map: &serde_json::Map<String, Value>,
) -> Result<()> {
    for param in params {
        match map.get(&param.name) {
            Some(val) => validate_parameter(tool, param, val)?,
            None if param.required => {
                return Err(ToolCompactError::InvalidArguments {
                    tool: tool.to_string(),
                    reasoning: format!("Missing required parameter '{}'", param.name),
                });
            }
            None => {}
        }
    }

    for key in map.keys() {
        if !params.iter().any(|p| &p.name == key) {
            return Err(ToolCompactError::InvalidArguments {
                tool: tool.to_string(),
                reasoning: format!("Unrecognized parameter '{}'", key),
            });
        }
    }
    Ok(())
}

fn validate_parameter(tool_name: &str, param: &ParameterSchema, val: &Value) -> Result<()> {
    // Type checking
    let type_ok = match param.val_type {
        ValueType::String => val.is_string(),
        ValueType::Integer => val.is_i64() || val.is_u64(),
        ValueType::Number => val.is_number(),
        ValueType::Boolean => val.is_boolean(),
        ValueType::Array => val.is_array(),
        ValueType::Object => val.is_object(),
    };

    if !type_ok {
        return Err(ToolCompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reasoning: format!(
                "Parameter '{}' expected type {}, found value {:?}",
                param.name, param.val_type, val
            ),
        });
    }

    // date-time format check (RFC 3339)
    if param.format.as_deref() == Some("date-time") {
        if let Some(s) = val.as_str() {
            if !is_rfc3339(s) {
                return Err(ToolCompactError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reasoning: format!(
                        "Parameter '{}' value '{}' is not an RFC 3339 date-time",
                        param.name, s
                    ),
                });
            }
        }
    }

    // Nested objects: recurse so nested required fields and types are enforced
    if let (Some(props), Some(obj)) = (&param.properties, val.as_object()) {
        validate_object(tool_name, props, obj)?;
    }

    // Enum value checking
    if let Some(ref enum_vals) = param.enum_values {
        if let Some(s) = val.as_str() {
            if !enum_vals.contains(&s.to_string()) {
                return Err(ToolCompactError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reasoning: format!(
                        "Parameter '{}' value '{}' is not one of allowed enum values: [{}]",
                        param.name,
                        s,
                        enum_vals.join(", ")
                    ),
                });
            }
        }
    }

    // Item type checking for arrays
    if param.val_type == ValueType::Array {
        if let (Some(item_type), Some(arr)) = (&param.item_type, val.as_array()) {
            for (idx, item) in arr.iter().enumerate() {
                let item_ok = match item_type.as_ref() {
                    ValueType::String => item.is_string(),
                    ValueType::Integer => item.is_i64() || item.is_u64(),
                    ValueType::Number => item.is_number(),
                    ValueType::Boolean => item.is_boolean(),
                    ValueType::Array => item.is_array(),
                    ValueType::Object => item.is_object(),
                };
                if !item_ok {
                    return Err(ToolCompactError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reasoning: format!(
                            "Array parameter '{}' element at index {} expected type {}, found {:?}",
                            param.name, idx, item_type, item
                        ),
                    });
                }
            }
        }
    }

    Ok(())
}

/// Strict structural check for RFC 3339 timestamps, e.g. 2026-10-05T15:00:00+05:30
fn is_rfc3339(s: &str) -> bool {
    if !s.is_ascii() || s.len() < 20 {
        return false;
    }
    let b = s.as_bytes();
    let digits = |from: usize, to: usize| b[from..to].iter().all(|c| c.is_ascii_digit());
    let num = |from: usize, to: usize| s[from..to].parse::<u32>().unwrap_or(u32::MAX);

    if !(digits(0, 4)
        && b[4] == b'-'
        && digits(5, 7)
        && b[7] == b'-'
        && digits(8, 10)
        && (b[10] == b'T' || b[10] == b't')
        && digits(11, 13)
        && b[13] == b':'
        && digits(14, 16)
        && b[16] == b':'
        && digits(17, 19))
    {
        return false;
    }
    if !((1..=12).contains(&num(5, 7))
        && (1..=31).contains(&num(8, 10))
        && num(11, 13) <= 23
        && num(14, 16) <= 59
        && num(17, 19) <= 60)
    {
        return false;
    }

    let mut i = 19;
    if b[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    match &b[i..] {
        [b'Z'] | [b'z'] => true,
        [sign, ..] if (*sign == b'+' || *sign == b'-') && b.len() - i == 6 => {
            digits(i + 1, i + 3)
                && b[i + 3] == b':'
                && digits(i + 4, i + 6)
                && num(i + 1, i + 3) <= 23
                && num(i + 4, i + 6) <= 59
        }
        _ => false,
    }
}