//! Strict JSON schema validation for tool call arguments.

use crate::error::Error;
use serde_json::Value;

/// Validate tool arguments against the tool's parameters schema.
pub fn validate_arguments(
    tool_name: &str,
    args: &serde_json::Map<String, Value>,
    param_map: &serde_json::Map<String, Value>,
) -> Result<(), Error> {
    validate_object(tool_name, "", args, param_map)
}

fn validate_object(
    tool_name: &str,
    path: &str,
    args: &serde_json::Map<String, Value>,
    schema: &serde_json::Map<String, Value>,
) -> Result<(), Error> {
    // 1. Check required fields
    if let Some(req_arr) = schema.get("required").and_then(Value::as_array) {
        for req in req_arr {
            if let Some(rname) = req.as_str()
                && !args.contains_key(rname)
            {
                let field_desc = if path.is_empty() {
                    rname.to_string()
                } else {
                    format!("{path}.{rname}")
                };
                return Err(Error::InvalidArguments(format!(
                    "missing required field '{field_desc}' in tool '{tool_name}'"
                )));
            }
        }
    }

    // 2. Reject unknown extra fields (D8: additionalProperties: false)
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for key in args.keys() {
        if !properties.contains_key(key) {
            let field_desc = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            return Err(Error::InvalidArguments(format!(
                "unknown field '{field_desc}' in tool '{tool_name}'"
            )));
        }
    }

    // 3. Validate properties present
    for (key, val) in args {
        if let Some(prop_schema) = properties.get(key).and_then(Value::as_object) {
            let field_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            validate_value(tool_name, &field_path, val, prop_schema)?;
        }
    }

    Ok(())
}

fn validate_value(
    tool_name: &str,
    path: &str,
    val: &Value,
    schema: &serde_json::Map<String, Value>,
) -> Result<(), Error> {
    // Check enum
    if let Some(enum_vals) = schema.get("enum").and_then(Value::as_array) {
        let s = val.as_str().ok_or_else(|| {
            Error::InvalidArguments(format!("field '{path}' must be a string for enum"))
        })?;
        if !enum_vals.iter().any(|ev| ev.as_str() == Some(s)) {
            return Err(Error::InvalidArguments(format!(
                "invalid enum value '{s}' for field '{path}' in tool '{tool_name}'"
            )));
        }
        return Ok(());
    }

    let expected_type = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("string");
    match expected_type {
        "string" => {
            let s = val.as_str().ok_or_else(|| {
                Error::InvalidArguments(format!("field '{path}' must be a string"))
            })?;
            if let Some(fmt) = schema.get("format").and_then(Value::as_str) {
                validate_string_format(tool_name, path, s, fmt)?;
            }
        }
        "integer" => {
            if !val.is_i64() && !val.is_u64() {
                return Err(Error::InvalidArguments(format!(
                    "field '{path}' must be an integer in tool '{tool_name}'"
                )));
            }
        }
        "number" => {
            if !val.is_number() {
                return Err(Error::InvalidArguments(format!(
                    "field '{path}' must be a number in tool '{tool_name}'"
                )));
            }
        }
        "boolean" => {
            if !val.is_boolean() {
                return Err(Error::InvalidArguments(format!(
                    "field '{path}' must be a boolean in tool '{tool_name}'"
                )));
            }
        }
        "array" => {
            let arr = val.as_array().ok_or_else(|| {
                Error::InvalidArguments(format!("field '{path}' must be an array"))
            })?;
            if let Some(item_schema) = schema.get("items").and_then(Value::as_object) {
                for (idx, item) in arr.iter().enumerate() {
                    let item_path = format!("{path}[{idx}]");
                    validate_value(tool_name, &item_path, item, item_schema)?;
                }
            }
        }
        "object" => {
            let obj = val.as_object().ok_or_else(|| {
                Error::InvalidArguments(format!("field '{path}' must be an object"))
            })?;
            validate_object(tool_name, path, obj, schema)?;
        }
        _ => {}
    }

    Ok(())
}

