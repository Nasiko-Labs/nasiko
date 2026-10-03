//! Incremental decoder for `<<call name {json}>>` in a model's reply.
//!
//! ```text
//! output   = *( text / call )
//! call     = "<<call" WS1 toolname *WSP ( json-object / "(" *WSP ")" ) *WSP ">>"
//!          / "<<call" WS1 toolname *WSP ">>"          ; the last two spellings mean `{}`
//! toolname = 1*64( ALPHA / DIGIT / "_" / "." / "-" )
//! text     = anything; "<<" not followed by "call" and whitespace is text
//! ```
//!
//! A state machine looks at every character exactly once, so a reply is decoded in one pass with
//! no re-scanning regardless of how it is chunked. `>>` inside a JSON string is harmless because
//! the object's end is found by tracking string/escape state and bracket depth, not by searching
//! for the closing marker. Every buffer has a limit from [`crate::limits`].
//!
//! The decoder is atomic: calls are validated as they complete but released only by
//! [`StreamDecoder::finish`], and the first error poisons the decoder so no later push or finish
//! can release anything. A literal `<<call` in prose cannot be escaped; it starts a call and, if
//! that call does not complete validly, fails the reply. This ambiguity is documented rather than
//! hidden.

use crate::call::validate_in;
use crate::catalog::Catalog;
use crate::error::{Result, ToolCompactError};
use crate::limits::{
    MAX_ARGS_BYTES, MAX_CALLS, MAX_DEPTH, MAX_NAME_LEN, MAX_RESPONSE_BYTES, MAX_TOTAL_ARGS_BYTES,
};
use crate::types::{Decoded, ToolCall, ToolDef};

const MARKER: &str = "<<call";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    /// Some prefix of `<<call` has matched; the prefix lives in `pending`.
    Marker,
    /// `<<call` matched completely; the next char decides (whitespace starts a name).
    AfterMarker,
    Name,
    AfterName,
    /// `(` seen after the name: only whitespace and `)` may follow. Equivalent to `{}`.
    EmptyParens,
    Json,
    Close,
    Close2,
}

/// Incremental decoder. Create one per reply.
pub struct StreamDecoder {
    catalog: Catalog,
    state: State,
    content: String,
    pending: String,
    name: String,
    args: String,
    depth: usize,
    in_string: bool,
    escaped: bool,
    calls: Vec<ToolCall>,
    total_args: usize,
    pushed: usize,
    failed: Option<ToolCompactError>,
}

