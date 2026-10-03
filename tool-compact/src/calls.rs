//! Call rendering, scanning, parsing and strict validation against tool schemas.

use serde_json::Value;

use crate::types::{CompactError, ToolCall, ToolDef};

/// Render a slice of tool calls into the compact grammar:
/// `<<call name {json}>>`
pub fn render_calls(calls: &[ToolCall]) -> String {
    let mut out = String::new();
    for call in calls {
        out.push_str("<<call ");
        out.push_str(&call.name);
        out.push(' ');
        let args_str = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string());
        out.push_str(&args_str);
        out.push_str(">>\n");
    }
    out
}

/// Decode and strictly validate calls from model output text.
/// Text before, between, or after calls is ignored.
/// If no calls are found, returns `Ok(vec![])`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let raw_calls = scan_raw_calls(text)?;
    let mut validated_calls = Vec::with_capacity(raw_calls.len());

    for (name, args_str) in raw_calls {
        let tool = tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| CompactError::UnknownTool(name.clone()))?;

        let args_val: Value =
            serde_json::from_str(&args_str).map_err(|e| CompactError::InvalidArguments {
                tool: name.clone(),
                reason: format!("JSON parse error: {e}"),
            })?;

        validate_tool_arguments(tool, &args_val)?;

        validated_calls.push(ToolCall {
            name,
            arguments: args_val,
        });
    }

    Ok(validated_calls)
}

/// JSON-aware scanner for `<<call NAME {JSON_OBJECT}>>`.
/// Tracks string escapes and brace depth so that `>>` or `<<` inside JSON strings
/// do not terminate or open a call.
pub fn scan_raw_calls(text: &str) -> Result<Vec<(String, String)>, CompactError> {
    let mut calls = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if i + 7 <= len && &bytes[i..i + 7] == b"<<call " {
            i += 7;
            while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }

            let name_start = i;
            while i < len
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
            {
                i += 1;
            }
            let name = match std::str::from_utf8(&bytes[name_start..i]) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    i += 1;
                    continue;
                }
            };

            if name.is_empty() {
                continue;
            }

            while i < len
                && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r' || bytes[i] == b'\n')
            {
                i += 1;
            }

            if i >= len || bytes[i] != b'{' {
                continue;
            }

            let json_start = i;
            let mut brace_depth = 0;
            let mut in_str = false;
            let mut in_escape = false;
            let mut json_end = None;

            while i < len {
                let b = bytes[i];
                if in_escape {
                    in_escape = false;
                    i += 1;
                    continue;
                }

                if b == b'\\' && in_str {
                    in_escape = true;
                    i += 1;
                    continue;
                }

                if b == b'"' {
                    in_str = !in_str;
                    i += 1;
                    continue;
                }

                if !in_str {
                    if b == b'{' {
                        brace_depth += 1;
                    } else if b == b'}' {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            json_end = Some(i + 1);
                            i += 1;
                            break;
                        }
                    }
                }
                i += 1;
            }

            let Some(end_idx) = json_end else {
                continue;
            };

            while i < len
                && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r' || bytes[i] == b'\n')
            {
                i += 1;
            }

            if i + 2 <= len && &bytes[i..i + 2] == b">>" {
                i += 2;
                if let Ok(json_str) = std::str::from_utf8(&bytes[json_start..end_idx]) {
                    calls.push((name, json_str.to_string()));
                }
            }
        } else {
            i += 1;
        }
    }

    Ok(calls)
}

/// Validate arguments against tool JSON Schema (fail closed).
pub fn validate_tool_arguments(tool: &ToolDef, args: &Value) -> Result<(), CompactError> {
    let Some(args_obj) = args.as_object() else {
        return Err(CompactError::InvalidArguments {
            tool: tool.name.clone(),
            reason: "Arguments must be a JSON object".to_string(),
        });
    };

    let Some(params) = &tool.parameters else {
        return Ok(());
    };

    let Some(params_obj) = params.as_object() else {
        return Ok(());
    };

    if let Some(req_arr) = params_obj.get("required").and_then(Value::as_array) {
        for req in req_arr {
            if let Some(key) = req.as_str()
                && !args_obj.contains_key(key)
            {
                return Err(CompactError::InvalidArguments {
                    tool: tool.name.clone(),
                    reason: format!("Missing required property '{key}'"),
                });
            }
        }
    }

    let properties = params_obj.get("properties").and_then(Value::as_object);
    let additional_props_allowed = params_obj
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    for (key, val) in args_obj {
        if let Some(props) = properties {
            if let Some(prop_schema) = props.get(key) {
                validate_value_against_schema(&tool.name, key, val, prop_schema)?;
            } else if !additional_props_allowed {
                return Err(CompactError::InvalidArguments {
                    tool: tool.name.clone(),
                    reason: format!("Unexpected property '{key}'"),
                });
            }
        }
    }

    Ok(())
}

