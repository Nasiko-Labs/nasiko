//! `<<call NAME {json}>>` — rendering, decoding and validation of model tool calls.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::cursor::Cursor;
use crate::error::{Error, Result};
use crate::schema::{is_name_char, params_from_schema, validate_args};
use crate::types::{ToolCall, ToolDef};

const OPEN: &str = "<<";
const KEYWORD: &str = "call";
const CLOSE: &str = ">>";

/// The result of [`decode_calls`]: the reply with call markers cut out, and the calls.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    /// The input with **only** the call markers (`<<` through `>>`) removed. Every other byte is
    /// kept exactly as it was: nothing is trimmed or joined.
    pub text: String,
    /// The validated calls, in order of appearance.
    pub calls: Vec<ToolCall>,
}

/// Render a call in the exact form the model is told to emit. `decode_calls` of the result
/// returns the same call (for arguments valid against the tool's schema).
pub fn render_call(call: &ToolCall) -> String {
    format!(
        "{OPEN}{KEYWORD} {} {}{CLOSE}",
        call.name,
        Value::Object(call.arguments.clone())
    )
}

/// Extract and validate every call in a model reply.
///
/// **Marker rule.** `<<`, optional whitespace, then `call` in any letter case.
///
/// * Whitespace after `call` makes it a call attempt. The tool name and then `{` must follow
///   (whitespace allowed before the `{`), or it is [`Error::InvalidArguments`]. This covers
///   `<<call send_email: {..}>>`, `<<call "send_email" {..}>>` and
///   `<<call send_email args {..}>>`.
/// * Text glued to `call`: if a name and `{` follow (`<<callsend_email {..}>>`), it is
///   [`Error::InvalidArguments`]; otherwise (`<<callback>>`) the `<<` is prose, like `a << b`.
///
/// For a call attempt:
///
/// * A name not in `tools`, including `NAME` echoed from the instruction, is
///   [`Error::UnknownTool`]. Names match exactly, so `send_email2` never resolves to
///   `send_email`.
/// * The JSON object is read with serde_json's streaming deserializer, so `>>` or `<<call ..>>`
///   inside a string is data. `>>` must follow the object.
/// * Bad or non-object JSON, an unterminated call, a missing required field, a wrong type, an
///   enum or limit violation, a duplicate key or an undeclared argument is
///   [`Error::InvalidArguments`]. Nothing is coerced or repaired.
///
/// Two cases this crate also treats as failed call attempts, to stay fail-closed: input that
/// ends right after `<<call` (or after `<<call ` and a name), as a truncated stream would, and
/// `<<call {..}` with no name.
///
/// The first bad call fails the whole reply, and no text is returned with an error. A reply
/// with no call is `Ok` with the text unchanged and no calls.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Decoded> {
    for (i, t) in tools.iter().enumerate() {
        if tools.iter().skip(i + 1).any(|u| u.name == t.name) {
            return Err(Error::InvalidTools(format!(
                "duplicate tool name '{}'",
                t.name
            )));
        }
    }
    let mut out = Decoded::default();
    let mut c = Cursor::new(text);
    while let Some(idx) = c.rest().find(OPEN) {
        let prose = c.rest().get(..idx).unwrap_or("");
        let mut m = Cursor::new(c.rest().get(idx..).unwrap_or(""));
        match call_at(&mut m, tools)? {
            Some(call) => {
                out.text.push_str(prose);
                out.calls.push(call);
                let consumed = idx + m.pos();
                if !c.advance(consumed) {
                    return Err(Error::InvalidArguments("bad call boundary".into()));
                }
            }
            None => {
                // Keep one '<' and rescan from the next one, so "<<<call ..>>" still finds the call.
                out.text.push_str(prose);
                out.text.push('<');
                if !c.advance(idx + 1) {
                    return Err(Error::InvalidArguments("bad call boundary".into()));
                }
            }
        }
    }
    out.text.push_str(c.rest());
    Ok(out)
}

/// Parse a call starting at `<<`. `Ok(None)` means the `<<` is prose.
fn call_at(m: &mut Cursor<'_>, tools: &[ToolDef]) -> Result<Option<ToolCall>> {
    m.eat(OPEN);
    m.skip_ws();
    let is_keyword = m
        .rest()
        .get(..KEYWORD.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(KEYWORD));
    if !is_keyword || !m.advance(KEYWORD.len()) {
        return Ok(None);
    }
    let gap = m.skip_ws();
    let name = m.take_while(is_name_char);
    m.skip_ws();
    if m.is_empty() {
        // `<<call` / `<<call name` at end of input: a truncated call, not prose.
        if gap > 0 || name.is_empty() {
            return Err(Error::InvalidArguments("unterminated call".into()));
        }
        return Ok(None);
    }
    if m.peek() != Some('{') {
        // Whitespace after "call" means a call was clearly intended ("<<call send_email: {..}",
        // "<<call \"send_email\" {..}"), so a missing "{" is a broken call. Only a word glued to
        // "call" with no "{" ("<<callback>>") is prose.
        if gap > 0 {
            return Err(Error::InvalidArguments(format!(
                "expected '{{' after '<<call {name}'"
            )));
        }
        return Ok(None);
    }
    if name.is_empty() {
        return Err(Error::InvalidArguments(
            "expected tool name after '<<call'".into(),
        ));
    }
    if gap == 0 {
        return Err(Error::InvalidArguments(format!(
            "expected whitespace between 'call' and '{name}'"
        )));
    }
    let Some(tool) = tools.iter().find(|t| t.name == name) else {
        return Err(Error::UnknownTool(name.to_string()));
    };
    let arguments = parse_args(m, name)?;
    m.skip_ws();
    if !m.eat(CLOSE) {
        return Err(Error::InvalidArguments(format!(
            "{name}: expected '{CLOSE}' after arguments"
        )));
    }
    let params = params_from_schema(tool.parameters.as_ref()).map_err(|u| {
        Error::InvalidTools(format!(
            "{name}: unsupported schema at {}: {}",
            u.path, u.detail
        ))
    })?;
    validate_args(&params, &Value::Object(arguments.clone()))
        .map_err(|e| Error::InvalidArguments(format!("{name}: {e}")))?;
    Ok(Some(ToolCall {
        name: name.to_string(),
        arguments,
    }))
}

