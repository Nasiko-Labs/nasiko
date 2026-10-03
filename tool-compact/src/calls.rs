//! Call grammar and decoder.
//!
//! ```text
//! call  = "<<call" WS+ NAME WS* OBJECT WS* ">>"
//! NAME  = [A-Za-z0-9_.-]+
//! OBJECT = a JSON object (strict JSON, duplicate keys rejected)
//! ```
//!
//! Text outside calls is free prose. Once `<<call` appears, what follows must be a complete,
//! well-formed call; anything else is `Error::Malformed`. Two layers: an outer scanner finds
//! the marker and name, a JSON scanner finds the object's exact end offset (it knows strings
//! and escapes, so `>>` or `}` inside a string never ends the call), then `serde_json` parses
//! exactly that span.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::compact::is_name;
use crate::{Error, ToolCall, ToolDef, validate_arguments};

/// Opens every call.
pub const MARKER: &str = "<<call";
const CLOSE: &str = ">>";

/// Write `call` in the call grammar (what the model is asked to produce).
pub fn render_call(call: &ToolCall) -> String {
    format!("{MARKER} {} {}{CLOSE}", call.name, call.arguments)
}

/// Decode every call in a complete model output. Fails on the first malformed, unknown or
/// invalid call: a partial or guessed result is never returned.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, Error> {
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(MARKER) {
        let tail = rest.get(at..).unwrap_or_default();
        let (call, used) = match scan_call(tail)? {
            Scan::Call { name, json, used } => (finish_call(name, json, tools)?, used),
            Scan::Incomplete => return Err(malformed("unterminated call")),
        };
        calls.push(call);
        rest = tail.get(used..).unwrap_or_default();
    }
    Ok(calls)
}

pub(crate) enum Scan<'a> {
    /// A syntactically complete call: tool name, the JSON span, bytes consumed.
    Call {
        name: &'a str,
        json: &'a str,
        used: usize,
    },
    /// `s` is a valid prefix of a call; more input is needed.
    Incomplete,
}

fn malformed(reason: impl Into<String>) -> Error {
    Error::Malformed(reason.into())
}

/// Scan one call at the start of `s` (which starts with [`MARKER`]).
pub(crate) fn scan_call(s: &str) -> Result<Scan<'_>, Error> {
    let b = s.as_bytes();
    let mut i = MARKER.len();
    let ws = |i: usize| b.get(i).is_some_and(|c| c.is_ascii_whitespace());
    let name_char = |i: usize| {
        b.get(i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    };
    if !ws(i) {
        return if i >= b.len() {
            Ok(Scan::Incomplete)
        } else {
            Err(malformed("expected a space after <<call"))
        };
    }
    while ws(i) {
        i += 1;
    }
    let name_start = i;
    while name_char(i) {
        i += 1;
    }
    let name = s.get(name_start..i).unwrap_or_default();
    while ws(i) {
        i += 1;
    }
    match b.get(i) {
        None => return Ok(Scan::Incomplete),
        Some(b'{') if is_name(name) => {}
        Some(_) => return Err(malformed("expected <<call NAME {JSON args}>>")),
    }
    let json_start = i;
    let Some(json_end) = json_object_end(b, json_start) else {
        return Ok(Scan::Incomplete);
    };
    i = json_end;
    while ws(i) {
        i += 1;
    }
    let after = s.get(i..).unwrap_or_default();
    if after.starts_with(CLOSE) {
        Ok(Scan::Call {
            name,
            json: s.get(json_start..json_end).unwrap_or_default(),
            used: i + CLOSE.len(),
        })
    } else if CLOSE.starts_with(after) {
        Ok(Scan::Incomplete)
    } else {
        Err(malformed("expected >> after the arguments"))
    }
}

