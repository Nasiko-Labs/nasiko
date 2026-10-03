use crate::{CompactError, MAX_DEPTH};
use serde_json::Value;

const COMMON: &[&str] = &[
    "type",
    "description",
    "title",
    "default",
    "examples",
    "enum",
    "const",
    "deprecated",
    "readOnly",
    "writeOnly",
];

fn unsupported(reason: impl Into<String>) -> CompactError {
    CompactError::UnsupportedSchema(reason.into())
}

/// Unsupported keywords are rejected, not silently erased.
pub(crate) fn check(s: &Value, depth: usize) -> Result<(), CompactError> {
    if depth > MAX_DEPTH {
        return Err(CompactError::LimitExceeded);
    }
    let m = s
        .as_object()
        .ok_or_else(|| unsupported("schema must be an object"))?;
    let kind = m
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported("explicit single type required"))?;
    let keywords: &[&str] = match kind {
        "object" => &[
            "properties",
            "required",
            "additionalProperties",
            "minProperties",
            "maxProperties",
        ],
        "array" => &["items", "minItems", "maxItems", "uniqueItems"],
        "string" => &["minLength", "maxLength", "format"],
        "integer" | "number" => &["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"],
        "boolean" | "null" => &[],
        _ => return Err(unsupported("unsupported type")),
    };
    for (key, value) in m {
        if !COMMON.contains(&key.as_str()) && !keywords.contains(&key.as_str()) {
            return Err(unsupported(format!("keyword {key}")));
        }
        match key.as_str() {
            "description" | "title" => {
                if !value.is_string() {
                    return Err(unsupported(format!("{key} must be a string")));
                }
            }
            "deprecated" | "readOnly" | "writeOnly" | "uniqueItems" => {
                if !value.is_boolean() {
                    return Err(unsupported(format!("{key} must be boolean")));
                }
            }
            "enum" => {
                let a = value
                    .as_array()
                    .filter(|a| !a.is_empty())
                    .ok_or_else(|| unsupported("nonempty enum required"))?;
                for (i, item) in a.iter().enumerate() {
                    if a[..i].iter().any(|previous| equal(previous, item)) {
                        return Err(unsupported("duplicate enum value"));
                    }
                }
            }
            "examples" => {
                if !value.is_array() {
                    return Err(unsupported("examples must be an array"));
                }
            }
            "minLength" | "maxLength" | "minItems" | "maxItems" | "minProperties"
            | "maxProperties" => {
                if value.as_u64().is_none() {
                    return Err(unsupported(format!("{key} must be a nonnegative integer")));
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                if !value.is_number() {
                    return Err(unsupported(format!("{key} must be numeric")));
                }
            }
            "format" => {
                if !matches!(value.as_str(), Some("date" | "date-time")) {
                    return Err(unsupported("only date and date-time formats supported"));
                }
            }
            _ => {}
        }
    }
    if let Some(p) = m.get("properties") {
        for v in p
            .as_object()
            .ok_or_else(|| unsupported("properties must be an object"))?
            .values()
        {
            check(v, depth + 1)?;
        }
    }
    if let Some(r) = m.get("required") {
        let required = r
            .as_array()
            .ok_or_else(|| unsupported("required must be an array"))?;
        let mut seen = std::collections::HashSet::new();
        for key in required {
            let key = key
                .as_str()
                .ok_or_else(|| unsupported("required entries must be strings"))?;
            if !seen.insert(key)
                || !m
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|p| p.contains_key(key))
            {
                return Err(unsupported(
                    "required must reference distinct declared properties",
                ));
            }
        }
    }
    if let Some(items) = m.get("items") {
        check(items, depth + 1)?;
    }
    if let Some(a) = m.get("additionalProperties")
        && !a.is_boolean()
    {
        check(a, depth + 1)?;
    }
    Ok(())
}

fn integer(v: &Value) -> bool {
    v.as_i64().is_some() || v.as_u64().is_some() || v.as_f64().is_some_and(|n| n.fract() == 0.0)
}

