//! Decoding model output: an incremental scanner over the call grammar, then strict
//! validation of each call against the tool's original schema.
//!
//! [`crate::decode`] and [`crate::decode_calls`] are a single [`StreamDecoder`] fed the whole
//! text, so the streaming and non-streaming paths cannot disagree.

use std::collections::HashMap;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::schema::{self, Field};
use crate::{ToolCall, ToolDef};

const MARKER: &str = "<<call";
const CLOSE: &str = ">>";

/// A complete, validated piece of model output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decoded {
    /// All text outside calls, concatenated as written (not trimmed).
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// What a [`StreamDecoder::push`] made safe to forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// Text that can no longer turn out to be part of a call.
    Text(String),
    /// A complete call that passed validation.
    Call(ToolCall),
}

enum State {
    Text,
    /// Inside a call; the buffer holds everything after `<<call`.
    Call,
}

/// Incremental decoder. Feed chunks with [`push`](Self::push); markers, names, JSON and the
/// closing `>>` may be split anywhere. Text is held back only while it could still be the start
/// of a marker. After the first error the decoder is poisoned and returns that error forever.
pub struct StreamDecoder {
    tools: HashMap<String, Vec<Field>>,
    state: State,
    buf: String,
    out: Decoded,
    failed: Option<Error>,
}

impl StreamDecoder {
    /// Fails with [`Error::Unsupported`] if a tool's schema cannot be validated — the same tools
    /// [`crate::encode_tools`] refuses, so a bypassed request never reaches this decoder.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        let mut map = HashMap::new();
        for t in tools {
            let fields =
                schema::lower_root(t.parameters.as_ref()).map_err(|reason| Error::Unsupported {
                    tool: t.name.clone(),
                    reason,
                })?;
            map.insert(t.name.clone(), fields);
        }
        Ok(Self {
            tools: map,
            state: State::Text,
            buf: String::new(),
            out: Decoded::default(),
            failed: None,
        })
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.buf.push_str(chunk);
        let mut events = Vec::new();
        match self.drain(&mut events) {
            Ok(()) => Ok(events),
            Err(e) => {
                self.failed = Some(e.clone());
                Err(e)
            }
        }
    }

    /// End of stream: flush held-back text, or fail if a call is still open.
    pub fn finish(mut self) -> Result<Decoded> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        match self.state {
            State::Text => {
                let rest = std::mem::take(&mut self.buf);
                self.out.text.push_str(&rest);
                Ok(self.out)
            }
            State::Call => Err(Error::MalformedCall(format!(
                "stream ended inside a call: {MARKER}{}",
                self.buf
            ))),
        }
    }

    fn drain(&mut self, events: &mut Vec<StreamEvent>) -> Result<()> {
        loop {
            match self.state {
                State::Text => {
                    let (emit, marker) = scan_text(&self.buf);
                    if emit > 0 {
                        let text: String = self.buf.drain(..emit).collect();
                        self.out.text.push_str(&text);
                        events.push(StreamEvent::Text(text));
                    }
                    if !marker {
                        return Ok(());
                    }
                    self.buf.drain(..MARKER.len());
                    self.state = State::Call;
                }
                State::Call => match parse_call(&self.buf)? {
                    Scan::Incomplete(Some(name)) if !self.tools.contains_key(name) => {
                        return Err(Error::UnknownTool(name.to_owned()));
                    }
                    Scan::Incomplete(_) => return Ok(()),
                    Scan::Complete {
                        name,
                        args,
                        consumed,
                    } => {
                        let call = self.validate(name, args)?;
                        self.buf.drain(..consumed);
                        self.out.calls.push(call.clone());
                        events.push(StreamEvent::Call(call));
                        self.state = State::Text;
                    }
                },
            }
        }
    }

    fn validate(&self, name: &str, args: &str) -> Result<ToolCall> {
        let fields = self
            .tools
            .get(name)
            .ok_or_else(|| Error::UnknownTool(name.to_owned()))?;
        let invalid = |reason: String| Error::InvalidArguments {
            tool: name.to_owned(),
            reason,
        };
        let value = parse_strict(args).map_err(invalid)?;
        schema::validate_root(&value, fields).map_err(invalid)?;
        Ok(ToolCall {
            name: name.to_owned(),
            arguments: args.to_owned(),
        })
    }
}

