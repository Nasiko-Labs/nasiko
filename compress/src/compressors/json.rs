//! Keeps every key and the document's shape; collapses repetitive arrays and long string values.
//!
//! Collapsing is irreversible unless the original is stored, so it happens only when the caller
//! passes a recovery handle ([`Policy::recovery_ref`]). Without one the document is rewritten
//! without removing anything — an agent that cannot fetch the 96 search results it was not shown
//! cannot answer a question about them either: whitespace goes, and a list of same-shaped
//! records becomes a table that names each key once ([`crate::table`], reversible with
//! [`crate::expand_tables`]).
//!
//! Subtrees under an error-ish key survive whole — when a tool result is 90% success payload and
//! 10% failure detail, the failure detail is the part the model is being asked about.

use serde_json::{Map, Value};

use crate::marker;
use crate::policy::Policy;
use crate::table;
use crate::text;

/// Keys whose values are never reduced, at any depth.
const PRESERVED_KEYS: [&str; 7] = [
    "error",
    "errors",
    "message",
    "stack",
    "stacktrace",
    "detail",
    "exception",
];

pub(super) fn compress(input: &str, policy: &Policy<'_>) -> Option<String> {
    let parsed: Value = serde_json::from_str(input).ok()?;
    let reduced = reduce(&parsed, policy);
    serde_json::to_string(&reduced).ok()
}

/// No recovery handle means no stored original: only rewrites that remove nothing are allowed.
fn is_lossless(policy: &Policy<'_>) -> bool {
    policy.recovery_ref.is_none()
}

fn reduce(value: &Value, policy: &Policy<'_>) -> Value {
    match value {
        Value::Object(map) => Value::Object(reduce_object(map, policy)),
        Value::Array(items) => reduce_array(items, policy),
        Value::String(s) => reduce_string(s, policy),
        other => other.clone(),
    }
}

fn reduce_object(map: &Map<String, Value>, policy: &Policy<'_>) -> Map<String, Value> {
    let mut out = Map::with_capacity(map.len());
    for (key, value) in map {
        if is_preserved(key) {
            out.insert(key.clone(), value.clone());
        } else {
            out.insert(key.clone(), reduce(value, policy));
        }
    }
    out
}

fn reduce_array(items: &[Value], policy: &Policy<'_>) -> Value {
    let (head, tail) = policy.level.json_array_head_tail();

    if is_lossless(policy) {
        return tabulate_if_smaller(items, policy);
    }

    // The marker costs an element; collapsing must remove more than one to be worth it.
    if items.len() <= head + tail + 1 || !is_homogeneous(items) {
        return Value::Array(items.iter().map(|v| reduce(v, policy)).collect());
    }

    let elided = items.len() - head - tail;
    let mut out: Vec<Value> = Vec::with_capacity(head + tail + 1);
    out.extend(items.iter().take(head).map(|v| reduce(v, policy)));
    out.push(Value::String(marker::elided_items(
        elided,
        items.len(),
        policy.recovery_ref,
    )));
    out.extend(
        items
            .iter()
            .skip(items.len() - tail)
            .map(|v| reduce(v, policy)),
    );
    Value::Array(out)
}

/// Every item kept: the table form when it is shorter once serialized, else the plain list.
fn tabulate_if_smaller(items: &[Value], policy: &Policy<'_>) -> Value {
    let plain = Value::Array(items.iter().map(|v| reduce(v, policy)).collect());
    match table::tabulate(items).map(Value::String) {
        Some(tabled) if serialized_len(&tabled) < serialized_len(&plain) => tabled,
        _ => plain,
    }
}

fn serialized_len(v: &Value) -> usize {
    serde_json::to_string(v).map_or(usize::MAX, |s| s.len())
}

fn reduce_string(s: &str, policy: &Policy<'_>) -> Value {
    if is_lossless(policy) {
        return Value::String(s.to_string());
    }
    let max = policy.level.max_string_chars();
    match text::head_tail(s, max * 2 / 3, max / 3, policy.recovery_ref) {
        Some(shortened) => Value::String(shortened),
        None => Value::String(s.to_string()),
    }
}

/// Only collapse arrays whose elements are the same kind of thing. A mixed array is usually a
/// tuple or a positional structure, where dropping the middle changes the meaning.
fn is_homogeneous(items: &[Value]) -> bool {
    let Some(first) = items.first() else {
        return false;
    };
    items.iter().all(|v| discriminant(v) == discriminant(first))
}

