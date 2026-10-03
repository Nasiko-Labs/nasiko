//! Decoding model output: text → validated tool calls.
//!
//! [`StreamDecoder`] is the only implementation; the one-shot [`decode`] feeds it the whole
//! text in one chunk, so streaming and non-streaming cannot disagree.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::encode::{CALL_CLOSE, CALL_OPEN};
use crate::schema::{Kind, Node};
use crate::validate::{Patterns, validate};
use crate::{Error, Result, ToolCall, ToolDef};

/// Largest single call accepted. A marker that never closes would otherwise buffer the whole
/// rest of the stream.
pub const MAX_CALL_BYTES: usize = 1 << 20;

/// One unit of decoded output, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// Plain text outside any call. Safe to forward: it can no longer become part of a marker.
    Text(String),
    /// A complete, validated call. `index` counts calls from 0 in output order — it is the
    /// OpenAI `ToolCallDelta.index`.
    Call { index: usize, call: ToolCall },
}

/// The result of decoding a whole response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Decoded {
    /// All text outside calls, concatenated.
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// Incremental decoder. Feed chunks with [`push`](Self::push), then call
/// [`finish`](Self::finish) once at end of stream.
///
/// Markers may be split anywhere — inside `<<call`, the tool name, the JSON (including inside
/// strings and escapes) or `>>`. Text that might be the start of a marker is held back until
/// the next chunk decides it.
///
/// Fail closed: the first error poisons the decoder, and every later `push`/`finish` returns
/// that same error. No call is emitted after an error, and no call is emitted before it is
/// fully validated.
pub struct StreamDecoder {
    tools: HashMap<String, Node>,
    /// Offered tool names, to recognise `<<name` written without `call`.
    names: Vec<String>,
    patterns: Patterns,
    buf: String,
    in_call: bool,
    next_index: usize,
    failed: Option<Error>,
}

/// Where the next marker is in the text buffer.
enum Marker {
    /// A complete opening marker starts here.
    At(usize),
    /// Nothing at or after this offset can be a marker yet — everything before it is text.
    TextUntil(usize),
}

/// Outcome of scanning a buffer that starts with `<<call`.
enum Scan<'a> {
    Incomplete,
    Malformed(String),
    Complete {
        name: &'a str,
        args: Option<&'a str>,
        consumed: usize,
    },
}