/// Exact integer ordering where possible; do not round u64/i64 into f64.
fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    let int = |v: &Value| {
        v.as_i64()
            .map(i128::from)
            .or_else(|| v.as_u64().map(i128::from))
    };
    match (int(a), int(b)) {
        (Some(a), Some(b)) => Some(a.cmp(&b)),
        _ => {
            let (a, b) = (a.as_f64()?, b.as_f64()?);
            // Refuse ambiguous comparisons outside f64's exact integer range.
            if a.abs() > 9_007_199_254_740_991.0 || b.abs() > 9_007_199_254_740_991.0 {
                None
            } else {
                a.partial_cmp(&b)
            }
        }
    }
}

// JSON Schema numeric equality treats 1 and 1.0 identically, including inside arrays/objects.
fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => a == b || compare(a, b).is_some_and(|o| o.is_eq()),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| equal(a, b)))
        }
        _ => a == b,
    }
}

pub(crate) fn validate(s: &Value, value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("nesting limit".into());
    }
    let kind = s["type"].as_str().ok_or("missing type")?;
    let correct = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_number() && integer(value),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if !correct {
        return Err(format!("expected {kind}"));
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.iter().any(|v| equal(v, value))
    {
        return Err("enum violation".into());
    }
    if let Some(c) = s.get("const")
        && !equal(c, value)
    {
        return Err("const violation".into());
    }
    let size = match value {
        Value::String(v) => Some((v.chars().count(), "minLength", "maxLength")),
        Value::Array(v) => Some((v.len(), "minItems", "maxItems")),
        Value::Object(v) => Some((v.len(), "minProperties", "maxProperties")),
        _ => None,
    };
    if let Some((size, min, max)) = size
        && (s
            .get(min)
            .and_then(Value::as_u64)
            .is_some_and(|m| (size as u64) < m)
            || s.get(max)
                .and_then(Value::as_u64)
                .is_some_and(|m| (size as u64) > m))
    {
        return Err("size constraint violation".into());
    }
    for (key, exclusive, lower) in [
        ("minimum", false, true),
        ("maximum", false, false),
        ("exclusiveMinimum", true, true),
        ("exclusiveMaximum", true, false),
    ] {
        if let Some(bound) = s.get(key) {
            let ordering = compare(value, bound).ok_or("ambiguous numeric comparison")?;
            let allowed = if lower {
                ordering.is_gt() || (!exclusive && ordering.is_eq())
            } else {
                ordering.is_lt() || (!exclusive && ordering.is_eq())
            };
            if !allowed {
                return Err("numeric bound violation".into());
            }
        }
    }
    if let Some(text) = value.as_str() {
        let valid = match s.get("format").and_then(Value::as_str) {
            Some("date-time") => chrono::DateTime::parse_from_rfc3339(text).is_ok(),
            Some("date") => chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .is_ok_and(|d| d.format("%Y-%m-%d").to_string() == text),
            None => true,
            _ => false,
        };
        if !valid {
            return Err("format violation".into());
        }
    }
    if let Some(obj) = value.as_object() {
        if let Some(required) = s.get("required").and_then(Value::as_array)
            && required
                .iter()
                .any(|key| !obj.contains_key(key.as_str().unwrap_or("")))
        {
            return Err("missing required property".into());
        }
        for (key, item) in obj {
            if let Some(sub) = s.get("properties").and_then(|p| p.get(key)) {
                validate(sub, item, depth + 1)?;
            } else if let Some(additional) = s.get("additionalProperties") {
                if additional == &Value::Bool(false) {
                    return Err("additional property".into());
                }
                if additional.is_object() {
                    validate(additional, item, depth + 1)?;
                }
            }
        }
    }
    if let Some(array) = value.as_array() {
        if array.len() > crate::MAX_ITEMS {
            return Err("array item limit".into());
        }
        if s.get("uniqueItems") == Some(&Value::Bool(true)) {
            for (i, item) in array.iter().enumerate() {
                if array[..i].iter().any(|previous| equal(previous, item)) {
                    return Err("duplicate array item".into());
                }
            }
        }
        if let Some(items) = s.get("items") {
            for item in array {
                validate(items, item, depth + 1)?;
            }
        }
    }
    Ok(())
}
