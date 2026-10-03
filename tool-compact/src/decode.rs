//! Call-marker scanner, strict argument parsing and the incremental [`StreamDecoder`].
//!
//! The scanner is a deterministic function of the buffered text, re-run whenever more text
//! arrives, so chunk boundaries cannot influence the result: it either finds a complete marker,
//! needs more input, or knows the text is not a marker / is malformed.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::schema::{self, Sig};
use crate::{Error, Result, ToolCall, ToolDef};

const OPEN: &str = "<<call";

/// A marker still open after this many bytes is rejected instead of buffered without bound.
const MAX_MARKER_BYTES: usize = 1 << 20;

enum Scan {
    /// A prefix of a marker; wait for more text.
    NeedMore,
    /// `<<call` was followed by something other than whitespace: ordinary prose.
    NotMarker,
    Bad(String),
    Done {
        end: usize,
        name: String,
        args: String,
    },
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// Scan one marker. `s` must start with [`OPEN`].
///
/// Only ASCII delimiters are inspected, and a UTF-8 continuation byte never equals an ASCII
/// byte, so every slice below lands on a char boundary.
fn scan(s: &str) -> Scan {
    let b = s.as_bytes();
    let mut i = OPEN.len();
    match b.get(i) {
        None => return Scan::NeedMore,
        Some(c) if is_ws(*c) => {}
        Some(_) => return Scan::NotMarker,
    }
    while b.get(i).copied().is_some_and(is_ws) {
        i += 1;
    }

    let name_start = i;
    while b
        .get(i)
        .is_some_and(|c| !is_ws(*c) && *c != b'{' && *c != b'>')
    {
        i += 1;
    }
    if i >= b.len() {
        return Scan::NeedMore;
    }
    let name = &s[name_start..i];
    if name.is_empty() {
        return Scan::Bad("missing tool name".into());
    }
    if !name.bytes().all(schema::tool_name_byte) {
        return Scan::Bad(format!("invalid tool name {name:?}"));
    }

    let ws_start = i;
    while b.get(i).copied().is_some_and(is_ws) {
        i += 1;
    }
    match b.get(i) {
        None => return Scan::NeedMore,
        Some(b'{') if i > ws_start => {}
        Some(b'{') => {
            return Scan::Bad("expected whitespace between tool name and arguments".into());
        }
        Some(_) => return Scan::Bad("expected `{` to open the arguments object".into()),
    }

    // Find the end of the JSON object: depth tracking that ignores braces/brackets inside strings.
    let args_start = i;
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    loop {
        let Some(&c) = b.get(i) else {
            return Scan::NeedMore;
        };
        i += 1;
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
        } else {
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
    }
    let args = s[args_start..i].to_string();

    while b.get(i).copied().is_some_and(is_ws) {
        i += 1;
    }
    match (b.get(i), b.get(i + 1)) {
        (None, _) | (Some(b'>'), None) => Scan::NeedMore,
        (Some(b'>'), Some(b'>')) => Scan::Done {
            end: i + 2,
            name: name.to_string(),
            args,
        },
        _ => Scan::Bad("expected `>>` after the arguments".into()),
    }
}

/// Length of the longest proper prefix of [`OPEN`] that `buf` ends with.
fn partial_open_len(buf: &str) -> usize {
    (1..OPEN.len())
        .rev()
        .find(|k| buf.as_bytes().ends_with(&OPEN.as_bytes()[..*k]))
        .unwrap_or(0)
}

// ── strict JSON ─────────────────────────────────────────────────────────────

/// `serde_json::Value`, except an object with a repeated key is an error rather than last-wins:
/// silently picking one of two conflicting values would be a guess.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_u64<E>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::String(v.to_string())))
            }
            fn visit_unit<E>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut out = Vec::new();
                while let Some(Strict(v)) = a.next_element()? {
                    out.push(v);
                }
                Ok(Strict(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(de::Error::custom(format!("duplicate key `{k}`")));
                    }
                    let Strict(v) = a.next_value()?;
                    out.insert(k, v);
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}