fn validate_value_against_schema(
    tool_name: &str,
    field_name: &str,
    val: &Value,
    schema: &Value,
) -> Result<(), CompactError> {
    if let Some(enum_vals) = schema.get("enum").and_then(Value::as_array)
        && !enum_vals.contains(val)
    {
        return Err(CompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: format!("Value {val} for '{field_name}' not in allowed enum variants"),
        });
    }

    if let Some(const_val) = schema.get("const")
        && val != const_val
    {
        return Err(CompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: format!("Value {val} for '{field_name}' does not match const {const_val}"),
        });
    }

    if let Some(type_val) = schema.get("type") {
        let matches = match type_val {
            Value::String(s) => check_type(s, val),
            Value::Array(arr) => arr.iter().any(|t| {
                if let Some(s) = t.as_str() {
                    check_type(s, val)
                } else {
                    false
                }
            }),
            _ => true,
        };

        if !matches {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!(
                    "Field '{field_name}' has invalid type; expected {type_val}, got {val}"
                ),
            });
        }
    }

    if let Some(s) = val.as_str() {
        if let Some(min) = schema.get("minLength").and_then(Value::as_i64)
            && (s.chars().count() as i64) < min
        {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' length is less than minLength {min}"),
            });
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_i64)
            && (s.chars().count() as i64) > max
        {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' length is greater than maxLength {max}"),
            });
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
            && let Ok(re) = regex::Regex::new(pattern)
            && !re.is_match(s)
        {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' does not match pattern /{pattern}/"),
            });
        }
        if let Some(format) = schema.get("format").and_then(Value::as_str) {
            validate_string_format(tool_name, field_name, s, format)?;
        }
    }

    if let Some(n) = val.as_i64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_i64)
            && n < min
        {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' is less than minimum {min}"),
            });
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_i64)
            && n > max
        {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' is greater than maximum {max}"),
            });
        }
    } else if let Some(n) = val.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' is less than minimum {min}"),
            });
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            return Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' is greater than maximum {max}"),
            });
        }
    }

    if let Some(arr) = val.as_array()
        && let Some(items_schema) = schema.get("items")
    {
        for (idx, item) in arr.iter().enumerate() {
            validate_value_against_schema(
                tool_name,
                &format!("{field_name}[{idx}]"),
                item,
                items_schema,
            )?;
        }
    }

    if let Some(obj) = val.as_object() {
        if let Some(req_arr) = schema.get("required").and_then(Value::as_array) {
            for req in req_arr {
                if let Some(key) = req.as_str()
                    && !obj.contains_key(key)
                {
                    return Err(CompactError::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("Missing required property '{field_name}.{key}'"),
                    });
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (k, v) in obj {
                if let Some(sub_schema) = props.get(k) {
                    validate_value_against_schema(
                        tool_name,
                        &format!("{field_name}.{k}"),
                        v,
                        sub_schema,
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn check_type(expected: &str, val: &Value) -> bool {
    match expected {
        "string" => val.is_string(),
        "integer" => {
            val.is_i64()
                || (val.is_number() && val.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false))
        }
        "number" => val.is_number(),
        "boolean" => val.is_boolean(),
        "null" => val.is_null(),
        "array" => val.is_array(),
        "object" => val.is_object(),
        _ => true,
    }
}

fn validate_string_format(
    tool_name: &str,
    field_name: &str,
    val: &str,
    format: &str,
) -> Result<(), CompactError> {
    match format {
        "date-time" if !is_rfc3339_datetime(val) => Err(CompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: format!("Field '{field_name}' is not a valid RFC 3339 date-time: {val}"),
        }),
        "date" if !is_iso_date(val) => Err(CompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: format!("Field '{field_name}' is not a valid date: {val}"),
        }),
        "time" if !is_iso_time(val) => Err(CompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: format!("Field '{field_name}' is not a valid time: {val}"),
        }),
        "email" if !val.contains('@') || val.starts_with('@') || val.ends_with('@') => {
            Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' is not a valid email: {val}"),
            })
        }
        "uri" if !val.contains("://") && !val.starts_with("urn:") => {
            Err(CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("Field '{field_name}' is not a valid URI: {val}"),
            })
        }
        "uuid" if !is_uuid(val) => Err(CompactError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: format!("Field '{field_name}' is not a valid UUID: {val}"),
        }),
        _ => Ok(()),
    }
}

fn is_rfc3339_datetime(s: &str) -> bool {
    if s.len() < 19 {
        return false;
    }
    let bytes = s.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return false;
    }
    true
}

fn is_iso_date(s: &str) -> bool {
    if s.len() != 10 {
        return false;
    }
    let bytes = s.as_bytes();
    bytes[4] == b'-' && bytes[7] == b'-'
}

fn is_iso_time(s: &str) -> bool {
    if s.len() < 8 {
        return false;
    }
    let bytes = s.as_bytes();
    bytes[2] == b':' && bytes[5] == b':'
}

fn is_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes.iter().enumerate().all(|(idx, &b)| {
            if idx == 8 || idx == 13 || idx == 18 || idx == 23 {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