fn discriminant(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

fn is_preserved(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    PRESERVED_KEYS.iter().any(|k| lowered == *k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_type::TypeMask;
    use crate::policy::Level;

    /// Lossy cuts are only allowed when the original is recoverable, so the collapsing tests
    /// run with a handle.
    fn policy(level: Level) -> Policy<'static> {
        Policy {
            enabled: true,
            min_bytes: 0,
            types: TypeMask::ALL,
            level,
            recovery_ref: Some("h-1"),
            ..Default::default()
        }
    }

    fn unrecoverable(level: Level) -> Policy<'static> {
        Policy {
            recovery_ref: None,
            ..policy(level)
        }
    }

    #[test]
    fn without_a_recovery_handle_no_item_or_string_is_removed() {
        let items: Vec<String> = (0..100)
            .map(|i| format!(r#"{{"repo": "org/r{i}", "stars": {i}}}"#))
            .collect();
        let long = "x".repeat(5000);
        let input = format!(r#"{{"items": [{}], "readme": "{long}"}}"#, items.join(", "));

        let out = compress(&input, &unrecoverable(Level::Aggressive)).unwrap();

        let original: Value = serde_json::from_str(&input).unwrap();
        assert_eq!(
            crate::expand_tables(&out).unwrap(),
            original,
            "every item and every character must survive"
        );
        assert!(
            !out.contains("elided"),
            "no elision without a stored original"
        );
        assert!(out.contains("table: 100 rows"), "the list is tabled: {out}");
        assert!(
            out.len() < input.len(),
            "whitespace still goes: {} vs {}",
            out.len(),
            input.len()
        );
    }

    #[test]
    fn collapses_a_long_homogeneous_array_and_counts_what_went() {
        let items: Vec<String> = (0..50).map(|i| format!(r#"{{"id":{i}}}"#)).collect();
        let input = format!(r#"{{"items":[{}]}}"#, items.join(","));

        let out = compress(&input, &policy(Level::Balanced)).unwrap();

        assert!(out.len() < input.len());
        assert!(out.contains("47 of 50 items elided"));
        assert!(out.contains(r#""id":0"#), "head element missing");
        assert!(out.contains(r#""id":49"#), "tail element missing");
    }

    #[test]
    fn preserves_error_subtrees_whole() {
        let errors: Vec<String> = (0..50).map(|i| format!(r#"{{"code":{i}}}"#)).collect();
        let input = format!(
            r#"{{"ok":[{}],"error":{{"nested":[{}]}}}}"#,
            errors.join(","),
            errors.join(",")
        );

        let out = compress(&input, &policy(Level::Aggressive)).unwrap();

        // The `ok` array collapses; the `error` subtree does not.
        assert!(out.contains("items elided"));
        assert!(out.contains(r#""code":25"#), "error subtree was reduced");
    }

    #[test]
    fn leaves_short_arrays_alone() {
        let input = r#"{"a":[1,2,3]}"#;
        let out = compress(input, &policy(Level::Aggressive)).unwrap();
        assert!(!marker::contains_marker(&out));
    }

    #[test]
    fn does_not_collapse_heterogeneous_arrays() {
        let input = r#"{"tuple":[1,"two",3,"four",5,"six",7,"eight",9,"ten"]}"#;
        let out = compress(input, &policy(Level::Aggressive)).unwrap();
        assert!(!marker::contains_marker(&out), "positional array collapsed");
    }

    #[test]
    fn shortens_long_string_values() {
        let long = "x".repeat(5000);
        let input = format!(r#"{{"blob":"{long}"}}"#);
        let out = compress(&input, &policy(Level::Aggressive)).unwrap();
        assert!(out.len() < input.len());
        assert!(out.contains("chars elided"));
    }

    #[test]
    fn malformed_json_returns_none() {
        assert!(compress(r#"{"a": 1"#, &policy(Level::Balanced)).is_none());
    }

    #[test]
    fn preserves_every_key() {
        let input = r#"{"alpha":1,"beta":{"gamma":2},"delta":[1,2,3]}"#;
        let out = compress(input, &policy(Level::Aggressive)).unwrap();
        for key in ["alpha", "beta", "gamma", "delta"] {
            assert!(out.contains(key), "lost key {key}");
        }
    }

    #[test]
    fn multibyte_string_values_do_not_panic() {
        let long = "€".repeat(5000);
        let input = format!(r#"{{"blob":"{long}"}}"#);
        assert!(compress(&input, &policy(Level::Aggressive)).is_some());
    }
}
