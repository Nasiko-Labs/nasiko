//! Validation of a decoded call against the tool's ORIGINAL JSON Schema.
//!
//! Enforced: `type` (string, integer, number, boolean, array, object, or a list of those),
//! `properties`, `required`, `enum`, `items`, `format: date-time`, and `additionalProperties:
//! false`. Other keywords (`minimum`, `pattern`, ...) are not enforced here; the encoder bypasses
//! tools that use them, so they never reach the compact path. Integers must be written as
//! integers (`30`, not `30.0`).

use serde_json::{Map, Value};

use crate::types::{DecodeError, ToolDef};

/// Check that `name` is a known tool and `args` satisfies its schema.
pub fn validate_call(name: &str, args: &Value, tools: &[ToolDef]) -> Result<(), DecodeError> {
    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| DecodeError::UnknownTool(name.to_string()))?;
    if !args.is_object() {
        return Err(invalid("$", "arguments must be a JSON object"));
    }
    match &tool.parameters {
        Some(schema) => check(args, schema, "$"),
        None => Ok(()),
    }
}

fn invalid(path: &str, msg: &str) -> DecodeError {
    DecodeError::InvalidArguments(format!("{path}: {msg}"))
}

fn check(value: &Value, schema: &Value, path: &str) -> Result<(), DecodeError> {
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    if let Some(ty) = schema.get("type") {
        check_type(value, ty, path)?;
    }
    if let Some(Value::Array(allowed)) = schema.get("enum")
        && !allowed.contains(value)
    {
        return Err(invalid(path, "value is not one of the allowed enum values"));
    }
    if schema.get("format").and_then(Value::as_str) == Some("date-time")
        && let Value::String(text) = value
        && !is_rfc3339(text)
    {
        return Err(invalid(path, "not an RFC 3339 date-time"));
    }
    match value {
        Value::Object(map) => check_object(map, schema, path),
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    check(item, item_schema, &format!("{path}[{i}]"))?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn check_object(
    map: &Map<String, Value>,
    schema: &Map<String, Value>,
    path: &str,
) -> Result<(), DecodeError> {
    if let Some(Value::Array(required)) = schema.get("required") {
        for name in required.iter().filter_map(Value::as_str) {
            if !map.contains_key(name) {
                return Err(invalid(
                    path,
                    &format!("missing required property {name:?}"),
                ));
            }
        }
    }
    let props = schema.get("properties").and_then(Value::as_object);
    for (key, child) in map {
        match props.and_then(|p| p.get(key)) {
            Some(child_schema) => check(child, child_schema, &format!("{path}.{key}"))?,
            None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                return Err(invalid(path, &format!("unexpected property {key:?}")));
            }
            None => {}
        }
    }
    Ok(())
}

fn check_type(value: &Value, ty: &Value, path: &str) -> Result<(), DecodeError> {
    let matches = |name: &str| match name {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => false,
    };
    let ok = match ty {
        Value::String(name) => matches(name),
        Value::Array(names) => names.iter().filter_map(Value::as_str).any(matches),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(invalid(path, &format!("expected type {ty}")))
    }
}

/// RFC 3339 date-time shape and ranges, without a date library.
fn is_rfc3339(text: &str) -> bool {
    let b = text.as_bytes();
    if !text.is_ascii() || b.len() < 20 {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        let mut n = 0u32;
        for &c in &b[r] {
            if !c.is_ascii_digit() {
                return None;
            }
            n = n * 10 + u32::from(c - b'0');
        }
        Some(n)
    };
    let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    let (Some(hour), Some(min), Some(sec)) = (digits(11..13), digits(14..16), digits(17..19))
    else {
        return false;
    };
    if b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if day == 0 || day > days || hour > 23 || min > 59 || sec > 60 {
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
    match b.get(i) {
        Some(b'Z' | b'z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            b.len() == i + 6
                && b[i + 3] == b':'
                && matches!(digits(i + 1..i + 3), Some(h) if h <= 23)
                && matches!(digits(i + 4..i + 6), Some(m) if m <= 59)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn datetime_shapes() {
        for ok in [
            "2026-10-05T15:00:00+05:30",
            "2026-10-05T15:00:00Z",
            "2026-10-05t15:00:00.123z",
            "2024-02-29T00:00:00-08:00",
        ] {
            assert!(is_rfc3339(ok), "{ok}");
        }
        for bad in [
            "",
            "tomorrow",
            "2026-10-05",
            "2026-10-05 15:00:00Z",
            "2026-13-05T15:00:00Z",
            "2026-02-29T15:00:00Z",
            "2026-10-05T24:00:00Z",
            "2026-10-05T15:60:00Z",
            "2026-10-05T15:00:00",
            "2026-10-05T15:00:00+0530",
            "2026-10-05T15:00:00.Z",
            "2026-10-05T15:00:00é",
        ] {
            assert!(!is_rfc3339(bad), "{bad}");
        }
    }

    #[test]
    fn unknown_tool_and_non_object_arguments() {
        let tools = vec![ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(json!({"type": "object"})),
        }];
        assert_eq!(
            validate_call("x", &json!({}), &tools).unwrap_err().code(),
            "unknown_tool"
        );
        assert_eq!(
            validate_call("t", &json!([1]), &tools).unwrap_err().code(),
            "invalid_arguments"
        );
        assert!(validate_call("t", &json!({}), &tools).is_ok());
    }

    #[test]
    fn type_lists_and_enums() {
        let tools = vec![ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "a": {"type": ["string", "null"]},
                    "b": {"enum": [1, "x"]}
                }
            })),
        }];
        assert!(validate_call("t", &json!({"a": null, "b": 1}), &tools).is_ok());
        assert!(validate_call("t", &json!({"a": 3}), &tools).is_err());
        assert!(validate_call("t", &json!({"b": 2}), &tools).is_err());
    }
}