/// Returns `(bytes safe to emit, whether a full marker starts there)`. A trailing proper prefix
/// of `<<call ` is held back, since the next chunk may complete it.
fn scan_text(buf: &str) -> (usize, bool) {
    let bytes = buf.as_bytes();
    let mut from = 0;
    while let Some(off) = buf.get(from..).and_then(|s| s.find('<')) {
        let i = from + off;
        let rest = buf.get(i..).unwrap_or("");
        if rest.starts_with(MARKER) {
            match bytes.get(i + MARKER.len()) {
                Some(b) if b.is_ascii_whitespace() => return (i, true),
                Some(_) => {}
                None => return (i, false),
            }
        } else if MARKER.starts_with(rest) {
            return (i, false);
        }
        from = i + 1;
    }
    (buf.len(), false)
}

enum Scan<'a> {
    /// Need more input; carries the tool name once it is known to be complete.
    Incomplete(Option<&'a str>),
    Complete {
        name: &'a str,
        args: &'a str,
        consumed: usize,
    },
}

/// Parse ` NAME {json}>>` (the marker already stripped). JSON is scanned structurally, string-
/// and escape-aware, so `>>` or `}` inside a string argument cannot end the call.
fn parse_call(buf: &str) -> Result<Scan<'_>> {
    let b = buf.as_bytes();
    let mut i = 0;
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let name_start = i;
    while b
        .get(i)
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
    {
        i += 1;
    }
    if i == b.len() {
        return Ok(Scan::Incomplete(None));
    }
    let name = buf.get(name_start..i).unwrap_or("");
    if name.is_empty() {
        return Err(malformed(buf, "expected a tool name"));
    }
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let (args, after) = match b.get(i) {
        None => return Ok(Scan::Incomplete(Some(name))),
        // `<<call name>>` — no arguments; validated as `{}`.
        Some(b'>') => ("{}", i),
        Some(b'{') => match json_object_end(b, i) {
            None => return Ok(Scan::Incomplete(Some(name))),
            Some(end) => (buf.get(i..end).unwrap_or(""), end),
        },
        Some(_) => return Err(malformed(buf, "expected '{' after the tool name")),
    };
    let mut j = after;
    while b.get(j).is_some_and(u8::is_ascii_whitespace) {
        j += 1;
    }
    let tail = buf.get(j..).unwrap_or("");
    if tail.starts_with(CLOSE) {
        Ok(Scan::Complete {
            name,
            args,
            consumed: j + CLOSE.len(),
        })
    } else if CLOSE.starts_with(tail) {
        Ok(Scan::Incomplete(Some(name)))
    } else {
        Err(malformed(buf, "expected '>>' after the arguments"))
    }
}

/// Index one past the `}` matching the `{` at `start`, or `None` if the input ends first.
fn json_object_end(b: &[u8], start: usize) -> Option<usize> {
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (k, &c) in b.iter().enumerate().skip(start) {
        if in_str {
            match c {
                _ if esc => esc = false,
                b'\\' => esc = true,
                b'"' => in_str = false,
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
                    return Some(k + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn malformed(buf: &str, what: &str) -> Error {
    let snippet: String = buf.chars().take(80).collect();
    Error::MalformedCall(format!("{what}: {MARKER}{snippet}"))
}

/// Parse JSON, rejecting duplicate object keys (serde_json would keep the last one silently,
/// while the raw argument string forwarded to the client would still carry both).
fn parse_strict(s: &str) -> std::result::Result<Value, String> {
    let mut de = serde_json::Deserializer::from_str(s);
    let v = Strict::deserialize(&mut de).map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(v.0)
}

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E>(self, v: f64) -> std::result::Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_str<E>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(k) = map.next_key::<String>()? {
            let Strict(v) = map.next_value()?;
            if out.insert(k.clone(), v).is_some() {
                return Err(de::Error::custom(format!("duplicate key '{k}'")));
            }
        }
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_back_partial_markers_only() {
        assert_eq!(scan_text("hello <<ca"), (6, false));
        assert_eq!(scan_text("a << b"), (6, false));
        assert_eq!(scan_text("a <"), (2, false));
        assert_eq!(scan_text("x <<call"), (2, false));
        assert_eq!(scan_text("x <<call f"), (2, true));
        assert_eq!(scan_text("x <<callback"), (12, false));
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        assert!(parse_strict(r#"{"a":1,"a":2}"#).is_err());
        assert!(parse_strict(r#"{"a":{"b":1},"c":[{"b":2}]}"#).is_ok());
    }
}
