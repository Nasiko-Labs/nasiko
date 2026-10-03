//! Validate decoded tool calls against the ORIGINAL `ToolDef` JSON schemas.
//!
//! Fail-closed: unknown tool → [`CompactError::UnknownTool`], any invalid
//! argument → [`CompactError::InvalidArguments`]. Never guesses or silently
//! alters a call.
//!
//! # Strictness
//!
//! - **Required fields**: must be present.
//! - **Unknown keys**: rejected (not in `properties`).
//! - **Type checks**: `integer` rejects floats; `number` rejects bools/strings;
//!   `boolean` rejects everything else; `string` rejects non-strings.
//! - **Enum values**: must match exactly.
//! - **DateTime format**: basic RFC 3339 structural check.
//! - **Arrays**: element types validated recursively.
//! - **Nested objects**: required fields and types validated recursively.

use serde_json::Value;

use crate::error::CompactError;
use crate::types::ToolDef;

/// Validate a decoded call (name + parsed args) against the original tool set.
pub fn validate_call(name: &str, args: &Value, tools: &[ToolDef]) -> Result<(), CompactError> {
    let tool = tools
        .iter()
        .find(|t| t.function.name == name)
        .ok_or_else(|| CompactError::UnknownTool {
            name: name.to_string(),
        })?;

    if let Some(schema) = tool.function.parameters.as_ref() {
        validate_object(name, args, schema)?;
    }

    Ok(())
}

/// Validate `args` as an object against a JSON Schema with `properties` and
/// `required`.
fn validate_object(tool: &str, args: &Value, schema: &Value) -> Result<(), CompactError> {
    let args_obj = args
        .as_object()
        .ok_or_else(|| CompactError::InvalidArguments {
            tool: tool.to_string(),
            details: "arguments must be a JSON object".to_string(),
        })?;

    let properties = schema.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    // Required fields must be present.
    for field in &required {
        if !args_obj.contains_key(*field) {
            return Err(CompactError::InvalidArguments {
                tool: tool.to_string(),
                details: format!("missing required field: {field}"),
            });
        }
    }

    // Unknown keys rejected.
    if let Some(props) = properties {
        for key in args_obj.keys() {
            if !props.contains_key(key) {
                return Err(CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    details: format!("unknown argument key: {key}"),
                });
            }
        }
    }

    // Type-check each provided field.
    if let Some(props) = properties {
        for (key, value) in args_obj {
            if let Some(field_schema) = props.get(key) {
                validate_value(tool, key, value, field_schema)?;
            }
        }
    }

    Ok(())
}

fn validate_value(
    tool: &str,
    field: &str,
    value: &Value,
    schema: &Value,
) -> Result<(), CompactError> {
    // Enum check first.
    if let Some(enum_vals) = schema.get("enum").and_then(Value::as_array) {
        if !enum_vals.contains(value) {
            return Err(CompactError::InvalidArguments {
                tool: tool.to_string(),
                details: format!("field '{field}': value {} not in enum", value),
            });
        }
        return Ok(());
    }

    let type_str = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("string");

    match type_str {
        "string" => {
            if !value.is_string() {
                return Err(CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    details: format!("field '{field}': expected string"),
                });
            }
            // Check datetime format if specified.
            if let Some("date-time") = schema.get("format").and_then(Value::as_str) {
                let s = value.as_str().unwrap_or_default();
                if !is_valid_rfc3339(s) {
                    return Err(CompactError::InvalidArguments {
                        tool: tool.to_string(),
                        details: format!("field '{field}': malformed RFC 3339 datetime"),
                    });
                }
            }
        }
        "integer" => match value {
            Value::Number(n) => {
                if !n.is_i64() && !n.is_u64() {
                    return Err(CompactError::InvalidArguments {
                        tool: tool.to_string(),
                        details: format!("field '{field}': expected integer, got float"),
                    });
                }
            }
            _ => {
                return Err(CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    details: format!("field '{field}': expected integer"),
                });
            }
        },
        "number" => {
            if !value.is_number() {
                return Err(CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    details: format!("field '{field}': expected number"),
                });
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err(CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    details: format!("field '{field}': expected boolean"),
                });
            }
        }
        "array" => {
            let arr = value
                .as_array()
                .ok_or_else(|| CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    details: format!("field '{field}': expected array"),
                })?;
            if let Some(items_schema) = schema.get("items") {
                for (i, item) in arr.iter().enumerate() {
                    validate_value(tool, &format!("{field}[{i}]"), item, items_schema)?;
                }
            }
        }
        "object" => {
            if !value.is_object() {
                return Err(CompactError::InvalidArguments {
                    tool: tool.to_string(),
                    details: format!("field '{field}': expected object"),
                });
            }
            validate_object(tool, value, schema)?;
        }
        _ => {}
    }

    Ok(())
}

/// Basic RFC 3339 structural check (enough to reject obvious garbage without
/// pulling in a datetime crate).
///
/// Accepts: `YYYY-MM-DDTHH:MM:SS[.frac](Z|+HH:MM|-HH:MM)`
fn is_valid_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    // Minimum: 2000-01-01T00:00:00Z = 20 bytes
    if b.len() < 20 {
        return false;
    }

    // Date: YYYY-MM-DD
    let date_ok = b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit);
    if !date_ok {
        return false;
    }

    // T separator
    if b[10] != b'T' && b[10] != b't' {
        return false;
    }

    // Time: HH:MM:SS
    let time_ok = b[11..13].iter().all(u8::is_ascii_digit)
        && b[13] == b':'
        && b[14..16].iter().all(u8::is_ascii_digit)
        && b[16] == b':'
        && b[17..19].iter().all(u8::is_ascii_digit);
    if !time_ok {
        return false;
    }

    // Optional fractional seconds.
    let mut i = 19;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return false; // dot with no digits
        }
    }

    // Timezone: Z | +HH:MM | -HH:MM
    if i >= b.len() {
        return false;
    }
    if b[i] == b'Z' || b[i] == b'z' {
        return i + 1 == b.len();
    }
    if b[i] == b'+' || b[i] == b'-' {
        if i + 6 != b.len() {
            return false;
        }
        return b[i + 1..i + 3].iter().all(u8::is_ascii_digit)
            && b[i + 3] == b':'
            && b[i + 4..i + 6].iter().all(u8::is_ascii_digit);
    }

    false
}
