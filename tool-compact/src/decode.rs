//! Strict decoding of compact tool calls from model output, whole or streamed.
//!
//! [`decode_calls`] is [`StreamDecoder`] fed a single chunk, so the buffered and streaming paths
//! share every rule and cannot drift apart.

use std::collections::HashMap;

use serde_json::Value;

use crate::schema::{self, Field};
use crate::{CALL_CLOSE, CALL_OPEN, Error, Result, ToolCall, ToolDef};

/// A single call's `<<call … >>` span may not exceed this. Past it the output is treated as
/// malformed instead of being buffered without bound.
pub const MAX_CALL_BYTES: usize = 1 << 20;

/// What a decoded model reply contains: the prose around the calls, and the calls in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Decoded {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

impl Decoded {
    /// The prose as assistant `content`: trimmed, with code fences left empty by the removed
    /// calls dropped. `None` when nothing but whitespace remains.
    pub fn content(&self) -> Option<String> {
        let bare_fence = |line: &str| {
            line.trim()
                .strip_prefix("```")
                .is_some_and(|lang| lang.chars().all(|c| c.is_ascii_alphanumeric()))
        };
        let kept: Vec<&str> = if self.calls.is_empty() {
            self.text.lines().collect()
        } else {
            self.text.lines().filter(|l| !bare_fence(l)).collect()
        };
        let joined = kept.join("\n");
        let trimmed = joined.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

/// An increment of decoded output.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Prose that can no longer be part of a call marker; safe to forward to the client.
    Text(String),
    /// A complete call that passed validation against its tool's schema.
    Call(ToolCall),
}

/// Decode a complete model reply into validated calls.
///
/// All or nothing: if any call is unknown, malformed or invalid, the whole reply is an error.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    decode(text, tools).map(|d| d.calls)
}

/// Like [`decode_calls`], but also returns the prose around the calls.
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut events = decoder.push(text)?;
    events.extend(decoder.finish()?);
    let mut out = Decoded::default();
    for event in events {
        match event {
            StreamEvent::Text(t) => out.text.push_str(&t),
            StreamEvent::Call(c) => out.calls.push(c),
        }
    }
    Ok(out)
}

/// Incremental decoder. Feed chunks with [`push`](Self::push), then call
/// [`finish`](Self::finish) once at end of stream.
///
/// Text is released as soon as it cannot be the start of a marker; a trailing `<`, `<<`, …
/// `<<call` is held back until the next chunk decides it. A call is released only once its
/// closing `>>` arrives and its arguments validate. After the first error the decoder stays
/// failed and returns that error from every later call.
#[derive(Debug)]
pub struct StreamDecoder {
    schemas: HashMap<String, Vec<Field>>,
    buf: String,
    failed: Option<Error>,
}