/// End offset (exclusive) of the JSON object starting at `b[start] == b'{'`, or `None` if
/// the input ends first. Only finds the boundary; validity is serde_json's job.
fn json_object_end(b: &[u8], start: usize) -> Option<usize> {
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (i, &c) in b.iter().enumerate().skip(start) {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Turn a scanned call into a validated [`ToolCall`].
pub(crate) fn finish_call(name: &str, json: &str, tools: &[ToolDef]) -> Result<ToolCall, Error> {
    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| Error::UnknownTool(name.to_string()))?;
    let Strict(arguments) =
        serde_json::from_str(json).map_err(|e| malformed(format!("bad JSON arguments: {e}")))?;
    validate_arguments(tool, &arguments)?;
    Ok(ToolCall {
        name: name.to_string(),
        arguments,
    })
}

/// A JSON value that rejects duplicate object keys (serde_json keeps the last silently,
/// which would be a silently altered call).
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
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
        Ok(Value::String(v.to_string()))
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
        while let Some(k) = map.next_key::<String>()? {
            let Strict(v) = map.next_value()?;
            if out.contains_key(&k) {
                return Err(de::Error::custom(format!("duplicate key `{k}`")));
            }
            out.insert(k, v);
        }
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
                name: "get_time".into(),
                description: None,
                parameters: None,
            },
        ]
    }

    const GOOD: &str = r#"<<call create_calendar_event {"title":"Retro >> }{ \"x\"","start":"2026-10-04T10:00:00+05:30"}>>"#;

    fn label(text: &str) -> &'static str {
        decode_calls(text, &tools()).unwrap_err().as_label()
    }

    #[test]
    fn decodes_calls_with_surrounding_text() {
        let t = tools();
        assert!(decode_calls("Sure, it is sunny.", &t).unwrap().is_empty());
        assert!(decode_calls("a << b, <<cal", &t).unwrap().is_empty());
        let calls = decode_calls(
            &format!("Booking now.\n{GOOD}\nand <<call get_time {{}}>> done"),
            &t,
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["title"], json!("Retro >> }{ \"x\""));
        assert_eq!(calls[1].name, "get_time");
        let spaced = "<<call  get_time\n{ }\n>>";
        assert_eq!(decode_calls(spaced, &t).unwrap().len(), 1);
    }

    #[test]
    fn render_call_round_trips() {
        let call = ToolCall {
            name: "create_calendar_event".into(),
            arguments: json!({"title": "Ünïcode \"q\" >>", "start": "2026-10-04T10:00:00Z"}),
        };
        assert_eq!(
            decode_calls(&render_call(&call), &tools()).unwrap(),
            vec![call]
        );
    }

    #[test]
    fn i5_unknown_tool_is_an_error() {
        assert_eq!(label("<<call delete_everything {}>>"), "unknown_tool");
    }

    #[test]
    fn i4_invalid_or_malformed_is_never_a_call() {
        for (text, want) in [
            (
                r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
                "invalid_arguments",
            ),
            (
                r#"<<call create_calendar_event {"title":"a"}>>"#,
                "invalid_arguments",
            ),
            (r#"<<call get_time {"x":1}>>"#, "invalid_arguments"),
            (r#"<<call get_time {"a":1,"a":2}>>"#, "malformed"),
            (r#"<<call get_time {'a':1}>>"#, "malformed"),
            (r#"<<call get_time {}"#, "malformed"),
            (r#"<<call get_time {} >"#, "malformed"),
            (r#"<<call get_time {}> >"#, "malformed"),
            (r#"<<call get_time []>>"#, "malformed"),
            (r#"<<callget_time {}>>"#, "malformed"),
            (r#"<<call {}>>"#, "malformed"),
            (r#"<<call get time {}>>"#, "malformed"),
            (r#"<<call get_time {}>> then <<call"#, "malformed"),
            (r#"<<call get_time {"a":[1}>>"#, "malformed"),
        ] {
            assert_eq!(label(text), want, "{text}");
        }
    }

    /// Mutations of a valid call never panic and never yield a call that fails validation.
    #[test]
    fn fuzz_mutations_never_yield_bad_calls() {
        let t = tools();
        let good = format!("x {GOOD} y");
        let chars: Vec<char> = good.chars().collect();
        let mut inputs = Vec::new();
        for i in 0..=chars.len() {
            inputs.push(chars[..i].iter().collect::<String>());
            let mut del = chars.clone();
            if i < del.len() {
                del.remove(i);
                inputs.push(del.into_iter().collect());
            }
            for c in ['"', '\\', '{', '}', '<', '>', ' ', 'é'] {
                let mut ins = chars.clone();
                ins.insert(i, c);
                inputs.push(ins.into_iter().collect());
            }
        }
        for s in inputs {
            if let Ok(calls) = decode_calls(&s, &t) {
                for c in calls {
                    let tool = t.iter().find(|x| x.name == c.name).unwrap();
                    assert!(validate_arguments(tool, &c.arguments).is_ok(), "{s}");
                }
            }
        }
    }
}
