//! Model output → validated tool calls.
//!
//! [`decode_calls`] is a [`StreamDecoder`] fed the whole text in one chunk, so whole-text and
//! streaming decoding share one code path and cannot disagree.

use serde_json::{Map, Value};

use crate::error::{CompactError, Result};
use crate::schema::{Object, Ty};
use crate::types::{ToolCall, ToolDef};
use crate::validate;

/// Opening marker of a call: `<<call NAME {JSON}>>`.
pub const CALL_OPEN: &str = "<<call";
/// Closing marker of a call.
pub const CALL_CLOSE: &str = ">>";
/// Longest call (marker to marker) the decoder buffers before failing closed.
pub const MAX_CALL_BYTES: usize = 1 << 20;

/// Text and calls from one model output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    /// Everything outside call markers, concatenated.
    pub text: String,
    /// Calls in the order they appear.
    pub calls: Vec<ToolCall>,
}

/// One unit of decoder output.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Plain text, safe to forward (it cannot be the start of a marker).
    Text(String),
    /// A complete, schema-valid call.
    Call(ToolCall),
}

/// Decodes the calls in `text`, validating each against `tools`. Any malformed call, unknown
/// tool or schema violation fails the whole decode; no call is guessed or repaired.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    decode(text, tools).map(|d| d.calls)
}

/// Like [`decode_calls`] but also returns the text outside the calls.
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

/// Validates one native tool call (`function.name` plus its `function.arguments` JSON string)
/// against `tools`, with the same rules as compact calls: unknown tool, invalid JSON, a repeated
/// key or a schema violation is an error, and nothing is coerced.
pub fn validate_call(name: &str, arguments_json: &str, tools: &[ToolDef]) -> Result<ToolCall> {
    let decoder = StreamDecoder::new(tools)?;
    decoder.check_name(name)?;
    let args = parse_args(name, arguments_json)?;
    decoder.validate(name.to_string(), args)
}

/// Tool name → argument schema, parsed once per request.
#[derive(Debug, Clone)]
struct Catalog {
    tools: Vec<(String, Object)>,
}

impl Catalog {
    fn new(tools: &[ToolDef]) -> Result<Catalog> {
        let mut out: Vec<(String, Object)> = Vec::with_capacity(tools.len());
        for t in tools {
            if out.iter().any(|(n, _)| n == &t.name) {
                return Err(CompactError::InvalidTool {
                    tool: t.name.clone(),
                    reason: "duplicate tool name".into(),
                });
            }
            out.push((t.name.clone(), parameters(t)?));
        }
        Ok(Catalog { tools: out })
    }

    fn get(&self, name: &str) -> Option<&Object> {
        self.tools.iter().find(|(n, _)| n == name).map(|(_, o)| o)
    }
}

/// The tool's argument object; a tool without `parameters` takes an empty, closed object.
pub(crate) fn parameters(t: &ToolDef) -> Result<Object> {
    match &t.parameters {
        None => Ok(Object {
            fields: Vec::new(),
            closed: true,
        }),
        Some(schema) => {
            Ty::parameters_from_json(schema).map_err(|reason| CompactError::Unsupported {
                tool: t.name.clone(),
                reason,
            })
        }
    }
}

/// Incremental decoder for streamed model output.
///
/// Feed chunks with [`push`](Self::push), then call [`finish`](Self::finish). Text is released as
/// soon as it cannot be part of a marker; a call is released once its closing `>>` arrives and it
/// validates. A marker split across chunks (`"<<ca"`, `"ll ..."`) is held back, never emitted as
/// text. After an error the decoder stays failed and returns that error again.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    catalog: Catalog,
    buf: String,
    failed: Option<CompactError>,
}

impl StreamDecoder {
    /// Builds a decoder for the request's tools.
    pub fn new(tools: &[ToolDef]) -> Result<StreamDecoder> {
        Ok(StreamDecoder {
            catalog: Catalog::new(tools)?,
            buf: String::new(),
            failed: None,
        })
    }

    /// Adds a chunk; returns the events it completes.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.buf.push_str(chunk);
        self.drain(false)
    }

    /// Ends the stream. An unterminated call is a [`CompactError::MalformedCall`].
    pub fn finish(mut self) -> Result<Vec<StreamEvent>> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        self.drain(true)
    }

    fn drain(&mut self, at_end: bool) -> Result<Vec<StreamEvent>> {
        let result = self.drain_inner(at_end);
        if let Err(e) = &result {
            self.failed = Some(e.clone());
        }
        result
    }

    fn drain_inner(&mut self, at_end: bool) -> Result<Vec<StreamEvent>> {
        let mut events = Vec::new();
        loop {
            let Some(start) = self.buf.find(CALL_OPEN) else {
                let keep = if at_end {
                    0
                } else {
                    marker_prefix_suffix(&self.buf)
                };
                let text: String = self.buf.drain(..self.buf.len() - keep).collect();
                push_text(&mut events, text);
                return Ok(events);
            };
            let text: String = self.buf.drain(..start).collect();
            push_text(&mut events, text);
            match scan_call(&self.buf, |name| self.check_name(name))? {
                Scan::Done { name, args, len } => {
                    events.push(StreamEvent::Call(self.validate(name, args)?));
                    self.buf.drain(..len);
                }
                Scan::NeedMore if at_end => return Err(malformed("output ended inside a call")),
                Scan::NeedMore if self.buf.len() > MAX_CALL_BYTES => {
                    return Err(malformed("call exceeds the size limit"));
                }
                Scan::NeedMore => return Ok(events),
            }
        }
    }

    fn check_name(&self, name: &str) -> Result<()> {
        match self.catalog.get(name) {
            Some(_) => Ok(()),
            None => Err(CompactError::UnknownTool { name: name.into() }),
        }
    }

    fn validate(&self, name: String, args: Map<String, Value>) -> Result<ToolCall> {
        let schema = self
            .catalog
            .get(&name)
            .ok_or_else(|| CompactError::UnknownTool { name: name.clone() })?;
        if let Err((path, reason)) = validate::object(&args, schema, "$") {
            return Err(CompactError::InvalidArguments {
                tool: name,
                path,
                reason,
            });
        }
        Ok(ToolCall {
            name,
            arguments: args,
        })
    }
}

