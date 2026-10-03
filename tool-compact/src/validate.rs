//! Argument validation against the schema tree.
//!
//! Strict by design: the decoder's job is to say "this is exactly a valid call" or "this is
//! not", never to repair. In particular:
//!
//! * every undeclared key is rejected, whether or not the schema says
//!   `additionalProperties: false` (schemas allowing extra keys bypass compaction instead);
//! * `default` is never filled in and no value is coerced (`"5"` is not an `int`);
//! * `format` is not validated — JSON Schema treats it as an annotation, and so do we.

use std::collections::HashMap;

use regex::Regex;
use serde_json::Value;

use crate::schema::{Base, Kind, Node};

pub(crate) type Patterns = HashMap<String, Regex>;

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "num",
        Value::String(_) => "str",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn matches_base(base: Base, v: &Value) -> bool {
    match (base, v) {
        (Base::Str, Value::String(_)) => true,
        (Base::Bool, Value::Bool(_)) => true,
        (Base::Null, Value::Null) => true,
        (Base::Num, Value::Number(_)) => true,
        // JSON Schema: an integer is a number with zero fractional part, so `30.0` passes. The
        // value is not rewritten — the caller receives exactly what the model wrote.
        (Base::Int, Value::Number(n)) => {
            n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => false,
    }
}

/// Validate `v` against `node`. The error names the JSON path of the first violation.
pub(crate) fn validate(
    node: &Node,
    v: &Value,
    path: &str,
    patterns: &Patterns,
) -> Result<(), String> {
    match &node.kind {
        Kind::Any => {}
        Kind::Types(bases) => {
            if !bases.iter().any(|b| matches_base(*b, v)) {
                let want: Vec<&str> = bases.iter().map(|b| b.keyword()).collect();
                return Err(format!(
                    "{path}: expected {}, got {}",
                    want.join("|"),
                    kind_name(v)
                ));
            }
        }
        Kind::Enum(values) => {
            if !values.contains(v) {
                return Err(format!("{path}: {v} is not one of the allowed values"));
            }
        }
        Kind::Array(item) => {
            let Value::Array(elems) = v else {
                return Err(format!("{path}: expected array, got {}", kind_name(v)));
            };
            let n = elems.len() as u64;
            if let Some(min) = node.ann.min_items
                && n < min
            {
                return Err(format!("{path}: {n} items, minimum {min}"));
            }
            if let Some(max) = node.ann.max_items
                && n > max
            {
                return Err(format!("{path}: {n} items, maximum {max}"));
            }
            if let Some(item) = item {
                for (i, e) in elems.iter().enumerate() {
                    validate(item, e, &format!("{path}[{i}]"), patterns)?;
                }
            }
        }
        Kind::Object(obj) => {
            let Value::Object(map) = v else {
                return Err(format!("{path}: expected object, got {}", kind_name(v)));
            };
            for p in &obj.props {
                if p.required && !map.contains_key(&p.name) {
                    return Err(format!("{path}: missing required field '{}'", p.name));
                }
            }
            for (key, val) in map {
                let Some(p) = obj.props.iter().find(|p| &p.name == key) else {
                    return Err(format!("{path}: unknown field '{key}'"));
                };
                validate(&p.node, val, &format!("{path}.{key}"), patterns)?;
            }
        }
    }
    check_ann(node, v, path, patterns)
}

/// Scalar keywords. Each applies only to values of its own kind, as in JSON Schema.
fn check_ann(node: &Node, v: &Value, path: &str, patterns: &Patterns) -> Result<(), String> {
    let a = &node.ann;
    if let Value::Number(n) = v
        && let Some(x) = n.as_f64()
    {
        if let Some(min) = a.minimum.as_ref().and_then(|m| m.as_f64())
            && x < min
        {
            return Err(format!("{path}: {n} is below the minimum {min}"));
        }
        if let Some(max) = a.maximum.as_ref().and_then(|m| m.as_f64())
            && x > max
        {
            return Err(format!("{path}: {n} is above the maximum {max}"));
        }
    }
    if let Value::String(s) = v {
        let len = s.chars().count() as u64;
        if let Some(min) = a.min_length
            && len < min
        {
            return Err(format!("{path}: length {len}, minimum {min}"));
        }
        if let Some(max) = a.max_length
            && len > max
        {
            return Err(format!("{path}: length {len}, maximum {max}"));
        }
        if let Some(p) = &a.pattern {
            // Compiled up front by the decoder; a missing entry is a bug, and fails closed.
            let ok = patterns.get(p).is_some_and(|re| re.is_match(s));
            if !ok {
                return Err(format!("{path}: does not match pattern '{p}'"));
            }
        }
    }
    Ok(())
}