fn parse_args(c: &mut Cursor<'_>, name: &str) -> Result<Map<String, Value>> {
    let mut stream = serde_json::Deserializer::from_str(c.rest()).into_iter::<Value>();
    let value = match stream.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) if e.is_eof() => {
            return Err(Error::InvalidArguments(format!(
                "{name}: unterminated call"
            )));
        }
        Some(Err(e)) => return Err(Error::InvalidArguments(format!("{name}: bad JSON: {e}"))),
        None => {
            return Err(Error::InvalidArguments(format!(
                "{name}: unterminated call"
            )));
        }
    };
    let used = stream.byte_offset();
    // serde_json keeps the last of duplicate keys; that is a silent alteration, so look again.
    if let Some(raw) = c.rest().get(..used)
        && let Some(key) = duplicate_key(raw)
    {
        return Err(Error::InvalidArguments(format!(
            "{name}: duplicate key '{key}'"
        )));
    }
    if !c.advance(used) {
        return Err(Error::InvalidArguments(format!(
            "{name}: bad JSON boundary"
        )));
    }
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(Error::InvalidArguments(format!(
            "{name}: arguments must be a JSON object"
        ))),
    }
}

/// First duplicate object key anywhere in `raw`, which must be one JSON value serde_json has
/// already accepted (so the scan can trust its structure). Keys are compared after unescaping,
/// so `"a"` and its `a` spelling are the same key.
fn duplicate_key(raw: &str) -> Option<String> {
    // One frame per open container: `Some(keys)` for an object, `None` for an array.
    let mut stack: Vec<Option<BTreeSet<String>>> = Vec::new();
    let mut expect_key = false;
    let mut chars = raw.char_indices();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '{' => {
                stack.push(Some(BTreeSet::new()));
                expect_key = true;
            }
            '[' => {
                stack.push(None);
                expect_key = false;
            }
            '}' | ']' => {
                stack.pop();
                expect_key = false;
            }
            ',' => expect_key = matches!(stack.last(), Some(Some(_))),
            ':' => expect_key = false,
            '"' => {
                let mut end = None;
                while let Some((j, c)) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => {
                            end = Some(j + 1);
                            break;
                        }
                        _ => {}
                    }
                }
                if expect_key && let Some(Some(keys)) = stack.last_mut() {
                    let literal = raw.get(i..end?)?;
                    let key: String = serde_json::from_str(literal).ok()?;
                    if !keys.insert(key.clone()) {
                        return Some(key);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Decoder for streamed model output.
///
/// Markers and JSON may be split anywhere across chunks (`"<<ca"`, `"ll name {..}>"`, `">"`).
/// The decoder buffers and decodes on [`StreamDecoder::finish`], so the result is identical to
/// [`decode_calls`] on the concatenated text, and split points can never change the outcome.
/// It does not emit text or calls before `finish` (see `README.md`, known limits).
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buf: String,
    max_bytes: usize,
    overflowed: bool,
}

impl StreamDecoder {
    /// A decoder validating against `tools`, with no size limit.
    pub fn new(tools: &[ToolDef]) -> Self {
        Self::with_limit(tools, usize::MAX)
    }

    /// A decoder that refuses to buffer more than `max_bytes` of output. The limit is the
    /// caller's to choose; this crate reads no configuration.
    pub fn with_limit(tools: &[ToolDef], max_bytes: usize) -> Self {
        Self {
            tools: tools.to_vec(),
            buf: String::new(),
            max_bytes,
            overflowed: false,
        }
    }

    /// Append one streamed chunk. Fails with [`Error::InvalidArguments`] once the total would
    /// exceed the limit, and keeps failing (including in [`StreamDecoder::finish`]) after that.
    pub fn push(&mut self, chunk: &str) -> Result<()> {
        if self.overflowed || self.buf.len().saturating_add(chunk.len()) > self.max_bytes {
            self.overflowed = true;
            return Err(self.overflow_error());
        }
        self.buf.push_str(chunk);
        Ok(())
    }

    /// Everything pushed so far.
    pub fn text(&self) -> &str {
        &self.buf
    }

    /// End of stream: decode and validate every call, with [`decode_calls`]' semantics.
    pub fn finish(self) -> Result<Decoded> {
        if self.overflowed {
            return Err(self.overflow_error());
        }
        decode_calls(&self.buf, &self.tools)
    }

    fn overflow_error(&self) -> Error {
        Error::InvalidArguments(format!("stream exceeded the {} byte limit", self.max_bytes))
    }
}