impl StreamDecoder {
    /// Fails if any tool's schema is outside the supported subset, the same rule
    /// [`encode_tools`](crate::encode_tools) applies, so a decoder exists only for tool sets
    /// that could have been compacted.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        let mut schemas = HashMap::with_capacity(tools.len());
        for tool in tools {
            let fields = crate::encode::lower(tool)?;
            if schemas.insert(tool.name.clone(), fields).is_some() {
                return Err(Error::UnsupportedSchema {
                    tool: tool.name.clone(),
                    reason: "duplicate tool name".into(),
                });
            }
        }
        Ok(Self {
            schemas,
            buf: String::new(),
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

    pub fn finish(mut self) -> Result<Vec<StreamEvent>> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        let mut events = Vec::new();
        self.drain(&mut events)?;
        if self.buf.starts_with(CALL_OPEN) && self.buf.len() > CALL_OPEN.len() {
            return Err(Error::MalformedCall("output ended inside a call".into()));
        }
        if !self.buf.is_empty() {
            events.push(StreamEvent::Text(std::mem::take(&mut self.buf)));
        }
        Ok(events)
    }

    fn drain(&mut self, events: &mut Vec<StreamEvent>) -> Result<()> {
        loop {
            match find_marker(&self.buf) {
                Marker::At(start) => {
                    if start > 0 {
                        let text: String = self.buf.drain(..start).collect();
                        events.push(StreamEvent::Text(text));
                    }
                    match scan_call(&self.buf)? {
                        Scan::Incomplete => {
                            if self.buf.len() > MAX_CALL_BYTES {
                                return Err(Error::MalformedCall(format!(
                                    "call exceeds {MAX_CALL_BYTES} bytes"
                                )));
                            }
                            return Ok(());
                        }
                        Scan::Complete { len, name, args } => {
                            let call = self.validate(name, args)?;
                            self.buf.drain(..len);
                            events.push(StreamEvent::Call(call));
                        }
                    }
                }
                Marker::Partial(start) => {
                    if start > 0 {
                        let text: String = self.buf.drain(..start).collect();
                        events.push(StreamEvent::Text(text));
                    }
                    return Ok(());
                }
                Marker::None => {
                    if !self.buf.is_empty() {
                        events.push(StreamEvent::Text(std::mem::take(&mut self.buf)));
                    }
                    return Ok(());
                }
            }
        }
    }

    fn validate(&self, name: &str, args: &str) -> Result<ToolCall> {
        let fields = self
            .schemas
            .get(name)
            .ok_or_else(|| Error::UnknownTool(name.to_string()))?;
        let invalid = |reason: String| Error::InvalidArguments {
            tool: name.to_string(),
            reason,
        };
        let value: Value = serde_json::from_str(args)
            .map_err(|e| invalid(format!("arguments are not JSON: {e}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| invalid("arguments are not a JSON object".into()))?;
        schema::validate_object(fields, obj, "").map_err(invalid)?;
        Ok(ToolCall {
            name: name.to_string(),
            arguments: args.to_string(),
        })
    }
}

enum Marker {
    /// A full `<<call` plus a following whitespace character starts here.
    At(usize),
    /// The buffer ends with something that may still become a marker.
    Partial(usize),
    None,
}

fn find_marker(buf: &str) -> Marker {
    let mut from = 0;
    while let Some(rel) = buf.get(from..).and_then(|s| s.find(CALL_OPEN)) {
        let start = from + rel;
        let after = start + CALL_OPEN.len();
        match buf.get(after..).and_then(|s| s.chars().next()) {
            None => return Marker::Partial(start),
            Some(c) if c.is_whitespace() => return Marker::At(start),
            // `<<caller` and the like are prose.
            Some(_) => from = after,
        }
    }
    // Hold back the longest suffix that is a proper prefix of the opener.
    for k in (1..CALL_OPEN.len()).rev() {
        if buf.len() >= k && buf.is_char_boundary(buf.len() - k) {
            let tail = buf.get(buf.len() - k..).unwrap_or("");
            if CALL_OPEN.starts_with(tail) {
                return Marker::Partial(buf.len() - k);
            }
        }
    }
    Marker::None
}

enum Scan<'a> {
    Incomplete,
    Complete {
        len: usize,
        name: &'a str,
        args: &'a str,
    },
}

/// Scan one call at the start of `buf`, which begins with `<<call` + whitespace.
fn scan_call(buf: &str) -> Result<Scan<'_>> {
    let malformed = |why: &str| Err(Error::MalformedCall(why.to_string()));
    let bytes = buf.as_bytes();
    let mut i = CALL_OPEN.len();
    let skip_ws = |mut i: usize| {
        while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        i
    };

    i = skip_ws(i);
    let name_start = i;
    while i < bytes.len() && schema::is_ident_char(bytes[i] as char) {
        i += 1;
    }
    if i == bytes.len() {
        return Ok(Scan::Incomplete);
    }
    if i == name_start {
        return malformed("expected a tool name after `<<call`");
    }
    let name = buf.get(name_start..i).unwrap_or("");

    i = skip_ws(i);
    match bytes.get(i) {
        None => return Ok(Scan::Incomplete),
        Some(b'{') => {}
        Some(_) => return malformed("expected `{` to open the arguments"),
    }
    let args_start = i;
    let Some(args_len) = balanced_len(buf.get(args_start..).unwrap_or("")) else {
        return Ok(Scan::Incomplete);
    };
    let args = buf.get(args_start..args_start + args_len).unwrap_or("");

    i = skip_ws(args_start + args_len);
    let close = CALL_CLOSE.as_bytes();
    let available = &bytes[i.min(bytes.len())..];
    if available.len() < close.len() {
        return if close.starts_with(available) {
            Ok(Scan::Incomplete)
        } else {
            malformed("expected `>>` after the arguments")
        };
    }
    if !available.starts_with(close) {
        return malformed("expected `>>` after the arguments");
    }
    Ok(Scan::Complete {
        len: i + close.len(),
        name,
        args,
    })
}

/// Byte length of the bracketed JSON value at the start of `s` (`{…}` or `[…]`), tracking
/// strings and escapes so brackets and `>>` inside strings are inert. `None` if it never closes.
pub(crate) fn balanced_len(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, b) in s.bytes().enumerate() {
        if in_str {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}