fn validate_string_format(tool_name: &str, path: &str, s: &str, format: &str) -> Result<(), Error> {
    match format {
        "date-time" if !is_valid_datetime(s) => Err(Error::InvalidArguments(format!(
            "invalid date-time '{s}' for field '{path}' in tool '{tool_name}'"
        ))),
        "date" if !is_valid_date(s) => Err(Error::InvalidArguments(format!(
            "invalid date '{s}' for field '{path}' in tool '{tool_name}'"
        ))),
        "time" if !is_valid_time(s) => Err(Error::InvalidArguments(format!(
            "invalid time '{s}' for field '{path}' in tool '{tool_name}'"
        ))),
        "email" if !is_valid_email(s) => Err(Error::InvalidArguments(format!(
            "invalid email '{s}' for field '{path}' in tool '{tool_name}'"
        ))),
        "uri" if !is_valid_uri(s) => Err(Error::InvalidArguments(format!(
            "invalid uri '{s}' for field '{path}' in tool '{tool_name}'"
        ))),
        "uuid" if !is_valid_uuid(s) => Err(Error::InvalidArguments(format!(
            "invalid uuid '{s}' for field '{path}' in tool '{tool_name}'"
        ))),
        _ => Ok(()),
    }
}

fn parse_2digits(b1: u8, b2: u8) -> Option<u32> {
    if b1.is_ascii_digit() && b2.is_ascii_digit() {
        Some((b1 - b'0') as u32 * 10 + (b2 - b'0') as u32)
    } else {
        None
    }
}

fn is_valid_datetime(s: &str) -> bool {
    if s.len() < 19 {
        return false;
    }
    let b = s.as_bytes();
    if !b[0..4].iter().all(u8::is_ascii_digit) || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let month = match parse_2digits(b[5], b[6]) {
        Some(m) if (1..=12).contains(&m) => m,
        _ => return false,
    };
    let day = match parse_2digits(b[8], b[9]) {
        Some(d) if (1..=31).contains(&d) => d,
        _ => return false,
    };
    let _ = (month, day);

    if b[10] != b'T' && b[10] != b't' {
        return false;
    }
    if b[13] != b':' || b[16] != b':' {
        return false;
    }
    let hour = match parse_2digits(b[11], b[12]) {
        Some(h) if (0..=23).contains(&h) => h,
        _ => return false,
    };
    let minute = match parse_2digits(b[14], b[15]) {
        Some(m) if (0..=59).contains(&m) => m,
        _ => return false,
    };
    let second = match parse_2digits(b[17], b[18]) {
        Some(sec) if (0..=60).contains(&sec) => sec,
        _ => return false,
    };
    let _ = (hour, minute, second);

    let rest = &s[19..];
    if rest.is_empty() {
        return false;
    }
    let rest = if let Some(dot_idx) = rest.find('.') {
        let frac = &rest[dot_idx + 1..];
        let tz_idx = frac.find(['Z', 'z', '+', '-']);
        match tz_idx {
            Some(idx) => &frac[idx..],
            None => return false,
        }
    } else {
        rest
    };
    if rest == "Z" || rest == "z" {
        return true;
    }
    if (rest.starts_with('+') || rest.starts_with('-')) && rest.len() == 6 {
        let b = rest.as_bytes();
        let tz_h = match parse_2digits(b[1], b[2]) {
            Some(h) if (0..=23).contains(&h) => h,
            _ => return false,
        };
        let tz_m = match parse_2digits(b[4], b[5]) {
            Some(m) if (0..=59).contains(&m) => m,
            _ => return false,
        };
        let _ = (tz_h, tz_m);
        return b[3] == b':';
    }
    false
}

fn is_valid_date(s: &str) -> bool {
    if s.len() != 10 {
        return false;
    }
    let b = s.as_bytes();
    if !b[0..4].iter().all(u8::is_ascii_digit) || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let month = match parse_2digits(b[5], b[6]) {
        Some(m) if (1..=12).contains(&m) => m,
        _ => return false,
    };
    let day = match parse_2digits(b[8], b[9]) {
        Some(d) if (1..=31).contains(&d) => d,
        _ => return false,
    };
    let _ = (month, day);
    true
}

