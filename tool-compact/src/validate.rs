//! Validates decoded arguments against the original JSON Schema.
//!
//! Covers exactly the subset `encode` accepts (types, enum, nested objects/arrays, required).
//! Strict by design: unknown keys, `null`s and non-integer numbers for `integer` are errors.

use serde_json::Value;

use crate::{Error, Result};

pub(crate) fn validate_args(tool: &str, parameters: &Option<Value>, args: &Value) -> Result<()> {
    let outcome = match parameters {
        Some(schema) => check(schema, args, ""),
        None if args.as_object().is_some_and(|o| o.is_empty()) => Ok(()),
        None => Err("tool takes no arguments".to_string()),
    };
    outcome.map_err(|reason| Error::InvalidArguments {
        tool: tool.to_string(),
        reason,
    })
}

fn at(path: &str) -> &str {
    if path.is_empty() { "arguments" } else { path }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn check(schema: &Value, v: &Value, path: &str) -> std::result::Result<(), String> {
    if let Some(Value::Array(allowed)) = schema.get("enum")
        && !allowed.contains(v)
    {
        return Err(format!("`{}` is not one of the allowed values", at(path)));
    }
    let wrong = |want: &str| Err(format!("`{}` must be {want}", at(path)));
    match schema.get("type").and_then(Value::as_str) {
        Some("string") if !v.is_string() => wrong("a string"),
        Some("integer") if !(v.is_i64() || v.is_u64()) => wrong("an integer"),
        Some("number") if !v.is_number() => wrong("a number"),
        Some("boolean") if !v.is_boolean() => wrong("a boolean"),
        Some("array") => {
            let items = v
                .as_array()
                .ok_or_else(|| format!("`{}` must be an array", at(path)))?;
            let item_schema = &schema["items"];
            items
                .iter()
                .enumerate()
                .try_for_each(|(i, item)| check(item_schema, item, &format!("{path}[{i}]")))
        }
        Some("object") => check_object(schema, v, path),
        _ => Ok(()),
    }
}

fn check_object(schema: &Value, v: &Value, path: &str) -> std::result::Result<(), String> {
    let obj = v
        .as_object()
        .ok_or_else(|| format!("`{}` must be an object", at(path)))?;
    let props = schema.get("properties").and_then(Value::as_object);
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !obj.contains_key(name) {
            return Err(format!("missing required `{}`", join(path, name)));
        }
    }
    for (key, val) in obj {
        match props.and_then(|p| p.get(key)) {
            Some(prop_schema) => check(prop_schema, val, &join(path, key))?,
            None => return Err(format!("unknown argument `{}`", join(path, key))),
        }
    }
    Ok(())
}
