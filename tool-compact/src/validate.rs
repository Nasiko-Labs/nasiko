use serde_json::Value;
use std::collections::HashSet;

use crate::error::CompactError;

/// Compares two JSON values for structural equality under Section 1.7:
/// - Ignore JSON object-member ordering.
/// - Preserve all arrays and their order.
/// - Distinguish absent values from empty values.
/// - Preserve description strings exactly.
/// - Preserve numeric values without coercion.
pub fn schemas_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(map_a), Value::Object(map_b)) => {
            if map_a.len() != map_b.len() {
                return false;
            }
            for (k, v_a) in map_a {
                match map_b.get(k) {
                    Some(v_b) => {
                        if !schemas_equal(v_a, v_b) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        (Value::Array(arr_a), Value::Array(arr_b)) => {
            if arr_a.len() != arr_b.len() {
                return false;
            }
            for (v_a, v_b) in arr_a.iter().zip(arr_b.iter()) {
                if !schemas_equal(v_a, v_b) {
                    return false;
                }
            }
            true
        }
        (Value::Number(num_a), Value::Number(num_b)) => {
            // Compare without lossy float coercion
            if let (Some(ia), Some(ib)) = (num_a.as_i64(), num_b.as_i64()) {
                ia == ib
            } else if let (Some(ua), Some(ub)) = (num_a.as_u64(), num_b.as_u64()) {
                ua == ub
            } else if let (Some(fa), Some(fb)) = (num_a.as_f64(), num_b.as_f64()) {
                fa.to_bits() == fb.to_bits()
            } else {
                num_a == num_b
            }
        }
        _ => a == b,
    }
}

/// Validates parsed arguments against the tool's original parameter schema.
pub fn validate_arguments(
    tool_name: &str,
    arguments: &Value,
    schema: &Value,
    assert_date_time: bool,
) -> Result<(), CompactError> {
    if !arguments.is_object() {
        return Err(CompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: "arguments must be a JSON object".to_string(),
            pointer: None,
        });
    }

    validate_value(tool_name, arguments, schema, "", assert_date_time)
}

fn validate_value(
    tool_name: &str,
    val: &Value,
    schema: &Value,
    pointer: &str,
    assert_date_time: bool,
) -> Result<(), CompactError> {
    let schema_obj = match schema.as_object() {
        Some(o) => o,
        None => return Ok(()),
    };

    // 1. Check type
    if let Some(type_val) = schema_obj.get("type").and_then(|t| t.as_str()) {
        match type_val {
            "string" => {
                if !val.is_string() {
                    return Err(CompactError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("expected string, got {:?}", val),
                        pointer: Some(pointer.to_string()),
                    });
                }
                let s = val.as_str().unwrap();

                // Format validation (date-time)
                if assert_date_time {
                    if let Some("date-time") = schema_obj.get("format").and_then(|f| f.as_str()) {
                        if chrono::DateTime::parse_from_rfc3339(s).is_err() {
                            return Err(CompactError::InvalidArguments {
                                tool: tool_name.to_string(),
                                reason: format!("string '{}' is not a valid RFC 3339 date-time", s),
                                pointer: Some(pointer.to_string()),
                            });
                        }
                    }
                }

                // minLength / maxLength
                if let Some(min) = schema_obj.get("minLength").and_then(|m| m.as_u64()) {
                    if (s.chars().count() as u64) < min {
                        return Err(CompactError::InvalidArguments {
                            tool: tool_name.to_string(),
                            reason: format!("string length {} is less than minLength {}", s.chars().count(), min),
                            pointer: Some(pointer.to_string()),
                        });
                    }
                }
                if let Some(max) = schema_obj.get("maxLength").and_then(|m| m.as_u64()) {
                    if (s.chars().count() as u64) > max {
                        return Err(CompactError::InvalidArguments {
                            tool: tool_name.to_string(),
                            reason: format!("string length {} is greater than maxLength {}", s.chars().count(), max),
                            pointer: Some(pointer.to_string()),
                        });
                    }
                }
            }
            "integer" => {
                let is_int = val.is_i64() || val.is_u64();
                if !is_int {
                    return Err(CompactError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("expected integer, got {:?}", val),
                        pointer: Some(pointer.to_string()),
                    });
                }
            }
            "number" => {
                if !val.is_number() {
                    return Err(CompactError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("expected number, got {:?}", val),
                        pointer: Some(pointer.to_string()),
                    });
                }
            }
            "boolean" => {
                if !val.is_boolean() {
                    return Err(CompactError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("expected boolean, got {:?}", val),
                        pointer: Some(pointer.to_string()),
                    });
                }
            }
            "null" => {
                if !val.is_null() {
                    return Err(CompactError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("expected null, got {:?}", val),
                        pointer: Some(pointer.to_string()),
                    });
                }
            }
            "array" => {
                let arr = val.as_array().ok_or_else(|| CompactError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reason: format!("expected array, got {:?}", val),
                    pointer: Some(pointer.to_string()),
                })?;

                if let Some(min) = schema_obj.get("minItems").and_then(|m| m.as_u64()) {
                    if (arr.len() as u64) < min {
                        return Err(CompactError::InvalidArguments {
                            tool: tool_name.to_string(),
                            reason: format!("array length {} is less than minItems {}", arr.len(), min),
                            pointer: Some(pointer.to_string()),
                        });
                    }
                }
                if let Some(max) = schema_obj.get("maxItems").and_then(|m| m.as_u64()) {
                    if (arr.len() as u64) > max {
                        return Err(CompactError::InvalidArguments {
                            tool: tool_name.to_string(),
                            reason: format!("array length {} is greater than maxItems {}", arr.len(), max),
                            pointer: Some(pointer.to_string()),
                        });
                    }
                }
                if let Some(true) = schema_obj.get("uniqueItems").and_then(|u| u.as_bool()) {
                    for i in 0..arr.len() {
                        for j in (i + 1)..arr.len() {
                            if schemas_equal(&arr[i], &arr[j]) {
                                return Err(CompactError::InvalidArguments {
                                    tool: tool_name.to_string(),
                                    reason: "array items must be unique".to_string(),
                                    pointer: Some(pointer.to_string()),
                                });
                            }
                        }
                    }
                }

                if let Some(item_schema) = schema_obj.get("items") {
                    for (i, item) in arr.iter().enumerate() {
                        let sub_ptr = format!("{}/{}", pointer, i);
                        validate_value(tool_name, item, item_schema, &sub_ptr, assert_date_time)?;
                    }
                }
            }
            "object" => {
                let obj = val.as_object().ok_or_else(|| CompactError::InvalidArguments {
                    tool: tool_name.to_string(),
                    reason: format!("expected object, got {:?}", val),
                    pointer: Some(pointer.to_string()),
                })?;

                if let Some(min) = schema_obj.get("minProperties").and_then(|m| m.as_u64()) {
                    if (obj.len() as u64) < min {
                        return Err(CompactError::InvalidArguments {
                            tool: tool_name.to_string(),
                            reason: format!("object size {} is less than minProperties {}", obj.len(), min),
                            pointer: Some(pointer.to_string()),
                        });
                    }
                }
                if let Some(max) = schema_obj.get("maxProperties").and_then(|m| m.as_u64()) {
                    if (obj.len() as u64) > max {
                        return Err(CompactError::InvalidArguments {
                            tool: tool_name.to_string(),
                            reason: format!("object size {} is greater than maxProperties {}", obj.len(), max),
                            pointer: Some(pointer.to_string()),
                        });
                    }
                }

                // Check required
                if let Some(req_arr) = schema_obj.get("required").and_then(|r| r.as_array()) {
                    for item in req_arr {
                        if let Some(key) = item.as_str() {
                            if !obj.contains_key(key) {
                                return Err(CompactError::InvalidArguments {
                                    tool: tool_name.to_string(),
                                    reason: format!("missing required property '{}'", key),
                                    pointer: Some(format!("{}/{}", pointer, key)),
                                });
                            }
                        }
                    }
                }

                let props = schema_obj.get("properties").and_then(|p| p.as_object());

                // Check additionalProperties: false
                if let Some(false) = schema_obj.get("additionalProperties").and_then(|a| a.as_bool()) {
                    let allowed_keys: HashSet<&str> = props.map(|p| p.keys().map(|s| s.as_str()).collect()).unwrap_or_default();
                    for k in obj.keys() {
                        if !allowed_keys.contains(k.as_str()) {
                            return Err(CompactError::InvalidArguments {
                                tool: tool_name.to_string(),
                                reason: format!("additional property '{}' not permitted", k),
                                pointer: Some(format!("{}/{}", pointer, k)),
                            });
                        }
                    }
                }

                // Validate properties
                if let Some(props_map) = props {
                    for (k, v) in obj {
                        if let Some(prop_schema) = props_map.get(k) {
                            let sub_ptr = format!("{}/{}", pointer, k);
                            validate_value(tool_name, v, prop_schema, &sub_ptr, assert_date_time)?;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // 2. Check enum
    if let Some(enum_arr) = schema_obj.get("enum").and_then(|e| e.as_array()) {
        let matched = enum_arr.iter().any(|item| schemas_equal(item, val));
        if !matched {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("value {:?} is not one of allowed enum variants {:?}", val, enum_arr),
                pointer: Some(pointer.to_string()),
            });
        }
    }

    Ok(())
}