impl StreamDecoder {
    /// Build a decoder for this tool set. Fails if a tool cannot be compacted — a model that
    /// never saw a compact definition has no compact calls to decode.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        let compiled = crate::compile(tools)?;
        let mut map = HashMap::with_capacity(compiled.len());
        let mut patterns = Patterns::new();
        for tool in compiled {
            let node = Node {
                kind: Kind::Object(tool.params),
                description: None,
                ann: Default::default(),
            };
            let mut ps = Vec::new();
            node.patterns(&mut ps);
            for p in ps {
                if !patterns.contains_key(p) {
                    // Already proven to compile by `compile`; fail closed regardless.
                    let re = regex::Regex::new(p).map_err(|e| Error::Unsupported {
                        tool: tool.name.clone(),
                        path: "#".into(),
                        feature: format!("pattern: {e}"),
                    })?;
                    patterns.insert(p.to_string(), re);
                }
            }
            map.insert(tool.name, node);
        }
        let mut names: Vec<String> = map.keys().cloned().collect();
        names.sort();
        Ok(Self {
            names,
            tools: map,
            patterns,
            buf: String::new(),
            in_call: false,
            next_index: 0,
            failed: None,
        })
    }

    /// Feed the next chunk. Returns the events it completed.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.buf.push_str(chunk);
        let mut events = Vec::new();
        match self.drain(&mut events, false) {
            Ok(()) => Ok(events),
            Err(e) => {
                self.failed = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Signal end of stream. Flushes held-back text; an unterminated call is an error.
    pub fn finish(&mut self) -> Result<Vec<StreamEvent>> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let mut events = Vec::new();
        match self.drain(&mut events, true) {
            Ok(()) => Ok(events),
            Err(e) => {
                self.failed = Some(e.clone());
                Err(e)
            }
        }
    }

    fn drain(&mut self, events: &mut Vec<StreamEvent>, eof: bool) -> Result<()> {
        loop {
            if !self.in_call {
                match find_marker(&self.buf, &self.names, eof)? {
                    Marker::At(pos) => {
                        emit_text(events, &self.buf[..pos]);
                        self.buf.drain(..pos);
                        self.in_call = true;
                    }
                    Marker::TextUntil(pos) => {
                        emit_text(events, &self.buf[..pos]);
                        self.buf.drain(..pos);
                        return Ok(());
                    }
                }
            } else {
                let (call, consumed) = match scan_call(&self.buf) {
                    Scan::Incomplete if eof => {
                        return Err(Error::MalformedCall(
                            "unterminated call at end of output".into(),
                        ));
                    }
                    Scan::Incomplete if self.buf.len() > MAX_CALL_BYTES => {
                        return Err(Error::MalformedCall(format!(
                            "call longer than {MAX_CALL_BYTES} bytes"
                        )));
                    }
                    Scan::Incomplete => return Ok(()),
                    Scan::Malformed(reason) => return Err(Error::MalformedCall(reason)),
                    Scan::Complete {
                        name,
                        args,
                        consumed,
                    } => (self.check(name, args)?, consumed),
                };
                self.buf.drain(..consumed);
                self.in_call = false;
                events.push(StreamEvent::Call {
                    index: self.next_index,
                    call,
                });
                self.next_index += 1;
            }
        }
    }

    /// Resolve and validate one complete call.
    fn check(&self, name: &str, args: Option<&str>) -> Result<ToolCall> {
        let Some(schema) = self.tools.get(name) else {
            return Err(Error::UnknownTool(name.to_string()));
        };
        let raw = args.unwrap_or("{}");
        // Duplicate keys first: `serde_json::Value` would silently keep the last one, and a
        // consumer parsing the raw string might keep the first — a validated value that is not
        // the value delivered.
        serde_json::from_str::<NoDuplicateKeys>(raw)
            .map_err(|e| Error::MalformedCall(format!("arguments of '{name}': {e}")))?;
        let value: Value = serde_json::from_str(raw)
            .map_err(|e| Error::MalformedCall(format!("arguments of '{name}': {e}")))?;
        if !value.is_object() {
            return Err(Error::MalformedCall(format!(
                "arguments of '{name}' are not a JSON object"
            )));
        }
        validate(schema, &value, "$", &self.patterns).map_err(|reason| {
            Error::InvalidArguments {
                tool: name.to_string(),
                reason,
            }
        })?;
        Ok(ToolCall {
            name: name.to_string(),
            // Exactly what the model wrote: validated, never rewritten.
            arguments: raw.to_string(),
        })
    }
}

