//! The call grammar: `<<call NAME {JSON}>>`.
//!
//! The scanner is JSON-aware: `>>` only closes a call after the argument object has closed, so
//! `>>` (or `}`) inside a string argument needs no escaping.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

pub(crate) const MARKER: &str = "<<call";
/// Upper bound on one buffered call, so a never-closed call cannot grow memory without limit.
pub(crate) const MAX_CALL_BYTES: usize = 1 << 20;

#[derive(Debug, PartialEq)]
pub(crate) enum Scan<'a> {
    /// More input is needed to decide.
    Incomplete,
    /// A structurally complete call spanning `len` bytes.
    Done {
        len: usize,
        name: &'a str,
        args: &'a str,
    },
    Malformed(String),
}

/// Scan one call. `s` must start with [`MARKER`] followed by ASCII whitespace.
pub(crate) fn scan_call(s: &str) -> Scan<'_> {
    if s.len() > MAX_CALL_BYTES {
        return Scan::Malformed("call exceeds the size limit".into());
    }
    let b = s.as_bytes();
    let name_start = skip_ws(b, MARKER.len());
    let mut i = name_end(b, name_start);
    if i == b.len() {
        return Scan::Incomplete;
    }
    let name = &s[name_start..i];
    if name.is_empty() {
        return Scan::Malformed("missing tool name after `<<call`".into());
    }

    i = skip_ws(b, i);
    match b.get(i) {
        None => return Scan::Incomplete,
        Some(b'{') => {}
        Some(_) => return Scan::Malformed(format!("expected `{{` after tool name `{name}`")),
    }

    let args_start = i;
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    loop {
        let Some(&c) = b.get(i) else {
            return Scan::Incomplete;
        };
        i += 1;
        if in_str {
            match c {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    let args = &s[args_start..i];

    i = skip_ws(b, i);
    match &b[i..] {
        [] | [b'>'] => Scan::Incomplete,
        [b'>', b'>', ..] => Scan::Done {
            len: i + 2,
            name,
            args,
        },
        _ => Scan::Malformed(format!("expected `>>` after the arguments of `{name}`")),
    }
}

/// The call's tool name once it has fully arrived (something other than a name byte follows),
/// so an unknown tool is reported as such even if the rest of the call is broken or cut off.
pub(crate) fn call_name(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let start = skip_ws(b, MARKER.len());
    let end = name_end(b, start);
    (end > start && end < b.len()).then(|| &s[start..end])
}

fn name_end(b: &[u8], mut i: usize) -> usize {
    while b
        .get(i)
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    {
        i += 1;
    }
    i
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Parse an argument object, rejecting duplicate keys (serde_json would keep the last one
/// silently, which would be a silently altered call).
pub(crate) fn parse_args(s: &str) -> Result<Map<String, Value>, String> {
    match serde_json::from_str::<Strict>(s) {
        Ok(Strict(Value::Object(m))) => Ok(m),
        Ok(_) => Err("arguments must be a JSON object".into()),
        Err(e) => Err(format!("arguments are not valid JSON: {e}")),
    }
}

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate key `{key}`")));
            }
            let Strict(v) = map.next_value()?;
            out.insert(key, v);
        }
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_a_complete_call() {
        let s = "<<call send_email {\"subject\":\"a >> b\",\"body\":\"}\"}>> after";
        let Scan::Done { len, name, args } = scan_call(s) else {
            panic!("{:?}", scan_call(s))
        };
        assert_eq!(name, "send_email");
        assert_eq!(args, "{\"subject\":\"a >> b\",\"body\":\"}\"}");
        assert_eq!(&s[len..], " after");
    }

    #[test]
    fn every_proper_prefix_is_incomplete() {
        let s = "<<call t {\"a\":[1,{\"b\":\"x\\\"}\"}]} >>";
        for end in MARKER.len() + 1..s.len() {
            assert_eq!(
                scan_call(&s[..end]),
                Scan::Incomplete,
                "prefix {:?}",
                &s[..end]
            );
        }
        assert!(matches!(scan_call(s), Scan::Done { .. }));
    }

    #[test]
    fn structural_errors() {
        assert!(matches!(scan_call("<<call  {}>>"), Scan::Malformed(_)));
        assert!(matches!(scan_call("<<call t [1]>>"), Scan::Malformed(_)));
        assert!(matches!(scan_call("<<call t {} >x"), Scan::Malformed(_)));
        assert!(matches!(scan_call("<<call t! {}>>"), Scan::Malformed(_)));
    }

    #[test]
    fn args_must_be_a_duplicate_free_object() {
        assert!(parse_args("{\"a\":1,\"b\":{\"c\":[true,null,1.5]}}").is_ok());
        assert!(
            parse_args("{\"a\":1,\"a\":2}")
                .unwrap_err()
                .contains("duplicate key `a`")
        );
        assert!(parse_args("{\"a\":{\"b\":1,\"b\":1}}").is_err());
        assert!(parse_args("{\"a\":}").is_err());
    }
}
