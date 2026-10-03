//! Duplicate-key detection. `serde_json` keeps the last of two equal keys without a word, which
//! would turn `{"to":"a","to":"b"}` into a silently altered call; this walks the same text and
//! reports any object that repeats a key, at any depth.

use std::collections::HashSet;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// `true` if `raw` (already known to be valid JSON) has an object with a repeated key.
pub(crate) fn has_duplicate_keys(raw: &str) -> bool {
    serde_json::from_str::<NoDuplicates>(raw).is_err()
}

/// Deserializes any JSON value, failing on a repeated object key.
struct NoDuplicates;

impl<'de> Deserialize<'de> for NoDuplicates {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NoDuplicatesVisitor)
    }
}

struct NoDuplicatesVisitor;

impl<'de> Visitor<'de> for NoDuplicatesVisitor {
    type Value = NoDuplicates;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_i64<E>(self, _: i64) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_u64<E>(self, _: u64) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_f64<E>(self, _: f64) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_str<E>(self, _: &str) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }
    fn visit_unit<E>(self) -> Result<NoDuplicates, E> {
        Ok(NoDuplicates)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<NoDuplicates, A::Error> {
        while seq.next_element::<NoDuplicates>()?.is_some() {}
        Ok(NoDuplicates)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<NoDuplicates, A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key) {
                return Err(de::Error::custom("duplicate key"));
            }
            map.next_value::<NoDuplicates>()?;
        }
        Ok(NoDuplicates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_repeated_keys_at_any_depth() {
        assert!(has_duplicate_keys(r#"{"a":1,"a":2}"#));
        assert!(has_duplicate_keys(r#"{"a":{"b":1,"b":1}}"#));
        assert!(has_duplicate_keys(r#"{"a":[{"x":1},{"y":1,"y":2}]}"#));
        // Equal after unescaping is equal.
        assert!(has_duplicate_keys(r#"{"ab":1,"a\u0062":2}"#));
    }

    #[test]
    fn accepts_equal_keys_in_different_objects() {
        assert!(!has_duplicate_keys(
            r#"{"a":{"k":1},"b":{"k":1},"k":[{"k":2}]}"#
        ));
        assert!(!has_duplicate_keys(r#"{"ab":1,"ab ":2}"#));
    }
}
