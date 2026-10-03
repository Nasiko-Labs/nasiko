//! Streaming decoder for `<<call name {json}>>` markers.
//!
//! # State machine
//!
//! ```text
//! Text ──< '<<call ' prefix >──→ Name ──→ PreArgs ──→ Args ──→ Close ──→ Text
//!   ↑                                                                       │
//!   └───────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! - **Text**: scans for the `<<call ` prefix using KMP-style matching.
//! - **Name**: reads tool name (alphanumeric, `_`, `-`).
//! - **PreArgs**: skips whitespace between name and `{`.
//! - **Args**: JSON-aware scanner tracking `depth`, `in_string`, `escape`.
//!   `>>` inside a string is safe by construction (depth ≠ 0).
//! - **Close**: expects exactly `>>` after args JSON closes at depth 0.
//!
//! # Invariants
//!
//! - Final result is identical regardless of how input is split into chunks.
//! - A lone `<` or `<<` not followed by `call ` is plain text.
//! - Any invalid call makes the **whole** result an error (no partial calls).
//! - Truncated call at end of input is an error.
//! - [`StreamDecoder::push_bytes`] handles splits inside UTF-8 characters.

use crate::error::CompactError;

/// The marker prefix: `<<call ` (7 ASCII bytes).
const MARKER: &[u8] = b"<<call ";

/// KMP failure function for `<<call `.
/// `<<call ` = `[<, <, c, a, l, l, ' ']`
///   f = `[0, 1, 0, 0, 0, 0, 0]`
const FAILURE: [usize; 7] = [0, 1, 0, 0, 0, 0, 0];

/// A raw decoded call before validation.
#[derive(Debug, Clone)]
pub struct RawCall {
    pub name: String,
    pub args: String,
}

/// Incremental decoder for the compact call format.
///
/// Feed text via [`push`] or arbitrary bytes via [`push_bytes`], then call
/// [`finish`] to obtain decoded calls or an error.
pub struct StreamDecoder {
    state: State,
    calls: Vec<RawCall>,
    /// Buffer for incomplete UTF-8 sequences (used by [`push_bytes`]).
    utf8_buf: Vec<u8>,
    /// First error encountered (fail-closed: whole result is an error).
    error: Option<CompactError>,
}

enum State {
    /// Scanning for the `<<call ` prefix.
    Text,
    /// Matching KMP prefix; `pos` chars of `<<call ` matched so far.
    Prefix { pos: usize },
    /// Reading the tool name.
    Name { name: String },
    /// After name, before `{` (whitespace).
    PreArgs { name: String },
    /// Scanning JSON arguments. Tracks brace depth, string state, escape state.
    Args {
        name: String,
        args: String,
        depth: u32,
        in_string: bool,
        escape: bool,
    },
    /// After args JSON closed at depth 0, expecting `>>`.
    Close {
        name: String,
        args: String,
        got_first: bool,
    },
}

impl StreamDecoder {
    /// Create a new decoder.
    pub fn new() -> Self {
        Self {
            state: State::Text,
            calls: Vec::new(),
            utf8_buf: Vec::new(),
            error: None,
        }
    }

    /// Feed a string chunk. Can be called any number of times.
    pub fn push(&mut self, chunk: &str) {
        for c in chunk.chars() {
            self.process_char(c);
            if self.error.is_some() {
                return;
            }
        }
    }