fn is_valid_time(s: &str) -> bool {
    if s.len() < 8 {
        return false;
    }
    let b = s.as_bytes();
    if b[2] != b':' || b[5] != b':' {
        return false;
    }
    let hour = match parse_2digits(b[0], b[1]) {
        Some(h) if (0..=23).contains(&h) => h,
        _ => return false,
    };
    let minute = match parse_2digits(b[3], b[4]) {
        Some(m) if (0..=59).contains(&m) => m,
        _ => return false,
    };
    let second = match parse_2digits(b[6], b[7]) {
        Some(sec) if (0..=60).contains(&sec) => sec,
        _ => return false,
    };
    let _ = (hour, minute, second);
    true
}

fn is_valid_email(s: &str) -> bool {
    if s.len() < 3 || !s.contains('@') {
        return false;
    }
    let parts: Vec<&str> = s.split('@').collect();
    parts.len() == 2 && !parts[0].is_empty() && parts[1].contains('.') && !parts[1].ends_with('.')
}

fn is_valid_uri(s: &str) -> bool {
    if s.len() < 3 || !s.contains(':') {
        return false;
    }
    let scheme_end = match s.find(':') {
        Some(idx) => idx,
        None => return false,
    };
    scheme_end > 0
        && s[..scheme_end]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
}

fn is_valid_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let b = s.as_bytes();
    b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && b[..8].iter().all(u8::is_ascii_hexdigit)
        && b[9..13].iter().all(u8::is_ascii_hexdigit)
        && b[14..18].iter().all(u8::is_ascii_hexdigit)
        && b[19..23].iter().all(u8::is_ascii_hexdigit)
        && b[24..].iter().all(u8::is_ascii_hexdigit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validate_required_and_unknown_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string"},
                "b": {"type": "integer"}
            },
            "required": ["a"]
        });
        let s_obj = schema.as_object().unwrap();

        // Missing required
        let args_missing = json!({"b": 10});
        assert!(validate_arguments("test_tool", args_missing.as_object().unwrap(), s_obj).is_err());

        // Unknown extra field
        let args_unknown = json!({"a": "ok", "extra": true});
        assert!(validate_arguments("test_tool", args_unknown.as_object().unwrap(), s_obj).is_err());

        // Valid
        let args_valid = json!({"a": "ok", "b": 10});
        assert!(validate_arguments("test_tool", args_valid.as_object().unwrap(), s_obj).is_ok());
    }

    #[test]
    fn test_strict_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "num": {"type": "integer"},
                "flag": {"type": "boolean"},
                "val": {"type": "number"}
            }
        });
        let s_obj = schema.as_object().unwrap();

        // Integer rejects float
        let args_float = json!({"num": 3.5});
        assert!(validate_arguments("test_tool", args_float.as_object().unwrap(), s_obj).is_err());

        // Boolean rejects string
        let args_bool_str = json!({"flag": "true"});
        assert!(
            validate_arguments("test_tool", args_bool_str.as_object().unwrap(), s_obj).is_err()
        );

        // Number rejects string
        let args_num_str = json!({"val": "100"});
        assert!(validate_arguments("test_tool", args_num_str.as_object().unwrap(), s_obj).is_err());
    }

    #[test]
    fn test_nested_object_validation() {
        let schema = json!({
            "type": "object",
            "properties": {
                "meta": {
                    "type": "object",
                    "properties": {
                        "tag": {"type": "string"}
                    },
                    "required": ["tag"]
                }
            }
        });
        let s_obj = schema.as_object().unwrap();

        // Nested missing required
        let bad_nested = json!({"meta": {}});
        assert!(validate_arguments("test_tool", bad_nested.as_object().unwrap(), s_obj).is_err());

        // Nested unknown field
        let extra_nested = json!({"meta": {"tag": "t", "other": 1}});
        assert!(validate_arguments("test_tool", extra_nested.as_object().unwrap(), s_obj).is_err());

        // Valid nested
        let good_nested = json!({"meta": {"tag": "t"}});
        assert!(validate_arguments("test_tool", good_nested.as_object().unwrap(), s_obj).is_ok());
    }
}
