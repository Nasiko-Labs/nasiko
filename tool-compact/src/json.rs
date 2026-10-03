//! JSON parsing that refuses duplicate object keys.
//!
//! `serde_json` silently keeps the last of two equal keys. For tool arguments that would mean
//! returning a call the model did not unambiguously write, so a duplicate is an error here.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::error::ArgumentFault;

const DUPLICATE: &str = "duplicate key";

/// Parse exactly one JSON value from `text`, rejecting duplicate keys at any depth.
pub(crate) fn parse_strict(text: &str) -> Result<Value, ArgumentFault> {
    let mut de = serde_json::Deserializer::from_str(text);
    let value = StrictValue::deserialize(&mut de).map_err(classify)?;
    de.end().map_err(classify)?;
    Ok(value.0)
}

fn classify(error: serde_json::Error) -> ArgumentFault {
    let message = error.to_string();
    if message.starts_with(DUPLICATE) {
        ArgumentFault::DuplicateKey
    } else {
        ArgumentFault::Json(message)
    }
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Bool(v)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::from(v)))
    }

    fn visit_u64<E>(self, v: u64) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::from(v)))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<StrictValue, E> {
        Number::from_f64(v)
            .map(|n| StrictValue(Value::Number(n)))
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E>(self, v: &str) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::String(v.to_owned())))
    }

    fn visit_string<E>(self, v: String) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::String(v)))
    }

    fn visit_unit<E>(self) -> Result<StrictValue, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StrictValue, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictValue(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(StrictValue(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<StrictValue, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.contains_key(&key) {
                return Err(de::Error::custom(format!("{DUPLICATE} `{key}`")));
            }
            let StrictValue(value) = map.next_value()?;
            out.insert(key, value);
        }
        Ok(StrictValue(Value::Object(out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_like_serde_json() {
        let text = r#"{"a":[1,2.5,-3],"b":{"c":null,"d":"x\"y"},"e":true,"f":30.0}"#;
        assert_eq!(
            parse_strict(text).unwrap(),
            serde_json::from_str::<Value>(text).unwrap()
        );
        assert_eq!(
            parse_strict(r#"{"f":30.0}"#).unwrap().to_string(),
            r#"{"f":30.0}"#,
            "a fractional-looking integer is passed through unchanged"
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_at_any_depth() {
        assert_eq!(
            parse_strict(r#"{"a":1,"a":2}"#),
            Err(ArgumentFault::DuplicateKey)
        );
        assert_eq!(
            parse_strict(r#"{"o":{"k":1,"k":1}}"#),
            Err(ArgumentFault::DuplicateKey)
        );
    }

    #[test]
    fn malformed_json_is_a_json_fault() {
        for bad in [
            r#"{"a":1,}"#,
            "{'a':1}",
            r#"{"a":NaN}"#,
            r#"{"a":1} x"#,
            r#"{"a""#,
        ] {
            assert!(
                matches!(parse_strict(bad), Err(ArgumentFault::Json(_))),
                "{bad}"
            );
        }
        assert_eq!(parse_strict("{}").unwrap(), json!({}));
    }
}
