//! JSON helpers with stricter semantics than `serde_json`'s defaults.
//!
//! * [`parse_object_unique`] builds a `Value` while rejecting duplicate object keys (serde_json
//!   silently keeps the last one) and enforcing [`MAX_DEPTH`]. It reuses serde_json's string and
//!   number parsing so argument numbers and schema numbers have identical semantics.
//! * [`canonical_json`] serializes with recursively sorted keys so output never depends on the
//!   `preserve_order` feature, which dependents may switch on through feature unification.

use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::limits::MAX_DEPTH;

#[derive(Debug)]
pub(crate) enum JsonError {
    Syntax(String),
    DuplicateKey(String),
    TooDeep,
    NotAnObject,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(s) => write!(f, "invalid JSON: {s}"),
            Self::DuplicateKey(k) => write!(f, "duplicate key `{k}`"),
            Self::TooDeep => write!(f, "nesting deeper than {MAX_DEPTH}"),
            Self::NotAnObject => write!(f, "arguments must be a JSON object"),
        }
    }
}

struct ValueSeed {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ValueSeed {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(ValueVisitor { depth: self.depth })
    }
}

struct ValueVisitor {
    depth: usize,
}

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(de::Error::custom(JsonError::TooDeep.to_string()));
        }
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(ValueSeed {
            depth: self.depth + 1,
        })? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        if self.depth >= MAX_DEPTH {
            return Err(de::Error::custom(JsonError::TooDeep.to_string()));
        }
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(ValueSeed {
                depth: self.depth + 1,
            })?;
            if out.insert(key.clone(), value).is_some() {
                return Err(de::Error::custom(JsonError::DuplicateKey(key).to_string()));
            }
        }
        Ok(Value::Object(out))
    }
}

/// Parse `text` as exactly one JSON object with unique keys and bounded depth.
pub(crate) fn parse_object_unique(text: &str) -> Result<Map<String, Value>, JsonError> {
    let mut de = serde_json::Deserializer::from_str(text);
    let value = ValueSeed { depth: 0 }
        .deserialize(&mut de)
        .map_err(|e| classify(&e))?;
    de.end().map_err(|e| classify(&e))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(JsonError::NotAnObject),
    }
}

fn classify(e: &serde_json::Error) -> JsonError {
    let msg = e.to_string();
    if let Some(rest) = msg.strip_prefix("duplicate key `") {
        let key = rest.split('`').next().unwrap_or("").to_owned();
        return JsonError::DuplicateKey(key);
    }
    if msg.starts_with("nesting deeper than") {
        return JsonError::TooDeep;
    }
    JsonError::Syntax(msg)
}

/// Compact JSON with object keys sorted recursively.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&crate::lexeme::quote(k));
                out.push(':');
                if let Some(v) = map.get(*k) {
                    write_canonical(v, out);
                }
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        // Scalars have one serialization; `to_string` on them cannot fail.
        other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn duplicate_keys_are_rejected_at_any_depth() {
        assert!(matches!(
            parse_object_unique(r#"{"a":1,"a":2}"#),
            Err(JsonError::DuplicateKey(k)) if k == "a"
        ));
        assert!(matches!(
            parse_object_unique(r#"{"a":{"b":1,"b":2}}"#),
            Err(JsonError::DuplicateKey(k)) if k == "b"
        ));
        assert!(matches!(
            parse_object_unique(r#"{"a":[{"x":1,"x":1}]}"#),
            Err(JsonError::DuplicateKey(_))
        ));
    }

    #[test]
    fn well_formed_objects_parse_exactly_and_non_objects_are_refused() {
        let m = parse_object_unique(r#"{"b":[1,2.5,"xé"],"a":null}"#).unwrap();
        assert_eq!(Value::Object(m), json!({"a": null, "b": [1, 2.5, "xé"]}));
        assert!(matches!(
            parse_object_unique("[1]"),
            Err(JsonError::NotAnObject)
        ));
        assert!(matches!(
            parse_object_unique(r#"{"a":1} x"#),
            Err(JsonError::Syntax(_))
        ));
        assert!(matches!(
            parse_object_unique(r#"{"a":1"#),
            Err(JsonError::Syntax(_))
        ));
    }

    #[test]
    fn depth_is_bounded_before_serde_json_limits_apply() {
        let open = "[".repeat(MAX_DEPTH + 1);
        let close = "]".repeat(MAX_DEPTH + 1);
        let text = format!("{{\"a\":{open}{close}}}");
        assert!(matches!(
            parse_object_unique(&text),
            Err(JsonError::TooDeep)
        ));
        let open = "[".repeat(MAX_DEPTH - 1);
        let close = "]".repeat(MAX_DEPTH - 1);
        let text = format!("{{\"a\":{open}{close}}}");
        assert!(parse_object_unique(&text).is_ok());
    }

    #[test]
    fn canonical_json_sorts_keys_recursively_and_keeps_array_order() {
        let v = json!({"z": {"b": 1, "a": [3, {"y": 1, "x": 2}]}, "a": "s"});
        assert_eq!(
            canonical_json(&v),
            r#"{"a":"s","z":{"a":[3,{"x":2,"y":1}],"b":1}}"#
        );
    }
}