    /// Feed arbitrary bytes, handling splits inside UTF-8 characters.
    ///
    /// Buffered incomplete sequences are flushed when enough bytes arrive.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.utf8_buf.extend_from_slice(bytes);
        let valid_up_to = match std::str::from_utf8(&self.utf8_buf) {
            Ok(s) => {
                let owned = s.to_string();
                self.utf8_buf.clear();
                self.push(&owned);
                return;
            }
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            // Safety: from_utf8 told us these bytes are valid UTF-8.
            let valid = std::str::from_utf8(&self.utf8_buf[..valid_up_to]).unwrap_or_default();
            let owned = valid.to_string();
            self.utf8_buf = self.utf8_buf[valid_up_to..].to_vec();
            self.push(&owned);
        }
    }

    /// Finalize the decoder. Returns all decoded calls, or an error if any call
    /// was malformed, truncated, or the output contained no valid close marker
    /// for an open call.
    pub fn finish(self) -> Result<Vec<RawCall>, CompactError> {
        if let Some(err) = self.error {
            return Err(err);
        }
        // Incomplete UTF-8 at end is not a call error — it's just trailing bytes.
        match self.state {
            State::Text => Ok(self.calls),
            State::Prefix { .. } => {
                // Unfinished prefix is just text (e.g. lone `<` at end).
                Ok(self.calls)
            }
            _ => Err(CompactError::MalformedOutput {
                detail: "truncated call at end of input".to_string(),
            }),
        }
    }

    /// Process one character through the state machine.
    fn process_char(&mut self, c: char) {
        // Take ownership of state to avoid borrow issues.
        let state = std::mem::replace(&mut self.state, State::Text);
        self.step(c, state);
    }

    /// Recursive step for KMP fallback. Bounded by the prefix length (max 2 levels).
    fn step(&mut self, c: char, state: State) {
        match state {
            State::Text => {
                if c == '<' {
                    self.state = State::Prefix { pos: 1 };
                } else {
                    self.state = State::Text;
                }
            }

            State::Prefix { pos } => {
                if pos < MARKER.len() && c as u32 <= 127 && c as u8 == MARKER[pos] {
                    let new_pos = pos + 1;
                    if new_pos == MARKER.len() {
                        self.state = State::Name {
                            name: String::new(),
                        };
                    } else {
                        self.state = State::Prefix { pos: new_pos };
                    }
                } else if pos > 0 {
                    // KMP fallback: try shorter prefix.
                    let fallback_pos = FAILURE[pos - 1];
                    let new_state = if fallback_pos == 0 {
                        State::Text
                    } else {
                        State::Prefix { pos: fallback_pos }
                    };
                    // Re-process `c` at the new state.
                    self.step(c, new_state);
                } else {
                    self.state = State::Text;
                }
            }

            State::Name { mut name } => {
                if c == '{' {
                    if name.is_empty() {
                        self.error = Some(CompactError::MalformedOutput {
                            detail: "empty tool name in call marker".to_string(),
                        });
                        self.state = State::Text;
                    } else {
                        self.state = State::Args {
                            name,
                            args: "{".to_string(),
                            depth: 1,
                            in_string: false,
                            escape: false,
                        };
                    }
                } else if c == ' ' || c == '\t' {
                    if name.is_empty() {
                        // Skip extra whitespace after "<<call ".
                        self.state = State::Name { name };
                    } else {
                        self.state = State::PreArgs { name };
                    }
                } else if c.is_alphanumeric() || c == '_' || c == '-' {
                    name.push(c);
                    self.state = State::Name { name };
                } else {
                    self.error = Some(CompactError::MalformedOutput {
                        detail: format!("unexpected character '{c}' in tool name"),
                    });
                    self.state = State::Text;
                }
            }

            State::PreArgs { name } => {
                if c == '{' {
                    self.state = State::Args {
                        name,
                        args: "{".to_string(),
                        depth: 1,
                        in_string: false,
                        escape: false,
                    };
                } else if c.is_whitespace() {
                    self.state = State::PreArgs { name };
                } else {
                    self.error = Some(CompactError::MalformedOutput {
                        detail: format!("expected '{{' after tool name, got '{c}'"),
                    });
                    self.state = State::Text;
                }
            }

            State::Args {
                name,
                mut args,
                mut depth,
                mut in_string,
                mut escape,
            } => {
                args.push(c);

                if escape {
                    escape = false;
                } else if in_string {
                    match c {
                        '\\' => escape = true,
                        '"' => in_string = false,
                        _ => {}
                    }
                } else {
                    match c {
                        '"' => in_string = true,
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                self.state = State::Close {
                                    name,
                                    args,
                                    got_first: false,
                                };
                                return;
                            }
                        }
                        _ => {}
                    }
                }

                self.state = State::Args {
                    name,
                    args,
                    depth,
                    in_string,
                    escape,
                };
            }

            State::Close {
                name,
                args,
                got_first,
            } => {
                if !got_first {
                    if c == '>' {
                        self.state = State::Close {
                            name,
                            args,
                            got_first: true,
                        };
                    } else {
                        self.error = Some(CompactError::MalformedOutput {
                            detail: format!("expected '>>' after arguments, got '{c}'"),
                        });
                        self.state = State::Text;
                    }
                } else if c == '>' {
                    // Call complete!
                    self.calls.push(RawCall { name, args });
                    self.state = State::Text;
                } else {
                    self.error = Some(CompactError::MalformedOutput {
                        detail: format!("expected second '>' to close marker, got '{c}'"),
                    });
                    self.state = State::Text;
                }
            }
        }
    }
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}
