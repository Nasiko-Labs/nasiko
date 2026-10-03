use crate::{CompactError, Result};
use serde_json::Value;

fn unsupported(key: &str) -> CompactError {
    CompactError::UnsupportedSchema(key.into())
}
const KEYS: &[&str] = &[
    "type",
    "properties",
    "required",
    "items",
    "enum",
    "description",
    "format",
    "additionalProperties",
    "title",
    "default",
    "examples",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
];
const TYPES: &[&str] = &[
    "object", "array", "string", "number", "integer", "boolean", "null",
];

pub(crate) fn check(schema: &Value, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(CompactError::ResourceLimit);
    }
    let obj = schema
        .as_object()
        .ok_or_else(|| unsupported("non-object schema"))?;
    if obj.keys().any(|k| !KEYS.contains(&k.as_str())) {
        return Err(unsupported("unknown schema keyword"));
    }
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .filter(|t| TYPES.contains(t))
        .ok_or_else(|| unsupported("type"))?;
    for (key, value) in obj {
        match key.as_str() {
            "type" | "default" | "examples" => (),
            "description" | "title" => {
                if !value.is_string() {
                    return Err(unsupported(key));
                }
            }
            "format" => {
                if kind != "string" || !matches!(value.as_str(), Some("date-time")) {
                    return Err(unsupported(key));
                }
            }
            "properties" => {
                if kind != "object" {
                    return Err(unsupported(key));
                }
                for child in value.as_object().ok_or_else(|| unsupported(key))?.values() {
                    check(child, depth + 1)?;
                }
            }
            "required" => {
                if kind != "object" {
                    return Err(unsupported(key));
                }
                let mut seen = std::collections::BTreeSet::new();
                for field in value.as_array().ok_or_else(|| unsupported(key))? {
                    let field = field.as_str().ok_or_else(|| unsupported(key))?;
                    if !seen.insert(field) {
                        return Err(unsupported("duplicate required"));
                    }
                }
            }
            "items" => {
                if kind != "array" {
                    return Err(unsupported(key));
                }
                check(value, depth + 1)?;
            }
            "additionalProperties" => {
                if kind != "object" {
                    return Err(unsupported(key));
                }
                if !value.is_boolean() {
                    check(value, depth + 1)?;
                }
            }
            "enum" => {
                let vals = value
                    .as_array()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| unsupported(key))?;
                for (index, item) in vals.iter().enumerate() {
                    if vals[..index].contains(item) {
                        return Err(unsupported("duplicate enum"));
                    }
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                if !matches!(kind, "integer" | "number")
                    || !value.is_number()
                    || value
                        .as_f64()
                        .is_none_or(|v| v.abs() > 9_007_199_254_740_991.0)
                {
                    return Err(unsupported(key));
                }
            }
            "minLength" | "maxLength" => {
                if kind != "string" || value.as_u64().is_none() {
                    return Err(unsupported(key));
                }
            }
            "minItems" | "maxItems" => {
                if kind != "array" || value.as_u64().is_none() {
                    return Err(unsupported(key));
                }
            }
            "minProperties" | "maxProperties" => {
                if kind != "object" || value.as_u64().is_none() {
                    return Err(unsupported(key));
                }
            }
            "uniqueItems" => {
                if kind != "array" || !value.is_boolean() {
                    return Err(unsupported(key));
                }
            }
            _ => return Err(unsupported(key)),
        }
    }
    Ok(())
}

fn fail<T>() -> Result<T> {
    Err(CompactError::InvalidArguments)
}
fn bounds(schema: &Value, value: usize, min: &str, max: &str) -> Result<()> {
    if schema
        .get(min)
        .and_then(Value::as_u64)
        .is_some_and(|n| (value as u64) < n)
        || schema
            .get(max)
            .and_then(Value::as_u64)
            .is_some_and(|n| value as u64 > n)
    {
        return fail();
    }
    Ok(())
}

pub(crate) fn validate(schema: &Value, value: &Value, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(CompactError::ResourceLimit);
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.contains(value))
    {
        return fail();
    }
    match schema["type"].as_str() {
        Some("object") => {
            let Some(obj) = value.as_object() else {
                return fail();
            };
            bounds(schema, obj.len(), "minProperties", "maxProperties")?;
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for field in required {
                    if !field.as_str().is_some_and(|f| obj.contains_key(f)) {
                        return fail();
                    }
                }
            }
            let props = schema.get("properties").and_then(Value::as_object);
            for (key, value) in obj {
                if let Some(child) = props.and_then(|p| p.get(key)) {
                    validate(child, value, depth + 1)?;
                } else {
                    match schema.get("additionalProperties") {
                        Some(Value::Bool(false)) => return fail(),
                        Some(child) if child.is_object() => validate(child, value, depth + 1)?,
                        _ => (),
                    }
                }
            }
        }
        Some("array") => {
            let Some(items) = value.as_array() else {
                return fail();
            };
            bounds(schema, items.len(), "minItems", "maxItems")?;
            for (index, item) in items.iter().enumerate() {
                if schema["uniqueItems"] == true && items[..index].contains(item) {
                    return fail();
                }
                if let Some(child) = schema.get("items") {
                    validate(child, item, depth + 1)?;
                }
            }
        }
        Some("string") => {
            let Some(text) = value.as_str() else {
                return fail();
            };
            bounds(schema, text.chars().count(), "minLength", "maxLength")?;
            match schema.get("format").and_then(Value::as_str) {
                Some("date-time") if chrono::DateTime::parse_from_rfc3339(text).is_err() => {
                    return fail();
                }
                _ => (),
            }
        }
        Some("number" | "integer") => {
            let Some(number) = value.as_f64() else {
                return fail();
            };
            if number.abs() > 9_007_199_254_740_991.0
                && ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"]
                    .iter()
                    .any(|key| schema.get(key).is_some())
            {
                return fail();
            }
            if schema["type"] == "integer" && number.fract() != 0.0 {
                return fail();
            }
            for (key, exclusive, lower) in [
                ("minimum", false, true),
                ("maximum", false, false),
                ("exclusiveMinimum", true, true),
                ("exclusiveMaximum", true, false),
            ] {
                if let Some(bound) = schema.get(key).and_then(Value::as_f64)
                    && ((lower && (number < bound || (exclusive && number == bound)))
                        || (!lower && (number > bound || (exclusive && number == bound))))
                {
                    return fail();
                }
            }
        }
        Some("boolean") if value.is_boolean() => (),
        Some("null") if value.is_null() => (),
        _ => return fail(),
    }
    Ok(())
}
