//! Incremental parser for compact tool calls: `<<call name {json args}>>`.
//!
//! [`StreamDecoder`] is a byte-level state machine over the model's output stream. It
//! accepts the stream chunk by chunk, so the marker, a tool name, a JSON argument, a
//! string, an escape or the closing `>>` may split across any number of chunks without
//! losing data. Only the *unresolved tail* is buffered: text that can no longer become
//! part of a call is emitted immediately, and a completed call is emitted and removed,
//! so memory stays bounded by the longest in-progress call rather than by the stream.
//!
//! # States
//!
//! * `Text` — outside any call. Text passes through; a `<<` is held back until it can
//!   be decided: it must grow into `<<call` + whitespace to commit to a call. A `<<`
//!   that diverges (e.g. `<<x`, `<<calls`) is re-emitted as literal text.
//! * `Name` — inside a committed call, buffering the tool name until whitespace or `{`.
//! * `BeforeArgs` — name done; skipping whitespace until the mandatory `{`.
//! * `Args` — inside the JSON arguments object, tracking brace depth and, inside
//!   strings, quote/escape state. A `>>` inside a string is content, never the close.
//! * `Close` / `CloseGt` — arguments done; optional whitespace, then `>>`.
//!
//! # Fail-closed
//!
//! A committed call that breaks the grammar is [`DecodeError::MalformedCall`]; a call
//! still open at end-of-stream is [`DecodeError::UnterminatedCall`]. Once any error
//! occurs the decoder is poisoned — every later operation returns the same error, and
//! nothing further is decoded. All structural bytes are ASCII, so every slice is taken
//! at a char boundary and multibyte characters are always just opaque data: the parser
//! cannot panic on malformed input.

use super::DecodeError;
use super::validate;
use crate::ir::chat::{FunctionCall, ToolCall, ToolDef};
use serde_json::Value;

/// The marker that opens a compact tool call.
const MARKER: &str = "<<call";

/// A tool definition reduced to what decoding and validation need.
#[derive(Debug, Clone)]
pub(super) struct ToolSchema {
    pub(super) name: String,
    pub(super) parameters: Option<Value>,
}

impl ToolSchema {
    pub(super) fn from_def(def: &ToolDef) -> Self {
        Self {
            name: def.function.name.clone(),
            parameters: def.function.parameters.clone(),
        }
    }
}

/// What one chunk decoded into. The client gets `text` as the visible content and
/// `calls` as standard tool calls — never the compact representation itself.
#[derive(Debug, Default, Clone)]
pub struct Streamed {
    /// Plain text (compact calls removed) decoded from this chunk.
    pub text: String,
    /// Fully decoded and schema-validated tool calls completed by this chunk.
    pub calls: Vec<ToolCall>,
}

/// Parser states (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    Name,
    BeforeArgs,
    Args,
    Close,
    CloseGt,
}

/// Incremental decoder for model output containing compact tool calls.
///
/// Feed chunks with [`StreamDecoder::push_chunk`]; at end of stream call
/// [`StreamDecoder::finish`], which fails closed on an incomplete call. Errors are
/// sticky: after the first [`DecodeError`] the decoder returns that same error forever,
/// and the response as a whole must be discarded.
pub struct StreamDecoder {
    /// The tool set, reduced to the names + parameter schemas decoding needs.
    schemas: Vec<ToolSchema>,
    /// Current parser state.
    state: State,
    /// Unresolved tail: held-back marker-candidate text, or the in-progress call.
    buf: String,
    /// Absolute stream offset of `buf[0]`, for error byte offsets.
    base: usize,
    /// Absolute offset of the currently committed call's opening marker.
    call_start: Option<usize>,
    /// Resume cursor within `buf` for call states, so a chunked call body is never
    /// rescanned from the start on every push (no quadratic behavior).
    pos: usize,
    /// `Args` state: brace depth inside the arguments object.
    depth: usize,
    /// `Args` state: inside a JSON string?
    in_string: bool,
    /// `Args` state: previous byte was a backslash inside a string?
    escape: bool,
    /// Name of the call being decoded, set as soon as the name terminates.
    tool: String,
    /// Raw arguments held until the complete `>>` close marker is confirmed.
    pending_arguments: Option<String>,
    /// Ids are deterministic per decoder: `call_1`, `call_2`, … in completion order.
    next_id: u64,
    /// Set on the first error; sticky forever after.
    poisoned: Option<DecodeError>,
}