fn push_text(events: &mut Vec<StreamEvent>, text: String) {
    if !text.is_empty() {
        events.push(StreamEvent::Text(text));
    }
}

fn malformed(reason: &str) -> CompactError {
    CompactError::MalformedCall {
        reason: reason.into(),
    }
}

/// Length of the longest suffix of `buf` that is a proper prefix of [`CALL_OPEN`].
fn marker_prefix_suffix(buf: &str) -> usize {
    (1..CALL_OPEN.len())
        .rev()
        .find(|&n| buf.ends_with(&CALL_OPEN[..n]))
        .unwrap_or(0)
}

enum Scan {
    /// The call is not complete yet.
    NeedMore,
    /// A complete call of `len` bytes.
    Done {
        name: String,
        args: Map<String, Value>,
        len: usize,
    },
}

/// Scans one call at the start of `buf` (which begins with [`CALL_OPEN`]).
///
/// `check_name` runs as soon as the name is complete, before anything after it is looked at, so
/// an unknown tool is reported the same way whatever follows it and however the text is chunked.
fn scan_call(buf: &str, check_name: impl Fn(&str) -> Result<()>) -> Result<Scan> {
    let bytes = buf.as_bytes();
    let mut i = CALL_OPEN.len();
    match bytes.get(i) {
        None => return Ok(Scan::NeedMore),
        Some(c) if c.is_ascii_whitespace() => {}
        Some(_) => return Err(malformed("expected a space after `<<call`")),
    }
    i = skip_ws(bytes, i);
    let name_start = i;
    while bytes.get(i).is_some_and(|c| is_name_byte(*c)) {
        i += 1;
    }
    if i == bytes.len() {
        return Ok(Scan::NeedMore);
    }
    if i == name_start {
        return Err(malformed("missing tool name"));
    }
    let name = buf[name_start..i].to_string();
    check_name(&name)?;
    i = skip_ws(bytes, i);
    match bytes.get(i) {
        None => Ok(Scan::NeedMore),
        // `<<call name>>`: a call with no arguments.
        Some(b'>') => match closing(bytes, i) {
            Some(true) => Ok(Scan::Done {
                name,
                args: Map::new(),
                len: i + CALL_CLOSE.len(),
            }),
            Some(false) => Err(malformed("expected `{` or `>>` after the tool name")),
            None => Ok(Scan::NeedMore),
        },
        Some(b'{') => {
            let Some(end) = json_object_end(bytes, i) else {
                return Ok(Scan::NeedMore);
            };
            let args = parse_args(&name, &buf[i..end])?;
            let j = skip_ws(bytes, end);
            match closing(bytes, j) {
                Some(true) => Ok(Scan::Done {
                    name,
                    args,
                    len: j + CALL_CLOSE.len(),
                }),
                Some(false) => Err(malformed("expected `>>` after the arguments")),
                None => Ok(Scan::NeedMore),
            }
        }
        Some(_) => Err(malformed("expected `{` or `>>` after the tool name")),
    }
}

/// `Some(true)` if `>>` starts at `i`, `None` if the input may still become `>>`.
fn closing(bytes: &[u8], i: usize) -> Option<bool> {
    let rest = bytes.get(i..).unwrap_or_default();
    let close = CALL_CLOSE.as_bytes();
    if rest.len() < close.len() {
        return if close.starts_with(rest) {
            None
        } else {
            Some(false)
        };
    }
    Some(rest.starts_with(close))
}

fn parse_args(name: &str, raw: &str) -> Result<Map<String, Value>> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(_)) if crate::json::has_duplicate_keys(raw) => {
            Err(CompactError::InvalidArguments {
                tool: name.into(),
                path: "$".into(),
                reason: "an object repeats a key".into(),
            })
        }
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) | Err(_) => Err(CompactError::InvalidArguments {
            tool: name.into(),
            path: "$".into(),
            reason: "arguments are not a valid JSON object".into(),
        }),
    }
}

/// End (exclusive) of the JSON object starting at `start`, tracking strings and escapes so a
/// `}` or `>>` inside a string never ends the call. `None` if the object is not closed yet.
fn json_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &c) in bytes.get(start..)?.iter().enumerate() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.')
}