fn emit_text(events: &mut Vec<StreamEvent>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(StreamEvent::Text(prev)) = events.last_mut() {
        prev.push_str(text);
    } else {
        events.push(StreamEvent::Text(text.to_string()));
    }
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// What a `<<` in text turns out to be.
enum Opening {
    /// `<<call` + whitespace.
    Call,
    /// `<<` + the name of an offered tool, without `call` (`<<send_email {…}>>`) — a call
    /// attempt in the wrong shape.
    Bare(String),
    /// Cannot be decided until more input arrives.
    Undecided,
    Text,
}

/// Classify the text after a `<<` (`rest`).
fn classify(rest: &str, names: &[String], eof: bool) -> Opening {
    let word_len = rest.bytes().take_while(|b| is_name_byte(*b)).count();
    let word = &rest[..word_len];
    let next = rest[word_len..].chars().next();
    if word == "call" {
        return match next {
            Some(c) if c.is_whitespace() => Opening::Call,
            Some(_) => Opening::Text,
            None if eof => Opening::Text,
            None => Opening::Undecided,
        };
    }
    let known = names.iter().any(|n| n == word);
    match next {
        Some(_) if known => Opening::Bare(word.to_string()),
        Some(_) => Opening::Text,
        None if eof && known => Opening::Bare(word.to_string()),
        None if eof => Opening::Text,
        // At the end of the buffer the word may still grow into `call` or a tool name.
        None if "call".starts_with(word) || names.iter().any(|n| n.starts_with(word)) => {
            Opening::Undecided
        }
        None => Opening::Text,
    }
}

/// Find the next opening marker: `<<call` followed by whitespace. `<<call` followed by
/// anything else (`<<callback`) is text. `<<` directly followed by an offered tool's name is
/// a malformed call: reporting it as text would turn a failed call into a silent non-call.
fn find_marker(buf: &str, names: &[String], eof: bool) -> Result<Marker> {
    let mut from = 0;
    while let Some(rel) = buf[from..].find("<<") {
        let pos = from + rel;
        match classify(&buf[pos + 2..], names, eof) {
            Opening::Call => return Ok(Marker::At(pos)),
            Opening::Bare(name) => {
                return Err(Error::MalformedCall(format!(
                    "'<<{name}' is missing 'call' (expected <<call {name} {{…}}>>)"
                )));
            }
            Opening::Undecided => return Ok(Marker::TextUntil(pos)),
            Opening::Text => from = pos + 1,
        }
    }
    // A lone trailing `<` may become `<<`. ASCII, so the offset is a char boundary.
    if !eof && buf.ends_with('<') {
        return Ok(Marker::TextUntil(buf.len() - 1));
    }
    Ok(Marker::TextUntil(buf.len()))
}

/// Scan one call. `buf` starts with `<<call`.
fn scan_call(buf: &str) -> Scan<'_> {
    let s = &buf[CALL_OPEN.len()..];
    let bytes = s.as_bytes();
    let mut i = 0;
    let skip_ws = |i: &mut usize| {
        while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };

    skip_ws(&mut i);
    // Tool name: everything up to whitespace, `{`, `(` or `>`. Validated against the tool set,
    // so a garbled name becomes `unknown_tool`, not a guess.
    let name_start = i;
    while i < bytes.len()
        && !bytes[i].is_ascii_whitespace()
        && !matches!(bytes[i], b'{' | b'(' | b'>')
    {
        i += 1;
    }
    if i == bytes.len() {
        return Scan::Incomplete;
    }
    let name = &s[name_start..i];
    if name.is_empty() {
        return Scan::Malformed("missing tool name after <<call".into());
    }
    skip_ws(&mut i);
    if i == bytes.len() {
        return Scan::Incomplete;
    }

    let mut args = None;
    if bytes[i] == b'{' {
        let start = i;
        let mut depth = 0usize;
        let mut in_str = false;
        let mut escaped = false;
        let mut end = None;
        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            if in_str {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    in_str = false;
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            return Scan::Incomplete;
        };
        args = Some(&s[start..end]);
        skip_ws(&mut i);
        if i == bytes.len() {
            return Scan::Incomplete;
        }
    }

    match &bytes[i..] {
        [b'>'] => Scan::Incomplete,
        [b'>', b'>', ..] => Scan::Complete {
            name,
            args,
            consumed: CALL_OPEN.len() + i + CALL_CLOSE.len(),
        },
        _ if args.is_none() => Scan::Malformed(format!(
            "expected a JSON object or '>>' after tool name '{name}'"
        )),
        _ => Scan::Malformed(format!("expected '>>' after the arguments of '{name}'")),
    }
}

/// Deserializes any JSON, failing on a duplicate object key at any depth.
struct NoDuplicateKeys;

impl<'de> Deserialize<'de> for NoDuplicateKeys {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(NoDuplicateKeysVisitor)
    }
}

struct NoDuplicateKeysVisitor;

impl<'de> Visitor<'de> for NoDuplicateKeysVisitor {
    type Value = NoDuplicateKeys;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> std::result::Result<Self::Value, E> {
        Ok(NoDuplicateKeys)
    }
    fn visit_i64<E>(self, _: i64) -> std::result::Result<Self::Value, E> {
        Ok(NoDuplicateKeys)
    }
    fn visit_u64<E>(self, _: u64) -> std::result::Result<Self::Value, E> {
        Ok(NoDuplicateKeys)
    }
    fn visit_f64<E>(self, _: f64) -> std::result::Result<Self::Value, E> {
        Ok(NoDuplicateKeys)
    }
    fn visit_str<E>(self, _: &str) -> std::result::Result<Self::Value, E> {
        Ok(NoDuplicateKeys)
    }
    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(NoDuplicateKeys)
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        while seq.next_element::<NoDuplicateKeys>()?.is_some() {}
        Ok(NoDuplicateKeys)
    }
    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate key '{key}'")));
            }
            map.next_value::<NoDuplicateKeys>()?;
        }
        Ok(NoDuplicateKeys)
    }
}

/// Decode a complete response: text plus validated calls.
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut events = decoder.push(text)?;
    events.extend(decoder.finish()?);
    let mut out = Decoded::default();
    for e in events {
        match e {
            StreamEvent::Text(t) => out.text.push_str(&t),
            StreamEvent::Call { call, .. } => out.calls.push(call),
        }
    }
    Ok(out)
}

/// Decode only the calls from a complete response. Plain answers decode to `Ok(vec![])`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    decode(text, tools).map(|d| d.calls)
}