impl StreamDecoder {
    /// Build a decoder against the request's tool definitions. Only the names and
    /// parameter schemas are copied; the definitions themselves are not kept.
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            schemas: tools.iter().map(ToolSchema::from_def).collect(),
            state: State::Text,
            buf: String::new(),
            base: 0,
            call_start: None,
            pos: 0,
            depth: 0,
            in_string: false,
            escape: false,
            tool: String::new(),
            pending_arguments: None,
            next_id: 1,
            poisoned: None,
        }
    }

    /// Feed the next chunk of model output.
    ///
    /// Returns the text (compact calls removed) and the calls this chunk completed.
    /// Fails on the first malformed, unknown or schema-violating call — possibly
    /// surfaced by a later chunk than the one that contained its beginning.
    pub fn push_chunk(&mut self, chunk: &str) -> Result<Streamed, DecodeError> {
        if let Some(err) = self.poisoned.clone() {
            return Err(err);
        }
        self.buf.push_str(chunk);
        let mut out = Streamed::default();
        match self.scan(&mut out) {
            Ok(()) => Ok(out),
            Err(err) => {
                self.poisoned = Some(err.clone());
                Err(err)
            }
        }
    }

    /// End of stream. Flushes held-back text, and fails closed if a call is still
    /// open: a partial call is never guessed into a `ToolCall`.
    pub fn finish(mut self) -> Result<Streamed, DecodeError> {
        if let Some(err) = self.poisoned {
            return Err(err);
        }
        match self.state {
            State::Text => {
                // After a full scan the Text buffer only ever holds a `<<call` prefix.
                // A shorter prefix is prose that merely looks like a marker; the full
                // marker with nothing after it is an incomplete call — fail closed.
                let tail = std::mem::take(&mut self.buf);
                if tail == MARKER {
                    return Err(self.unterminated("marker '<<call' is not followed by a tool name"));
                }
                Ok(Streamed {
                    text: tail,
                    calls: Vec::new(),
                })
            }
            State::Name => Err(self.unterminated("the tool name is incomplete")),
            State::BeforeArgs => {
                Err(self.unterminated("expected '{' to begin the arguments object"))
            }
            State::Args => {
                let reason = if self.in_string {
                    "the arguments contain an unterminated string"
                } else {
                    "the arguments object is not closed"
                };
                Err(self.unterminated(reason))
            }
            State::Close | State::CloseGt => Err(self.unterminated("missing closing '>>'")),
        }
    }

    /// Run the state machine until it needs more input.
    fn scan(&mut self, out: &mut Streamed) -> Result<(), DecodeError> {
        loop {
            let progressed = match self.state {
                State::Text => self.scan_text(out)?,
                State::Name => self.scan_name()?,
                State::BeforeArgs => self.scan_before_args()?,
                State::Args => self.scan_args()?,
                State::Close => self.scan_close()?,
                State::CloseGt => self.scan_close_gt(out)?,
            };
            if !progressed {
                return Ok(());
            }
        }
    }

    /// Drop the first `n` bytes of the buffer, advancing the absolute offset.
    ///
    /// Every `n` comes from an ASCII scan (`find("<<")`, structural bytes), so this
    /// can never split a multibyte character.
    fn drain(&mut self, n: usize) {
        self.buf.drain(..n);
        self.base += n;
    }

    fn malformed(&self, pos: usize, reason: &str) -> DecodeError {
        DecodeError::MalformedCall {
            offset: self.base + pos,
            reason: reason.to_string(),
        }
    }

    fn unterminated(&self, reason: &str) -> DecodeError {
        DecodeError::UnterminatedCall {
            offset: self.call_start.unwrap_or(self.base),
            reason: reason.to_string(),
        }
    }

    /// `Text` state. Returns `true` when a call was committed to.
    fn scan_text(&mut self, out: &mut Streamed) -> Result<bool, DecodeError> {
        loop {
            let Some(rel) = self.buf.find("<<") else {
                // No marker candidate: everything is final text except a trailing
                // lone `<`, which may pair with the first `<` of the next chunk.
                if self.buf.ends_with('<') {
                    let keep = self.buf.len() - 1;
                    out.text.push_str(&self.buf[..keep]);
                    self.drain(keep);
                } else {
                    out.text.push_str(&self.buf);
                    self.drain(self.buf.len());
                }
                return Ok(false);
            };
            // Everything before the candidate can never become part of a marker.
            out.text.push_str(&self.buf[..rel]);
            self.drain(rel);

            if self.buf.len() < MARKER.len() && MARKER.starts_with(self.buf.as_str()) {
                // A strict prefix of "<<call" — hold until more input decides.
                return Ok(false);
            }
            if !self.buf.starts_with(MARKER) {
                // "<<" that never becomes "<<call" is literal text. Re-emit only the
                // first two bytes: a new marker may start at the second `<`.
                out.text.push_str("<<");
                self.drain(2);
                continue;
            }
            // Full "<<call" matched; the next byte decides.
            match self.buf.as_bytes().get(MARKER.len()).copied() {
                None => return Ok(false), // "call" complete, separator unknown so far
                Some(b) if b.is_ascii_whitespace() => {
                    // Committed. The marker and its whitespace are grammar, not text.
                    self.call_start = Some(self.base);
                    self.drain(MARKER.len() + 1);
                    self.state = State::Name;
                    self.pos = 0;
                    return Ok(true);
                }
                Some(_) => {
                    // e.g. "<<calls" — not the marker; literal text.
                    out.text.push_str("<<");
                    self.drain(2);
                    continue;
                }
            }
        }
    }

    /// `Name` state. Returns `true` when the name terminated.
    fn scan_name(&mut self) -> Result<bool, DecodeError> {
        let bytes = self.buf.as_bytes();
        let mut i = 0;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && is_name_byte(bytes[i]) {
            i += 1;
        }
        if i == bytes.len() {
            // Partial name — wait for the terminator byte.
            return Ok(false);
        }
        let terminator = bytes[i];
        let name = self.buf[name_start..i].to_string();
        if name.is_empty() {
            return Err(self.malformed(i, "empty tool name"));
        }
        match terminator {
            b'{' => {
                self.tool = name;
                self.drain(i); // the `{` stays at buf[0]: it opens the arguments slice
                self.state = State::Args;
                self.pos = 1; // the `{` is classified; brace depth starts at 1
                self.depth = 1;
                self.in_string = false;
                self.escape = false;
                Ok(true)
            }
            b if b.is_ascii_whitespace() => {
                self.tool = name;
                self.drain(i + 1);
                self.state = State::BeforeArgs;
                self.pos = 0;
                Ok(true)
            }
            _ => Err(self.malformed(i, "invalid character in tool name")),
        }
    }

    /// `BeforeArgs` state. Returns `true` when the arguments object started.
    fn scan_before_args(&mut self) -> Result<bool, DecodeError> {
        loop {
            let b = match self.buf.as_bytes().get(self.pos) {
                Some(&b) => b,
                None => return Ok(false),
            };
            if b.is_ascii_whitespace() {
                self.pos += 1;
                continue;
            }
            if b == b'{' {
                self.drain(self.pos); // the `{` stays at buf[0]
                self.state = State::Args;
                self.pos = 1;
                self.depth = 1;
                self.in_string = false;
                self.escape = false;
                return Ok(true);
            }
            return Err(self.malformed(self.pos, "expected '{' to begin the arguments object"));
        }
    }

    /// `Args` state. Returns `true` when the arguments object closed.
    fn scan_args(&mut self) -> Result<bool, DecodeError> {
        loop {
            let b = match self.buf.as_bytes().get(self.pos) {
                Some(&b) => b,
                None => return Ok(false), // in_string/escape/depth persist across chunks
            };
            if self.escape {
                // Any byte may follow a backslash inside a string, including a quote:
                // the escape is what keeps the string open.
                self.escape = false;
                self.pos += 1;
                continue;
            }
            if self.in_string {
                match b {
                    b'\\' => self.escape = true,
                    b'"' => self.in_string = false,
                    _ => {}
                }
                self.pos += 1;
                continue;
            }
            match b {
                b'"' => {
                    self.in_string = true;
                    self.pos += 1;
                }
                b'{' => {
                    self.depth += 1;
                    self.pos += 1;
                }
                b'}' => {
                    self.depth -= 1;
                    self.pos += 1;
                    if self.depth == 0 {
                        self.pending_arguments = Some(self.buf[..self.pos].to_string());
                        self.drain(self.pos); // consumed through the closing `}`
                        self.state = State::Close;
                        self.pos = 0;
                        return Ok(true);
                    }
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Decode + validate arguments after the complete closing marker was confirmed.
    fn complete_call(&mut self, args_json: String, out: &mut Streamed) -> Result<(), DecodeError> {
        let tool = self.tool.clone();
        // Deterministic failure order: structural errors surface first (they are
        // positional), then unknown tool, then JSON parsing, then schema validation.
        let Some(schema) = self.schemas.iter().find(|s| s.name == tool) else {
            return Err(DecodeError::UnknownTool { tool });
        };
        let args: Value =
            serde_json::from_str(&args_json).map_err(|e| DecodeError::InvalidArguments {
                tool: tool.clone(),
                reason: format!("arguments are not valid JSON: {e}"),
            })?;
        validate::validate_arguments(schema.parameters.as_ref(), &args).map_err(|reason| {
            DecodeError::InvalidArguments {
                tool: tool.clone(),
                reason,
            }
        })?;

        let call = ToolCall {
            id: format!("call_{}", self.next_id),
            kind: "function".into(),
            function: FunctionCall {
                name: tool,
                arguments: args_json,
            },
            extra: Default::default(),
        };
        self.next_id += 1;
        out.calls.push(call);
        Ok(())
    }

    /// `Close` state: optional whitespace, then the first `>` of `>>`.
    fn scan_close(&mut self) -> Result<bool, DecodeError> {
        loop {
            let b = match self.buf.as_bytes().get(self.pos) {
                Some(&b) => b,
                None => return Ok(false),
            };
            if b.is_ascii_whitespace() {
                self.pos += 1;
                continue;
            }
            if b == b'>' {
                self.pos += 1;
                self.state = State::CloseGt;
                return Ok(true);
            }
            return Err(self.malformed(self.pos, "expected '>>' after the arguments"));
        }
    }

    /// `CloseGt` state: the second `>` completes the call.
    fn scan_close_gt(&mut self, out: &mut Streamed) -> Result<bool, DecodeError> {
        let b = match self.buf.as_bytes().get(self.pos) {
            Some(&b) => b,
            None => return Ok(false),
        };
        if b == b'>' {
            self.pos += 1;
            let Some(args_json) = self.pending_arguments.take() else {
                return Err(
                    self.malformed(self.pos - 1, "missing arguments for the completed call")
                );
            };
            self.complete_call(args_json, out)?;
            self.drain(self.pos); // consumed through the closing `>>`
            self.state = State::Text;
            self.pos = 0;
            self.call_start = None;
            return Ok(true);
        }
        Err(self.malformed(self.pos, "expected '>>' after the arguments"))
    }
}

/// Tool-name characters: `[A-Za-z0-9_-]` — the OpenAI function-name charset.
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::FunctionDef;
    use serde_json::json;

    fn schema(name: &str, parameters: Value) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: None,
                parameters: Some(parameters),
            },
            extra: Default::default(),
        }
    }

    fn tools() -> Vec<ToolDef> {
        vec![
            schema(
                "foo",
                json!({
                    "type": "object",
                    "properties": { "a": { "type": "string" } },
                    "required": ["a"],
                }),
            ),
            schema("bar", Value::Null),
        ]
    }

    /// Decode a whole text through the streaming path and return (text, calls).
    fn decode_all(text: &str) -> Result<(String, Vec<ToolCall>), DecodeError> {
        let mut decoder = StreamDecoder::new(&tools());
        let mut out = decoder.push_chunk(text)?;
        let tail = decoder.finish()?;
        out.text.push_str(&tail.text);
        out.calls.extend(tail.calls);
        Ok((out.text, out.calls))
    }

    #[test]
    fn a_trailing_angle_bracket_is_held_then_decided() {
        // "abc<" alone: the `<` may pair with the next chunk, so it is held back…
        let mut decoder = StreamDecoder::new(&tools());
        assert_eq!(decoder.push_chunk("abc<").unwrap().text, "abc");
        assert_eq!(decoder.push_chunk("<").unwrap().text, "");
        // …and it does become a marker.
        let out = decoder.push_chunk("call foo {\"a\":\"x\"}>> done").unwrap();
        assert_eq!(out.text, " done");
        assert_eq!(out.calls.len(), 1);
    }

    #[test]
    fn a_held_angle_bracket_that_never_becomes_a_marker_is_text() {
        let mut decoder = StreamDecoder::new(&tools());
        // Only the `<` is held; the text before it is final.
        assert_eq!(decoder.push_chunk("see <").unwrap().text, "see ");
        assert_eq!(decoder.finish().unwrap().text, "<");
    }

    #[test]
    fn lookalike_markers_are_literal_and_later_markers_still_decode() {
        let (text, calls) = decode_all("a << b <<calls <<call bar {\"a\":\"x\"}>> end").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "bar");
        assert_eq!(text, "a << b <<calls  end");
    }

    #[test]
    fn the_marker_may_split_anywhere() {
        for (head, tail) in [
            ("<<", "call bar {\"a\":\"x\"}>>"),
            ("<<c", "all bar {\"a\":\"x\"}>>"),
            ("<<call", " bar {\"a\":\"x\"}>>"),
            ("<<call ", "bar {\"a\":\"x\"}>>"),
            ("<<call bar", " {\"a\":\"x\"}>>"),
            ("<<call bar ", "{\"a\":\"x\"}>>"),
            ("<<call bar {", "\"a\":\"x\"}>>"),
            ("<<call bar {\"a\"", ":\"x\"}>>"),
            ("<<call bar {\"a\":\"x", "\"}>>"),
            ("<<call bar {\"a\":\"x\"}", ">>"),
            ("<<call bar {\"a\":\"x\"}>", ">"),
        ] {
            let mut decoder = StreamDecoder::new(&tools());
            decoder.push_chunk(head).unwrap();
            let out = decoder.push_chunk(tail).unwrap();
            assert_eq!(out.calls.len(), 1, "split at {head:?} | {tail:?}");
            assert_eq!(out.calls[0].function.name, "bar");
        }
    }

    #[test]
    fn an_escape_split_across_chunks_stays_inside_the_string() {
        let mut decoder = StreamDecoder::new(&tools());
        decoder.push_chunk("<<call bar {\"a\":\"x \\").unwrap();
        let out = decoder.push_chunk("\" y\"}>>").unwrap();
        assert_eq!(out.calls.len(), 1);
        assert_eq!(out.calls[0].function.arguments, "{\"a\":\"x \\\" y\"}");
    }

    #[test]
    fn close_marker_inside_a_string_is_string_content() {
        let (text, calls) = decode_all("<<call foo {\"a\":\"x >> y\"}>>").unwrap();
        assert_eq!(text, "");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.arguments, "{\"a\":\"x >> y\"}");
    }

    #[test]
    fn arguments_keep_the_models_exact_json_text() {
        let (_, calls) = decode_all("<<call foo { \"a\" : \"x\" } >>").unwrap();
        assert_eq!(calls[0].function.arguments, "{ \"a\" : \"x\" }");
    }

    #[test]
    fn calls_are_not_emitted_before_the_closing_marker_is_complete() {
        let mut decoder = StreamDecoder::new(&tools());
        assert!(
            decoder
                .push_chunk("<<call foo {\"a\":\"x\"}")
                .unwrap()
                .calls
                .is_empty()
        );
        assert!(decoder.push_chunk(">").unwrap().calls.is_empty());
        assert_eq!(
            decoder.finish().unwrap_err().category(),
            "unterminated_call"
        );

        let mut decoder = StreamDecoder::new(&tools());
        assert!(decoder.push_chunk("<<call foo {\"a\":\"x\"}>x").is_err());
    }

    #[test]
    fn ids_increment_across_calls_in_one_stream() {
        let (_, calls) =
            decode_all("<<call foo {\"a\":\"x\"}>> mid <<call bar {\"a\":\"y\"}>>").unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[1].id, "call_2");
    }

    #[test]
    fn every_open_state_fails_closed_at_finish() {
        let cases = [
            ("<<call", "marker '<<call'"),
            ("<<call foo", "tool name"),
            ("<<call foo ", "'{'"),
            ("<<call foo {", "not closed"),
            ("<<call foo {\"a\":", "not closed"),
            ("<<call foo {\"a\":\"x", "unterminated string"),
            ("<<call foo {\"a\":\"x\"} ", "'>>'"),
            ("<<call foo {\"a\":\"x\"} >", "'>>'"),
        ];
        for (text, expected) in cases {
            let mut decoder = StreamDecoder::new(&tools());
            decoder.push_chunk(text).unwrap();
            let err = decoder.finish().unwrap_err();
            assert_eq!(err.category(), "unterminated_call", "{text}");
            assert!(err.to_string().contains(expected), "{text} → {err}");
        }
    }

    #[test]
    fn malformed_committed_calls_fail_deterministically() {
        let cases = [
            ("<<call {\"a\":\"x\"}>>", "empty tool name"),
            ("<<call  {\"a\":\"x\"}>>", "empty tool name"),
            ("<<call fo.o {\"a\":\"x\"}>>", "invalid character"),
            ("<<call foo {\"a\":\"x\"} x>>", "'>>'"),
            ("<<call foo {\"a\":\"x\"} > >", "'>>'"),
        ];
        for (text, expected) in cases {
            let err = decode_all(text).unwrap_err();
            assert_eq!(err.category(), "malformed_call", "{text}");
            assert!(err.to_string().contains(expected), "{text} → {err}");
        }
    }
}
