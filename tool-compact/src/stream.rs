//! Incremental decoder: a single-pass state machine over characters.
//!
//! Accepted grammar (see `GRAMMAR.md`):
//!
//! ```text
//! call = "<<call" SP name SP json-object ">>"
//! ```
//!
//! Text outside calls is ignored. A partial opener (fewer characters than `<<call `) at the end
//! of input is plain text. Once the opener is complete, anything that does not continue the
//! grammar is an error: nothing is repaired. Memory is bounded by one call.

use serde_json::Value;

use crate::render::canonical_json;
use crate::strict_json::parse_object;
use crate::types::{DecodeError, ToolCall, ToolDef};
use crate::validate::validate_call;

const OPENER: &str = "<<call ";
/// Largest accepted argument object, in bytes.
const MAX_ARGS_BYTES: usize = 64 * 1024;
const MAX_NAME_BYTES: usize = 256;

enum State {
    /// Scanning prose; `matched` is the part of [`OPENER`] seen so far.
    Text { matched: usize },
    /// Reading the tool name up to the single space.
    Name { name: String },
    /// Reading the JSON object.
    Args {
        name: String,
        buf: String,
        depth: usize,
        in_string: bool,
        escaped: bool,
    },
    /// The object closed; expecting `>>`. `seen` counts the `>` read.
    Close {
        name: String,
        buf: String,
        seen: usize,
    },
    /// An error happened; every later call returns it and no call is ever emitted.
    Failed(DecodeError),
}

/// Feeds model output chunk by chunk; correct when a marker is split across chunks.
///
/// `push` returns the calls completed by that chunk, each exactly once. If any error occurs the
/// decoder is poisoned: that `push` returns the error (calls completed earlier in the same
/// chunk are not returned, so an error never comes with a partial result) and every later `push`
/// or `finish` returns the same error.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    state: State,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            state: State::Text { matched: 0 },
        }
    }

    /// Calls completed by this chunk.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, DecodeError> {
        if let State::Failed(err) = &self.state {
            return Err(err.clone());
        }
        let mut calls = Vec::new();
        for c in chunk.chars() {
            match self.step(c) {
                Ok(Some(call)) => calls.push(call),
                Ok(None) => {}
                Err(err) => {
                    self.state = State::Failed(err.clone());
                    return Err(err);
                }
            }
        }
        Ok(calls)
    }

    /// Errors if a call is still open.
    pub fn finish(&mut self) -> Result<Vec<ToolCall>, DecodeError> {
        match &self.state {
            State::Failed(err) => Err(err.clone()),
            State::Text { .. } => Ok(Vec::new()),
            _ => {
                let err = DecodeError::InvalidArguments("truncated call".into());
                self.state = State::Failed(err.clone());
                Err(err)
            }
        }
    }

    fn step(&mut self, c: char) -> Result<Option<ToolCall>, DecodeError> {
        let state = std::mem::replace(&mut self.state, State::Text { matched: 0 });
        match state {
            State::Text { matched } => {
                self.state = scan_text(matched, c);
                Ok(None)
            }
            State::Name { mut name } => {
                if c == ' ' {
                    if name.is_empty() {
                        return Err(invalid("missing tool name"));
                    }
                    if !self.tools.iter().any(|t| t.name == name) {
                        return Err(DecodeError::UnknownTool(name));
                    }
                    self.state = State::Args {
                        name,
                        buf: String::new(),
                        depth: 0,
                        in_string: false,
                        escaped: false,
                    };
                } else if is_name_char(c, name.is_empty()) && name.len() < MAX_NAME_BYTES {
                    name.push(c);
                    self.state = State::Name { name };
                } else {
                    return Err(invalid("malformed tool name"));
                }
                Ok(None)
            }
            State::Args {
                name,
                mut buf,
                mut depth,
                mut in_string,
                mut escaped,
            } => {
                if buf.is_empty() && c != '{' {
                    return Err(invalid(
                        "arguments must start with `{` right after the name",
                    ));
                }
                buf.push(c);
                if buf.len() > MAX_ARGS_BYTES {
                    return Err(invalid("arguments too large"));
                }
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        in_string = false;
                    }
                } else {
                    match c {
                        '"' => in_string = true,
                        '{' | '[' => depth += 1,
                        '}' | ']' => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                }
                self.state = if depth == 0 && !in_string && c == '}' {
                    State::Close { name, buf, seen: 0 }
                } else {
                    State::Args {
                        name,
                        buf,
                        depth,
                        in_string,
                        escaped,
                    }
                };
                Ok(None)
            }
            State::Close { name, buf, seen } => {
                if c != '>' {
                    return Err(invalid("a call must end with `>>` right after the object"));
                }
                if seen == 0 {
                    self.state = State::Close { name, buf, seen: 1 };
                    return Ok(None);
                }
                self.finish_call(name, &buf).map(Some)
            }
            State::Failed(err) => Err(err),
        }
    }

    fn finish_call(&self, name: String, raw: &str) -> Result<ToolCall, DecodeError> {
        let map = parse_object(raw).map_err(|e| invalid(&e))?;
        let args = Value::Object(map);
        validate_call(&name, &args, &self.tools)?;
        Ok(ToolCall {
            name,
            arguments: canonical_json(&args),
        })
    }
}

/// Advance the opener match in prose.
fn scan_text(matched: usize, c: char) -> State {
    let opener = OPENER.as_bytes();
    if c.is_ascii() && opener[matched] == c as u8 {
        let next = matched + 1;
        return if next == opener.len() {
            State::Name {
                name: String::new(),
            }
        } else {
            State::Text { matched: next }
        };
    }
    // Mismatch: keep the longest suffix of (matched prefix + c) that is still a proper opener
    // prefix, so `<<<call ` is found. Non-ASCII `c` can never be part of the opener.
    let mut seen = opener[..matched].to_vec();
    seen.push(if c.is_ascii() { c as u8 } else { 0 });
    let next = (1..opener.len())
        .rev()
        .find(|&k| k <= seen.len() && seen.ends_with(&opener[..k]))
        .unwrap_or(0);
    State::Text { matched: next }
}

fn is_name_char(c: char, first: bool) -> bool {
    if first {
        c.is_ascii_alphabetic() || c == '_'
    } else {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
    }
}

fn invalid(msg: &str) -> DecodeError {
    DecodeError::InvalidArguments(msg.to_string())
}