impl StreamDecoder {
    /// Compile the catalog up front so unsupported schemas and bad names fail before any text
    /// is decoded.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        Ok(Self {
            catalog: Catalog::compile(tools)?,
            state: State::Text,
            content: String::new(),
            pending: String::new(),
            name: String::new(),
            args: String::new(),
            depth: 0,
            in_string: false,
            escaped: false,
            calls: Vec::new(),
            total_args: 0,
            pushed: 0,
            failed: None,
        })
    }

    /// Feed the next chunk of model output. The first error is sticky.
    pub fn push(&mut self, chunk: &str) -> Result<()> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.pushed = self.pushed.saturating_add(chunk.len());
        if self.pushed > MAX_RESPONSE_BYTES {
            return Err(self.fail(ToolCompactError::limit(
                "MAX_RESPONSE_BYTES",
                MAX_RESPONSE_BYTES,
            )));
        }
        for c in chunk.chars() {
            if let Err(e) = self.step(c) {
                return Err(self.fail(e));
            }
        }
        Ok(())
    }

    /// Release all validated calls, or the sticky error, or `incomplete_call` if a call is open.
    pub fn finish(mut self) -> Result<Decoded> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        match self.state {
            State::Text => {}
            State::Marker | State::AfterMarker => {
                let pending = std::mem::take(&mut self.pending);
                self.content.push_str(&pending);
            }
            _ => return Err(ToolCompactError::IncompleteCall),
        }
        Ok(Decoded {
            content: self.content,
            calls: self.calls,
        })
    }

    fn fail(&mut self, e: ToolCompactError) -> ToolCompactError {
        self.failed = Some(e.clone());
        e
    }

    fn step(&mut self, c: char) -> Result<()> {
        match self.state {
            State::Text => {
                if c == '<' {
                    self.pending.push(c);
                    self.state = State::Marker;
                } else {
                    self.content.push(c);
                }
            }
            State::Marker => {
                let expected = MARKER.chars().nth(self.pending.chars().count());
                if expected == Some(c) {
                    self.pending.push(c);
                    if self.pending == MARKER {
                        self.state = State::AfterMarker;
                    }
                } else {
                    self.mismatch(c);
                }
            }
            State::AfterMarker => {
                if c.is_whitespace() {
                    self.pending.clear();
                    self.name.clear();
                    self.state = State::Name;
                } else {
                    self.mismatch(c);
                }
            }
            State::Name => {
                if is_name_char(c) {
                    if self.name.len() >= MAX_NAME_LEN {
                        return Err(ToolCompactError::limit("MAX_NAME_LEN", MAX_NAME_LEN));
                    }
                    self.name.push(c);
                } else if c.is_whitespace() && self.name.is_empty() {
                    // Extra whitespace between the marker and the name.
                } else if c.is_whitespace() || c == '{' || c == '(' || c == '>' {
                    self.lookup_name()?;
                    self.after_name(c);
                } else {
                    return Err(ToolCompactError::malformed(format!(
                        "unexpected {c:?} in tool name"
                    )));
                }
            }
            State::AfterName => {
                if c == '{' || c == '(' || c == '>' {
                    self.after_name(c);
                } else if !c.is_whitespace() {
                    return Err(ToolCompactError::malformed(
                        "expected '{' after the tool name",
                    ));
                }
            }
            State::EmptyParens => {
                if c == ')' {
                    self.empty_args();
                    self.state = State::Close;
                } else if !c.is_whitespace() {
                    return Err(ToolCompactError::malformed(
                        "expected ')' after '(': arguments go in a JSON object",
                    ));
                }
            }
            State::Json => self.json_char(c)?,
            State::Close => {
                if c == '>' {
                    self.state = State::Close2;
                } else if !c.is_whitespace() {
                    return Err(ToolCompactError::malformed(
                        "expected '>>' after the arguments",
                    ));
                }
            }
            State::Close2 => {
                if c == '>' {
                    self.complete()?;
                } else {
                    return Err(ToolCompactError::malformed(
                        "expected '>>' after the arguments",
                    ));
                }
            }
        }
        Ok(())
    }

    /// `pending` plus `c` is not a marker prefix: release characters from the front until the
    /// remainder is a prefix of `<<call` again (bounded by the marker length).
    fn mismatch(&mut self, c: char) {
        self.pending.push(c);
        loop {
            let first = self.pending.remove(0);
            self.content.push(first);
            if self.pending.is_empty() {
                self.state = State::Text;
                return;
            }
            if self.pending == MARKER {
                self.state = State::AfterMarker;
                return;
            }
            if MARKER.starts_with(self.pending.as_str()) {
                self.state = State::Marker;
                return;
            }
        }
    }

    fn lookup_name(&mut self) -> Result<()> {
        if self.name.is_empty() {
            return Err(ToolCompactError::malformed("missing tool name"));
        }
        if self.catalog.get(&self.name).is_none() {
            return Err(ToolCompactError::UnknownTool {
                name: self.name.clone(),
            });
        }
        Ok(())
    }

    /// The character that ends the tool name decides how the arguments are spelled: `{` opens
    /// the JSON object, `(` opens the empty-parentheses spelling, `>` is the first half of a
    /// closing `>>` with no arguments at all, and whitespace defers the decision.
    fn after_name(&mut self, c: char) {
        match c {
            '{' => self.begin_json(),
            '(' => self.state = State::EmptyParens,
            '>' => {
                self.empty_args();
                self.state = State::Close2;
            }
            _ => self.state = State::AfterName,
        }
    }

    /// `<<call name>>` and `<<call name()>>` mean `<<call name {}>>`: the arguments are the
    /// empty object, which `complete` still validates against the schema (a tool with required
    /// arguments rejects it as `invalid_arguments`).
    fn empty_args(&mut self) {
        self.args.clear();
        self.args.push_str("{}");
    }

    fn begin_json(&mut self) {
        self.args.clear();
        self.args.push('{');
        self.depth = 1;
        self.in_string = false;
        self.escaped = false;
        self.state = State::Json;
    }

    fn json_char(&mut self, c: char) -> Result<()> {
        if self.args.len() + c.len_utf8() > MAX_ARGS_BYTES {
            return Err(ToolCompactError::limit("MAX_ARGS_BYTES", MAX_ARGS_BYTES));
        }
        if self.total_args + self.args.len() + c.len_utf8() > MAX_TOTAL_ARGS_BYTES {
            return Err(ToolCompactError::limit(
                "MAX_TOTAL_ARGS_BYTES",
                MAX_TOTAL_ARGS_BYTES,
            ));
        }
        self.args.push(c);
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if c == '\\' {
                self.escaped = true;
            } else if c == '"' {
                self.in_string = false;
            }
            return Ok(());
        }
        match c {
            '"' => self.in_string = true,
            '{' | '[' => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(ToolCompactError::limit("MAX_DEPTH", MAX_DEPTH));
                }
            }
            '}' | ']' => {
                self.depth -= 1;
                if self.depth == 0 {
                    self.state = State::Close;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn complete(&mut self) -> Result<()> {
        if self.calls.len() >= MAX_CALLS {
            return Err(ToolCompactError::limit("MAX_CALLS", MAX_CALLS));
        }
        let call = validate_in(&self.catalog, &self.name, &self.args)?;
        self.total_args = self.total_args.saturating_add(self.args.len());
        self.calls.push(call);
        self.args.clear();
        self.name.clear();
        self.state = State::Text;
        Ok(())
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// Decode a complete reply in one step. Equivalent to one `push` followed by `finish`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Decoded> {
    let mut decoder = StreamDecoder::new(tools)?;
    decoder.push(text)?;
    decoder.finish()
}
