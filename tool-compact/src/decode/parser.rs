use crate::{CompactError, Result};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

/// Serde's default Value visitor overwrites duplicate keys. Calls must reject them.
pub(super) fn parse_object(text: &str) -> Result<Value> {
    let StrictValue(value) =
        serde_json::from_str::<StrictValue>(text).map_err(|_| CompactError::MalformedCall)?;
    if !value.is_object() {
        return Err(CompactError::MalformedCall);
    }
    verify_numbers(text)?;
    Ok(value)
}

fn verify_numbers(text: &str) -> Result<()> {
    // Reject decimals that serde_json would round into a different value. In
    // particular a fractional large number must not become a valid integer.
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while let Some(&byte) = bytes.get(index) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
        } else if byte == b'"' {
            in_string = true;
            index += 1;
        } else if byte == b'-' || byte.is_ascii_digit() {
            let start = index;
            while bytes.get(index).is_some_and(|byte| {
                byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E')
            }) {
                index += 1;
            }
            let original = text.get(start..index).ok_or(CompactError::MalformedCall)?;
            let number: Number =
                serde_json::from_str(original).map_err(|_| CompactError::MalformedCall)?;
            let serialized = number.to_string();
            if decimal(original).is_none() || decimal(original) != decimal(&serialized) {
                return Err(CompactError::MalformedCall);
            }
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn decimal(text: &str) -> Option<(bool, String, i64)> {
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (unsigned, 0),
    };
    let fraction = mantissa
        .split_once('.')
        .map(|(_, fraction)| fraction.len())
        .unwrap_or(0);
    let digits: String = mantissa
        .chars()
        .filter(|&character| character != '.')
        .collect();
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some((false, "0".into(), 0));
    }
    let trimmed = significant.trim_end_matches('0');
    let exponent = exponent
        .checked_sub(i64::try_from(fraction).ok()?)?
        .checked_add(i64::try_from(significant.len() - trimmed.len()).ok()?)?;
    Some((negative, trimmed.into(), exponent))
}

struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;
impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("JSON without duplicate keys")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
        Number::from_f64(value)
            .map(|number| StrictValue(Value::Number(number)))
            .ok_or_else(|| E::custom("nonfinite number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.into())))
    }
    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(StrictValue(value)) = sequence.next_element()? {
            values.push(value);
        }
        Ok(StrictValue(Value::Array(values)))
    }
    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom("duplicate object key"));
            }
            let StrictValue(value) = map.next_value()?;
            object.insert(key, value);
        }
        Ok(StrictValue(Value::Object(object)))
    }
}