// ── decoder ─────────────────────────────────────────────────────────────────

/// Incremental decoder for model output arriving in arbitrary chunks.
///
/// Feed text with [`push`](Self::push); each call returns the calls that *completed* in that
/// chunk. The result for any chunking equals decoding the concatenated text in one go. After the
/// first error the decoder stays failed: later input is never allowed to resurrect the stream.
pub struct StreamDecoder {
    compiled: HashMap<String, Sig>,
    /// Offered tools that have no compact form: calling them in compact syntax is an error.
    native_only: HashSet<String>,
    buf: String,
    /// Plain text seen so far that is not part of a marker and has not been handed out yet.
    text: String,
    failed: Option<Error>,
}

impl StreamDecoder {
    /// Fails on duplicate tool names, which would make a call ambiguous.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        crate::ensure_unique(tools)?;
        let mut compiled = HashMap::new();
        let mut native_only = HashSet::new();
        for t in tools {
            match schema::compile(t) {
                Ok(sig) => {
                    compiled.insert(t.name.clone(), sig);
                }
                Err(_) => {
                    native_only.insert(t.name.clone());
                }
            }
        }
        Ok(Self {
            compiled,
            native_only,
            buf: String::new(),
            text: String::new(),
            failed: None,
        })
    }

    /// Consume a chunk. Returns the calls completed by it (possibly none).
    ///
    /// On error nothing from this chunk is returned: one invalid call rejects the whole chunk.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        match self.drain(&mut out) {
            Ok(()) => Ok(out),
            Err(e) => {
                self.buf.clear();
                self.failed = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Plain text (everything outside call markers) seen since the last call, in order.
    ///
    /// A possible marker start (`<<ca…`) at the end of the input is held back until the next
    /// chunk shows whether it is a marker, so text is never emitted and then retracted.
    pub fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    /// Signal end of stream and return any plain text not yet taken, including a trailing
    /// partial marker start such as `<<ca`. A marker that never closed is an error, never a
    /// silent drop.
    pub fn finish(mut self) -> Result<String> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        if self.buf.starts_with(OPEN) {
            return Err(Error::MalformedCall(
                "stream ended inside a call marker".into(),
            ));
        }
        self.text.push_str(&self.buf);
        Ok(self.text)
    }

    fn drain(&mut self, out: &mut Vec<ToolCall>) -> Result<()> {
        loop {
            let Some(at) = self.buf.find(OPEN) else {
                // Keep a possible half-arrived `<<ca…`; everything before it is plain text.
                let cut = self.buf.len() - partial_open_len(&self.buf);
                self.text.extend(self.buf.drain(..cut));
                return Ok(());
            };
            self.text.extend(self.buf.drain(..at));
            match scan(&self.buf) {
                Scan::NeedMore if self.buf.len() > MAX_MARKER_BYTES => {
                    return Err(Error::MalformedCall(format!(
                        "call marker exceeds {MAX_MARKER_BYTES} bytes"
                    )));
                }
                Scan::NeedMore => return Ok(()),
                Scan::NotMarker => self.text.extend(self.buf.drain(..1)),
                Scan::Bad(msg) => return Err(Error::MalformedCall(msg)),
                Scan::Done { end, name, args } => {
                    out.push(self.validate(name, &args)?);
                    self.buf.drain(..end);
                }
            }
        }
    }

    fn validate(&self, name: String, args: &str) -> Result<ToolCall> {
        if self.native_only.contains(&name) {
            return Err(Error::UnsupportedSchema(name));
        }
        let Some(sig) = self.compiled.get(&name) else {
            return Err(Error::UnknownTool(name));
        };
        let invalid = |reason: String| Error::InvalidArguments {
            tool: name.clone(),
            reason,
        };
        let Strict(value) =
            serde_json::from_str(args).map_err(|e| invalid(format!("malformed JSON: {e}")))?;
        schema::validate_args(sig, &value).map_err(invalid)?;
        Ok(ToolCall {
            arguments: value.to_string(),
            name,
        })
    }
}
