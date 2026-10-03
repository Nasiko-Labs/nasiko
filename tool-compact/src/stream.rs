//! Incremental decoding of `<<call NAME {json}>>` from a streamed model reply.
//!
//! A character-level state machine: the state after each character depends only on the
//! characters consumed so far, never on where one chunk ended and the next began, so splitting a
//! reply differently cannot change the result.
//!
//! Text is released as soon as it cannot be the start of a marker. Calls are released only by
//! [`StreamDecoder::finish`]: a later malformed call voids the whole reply, so no call may have
//! escaped before the reply is known to be clean.

use std::collections::BTreeMap;

use serde_json::Map;

use crate::error::{ArgumentFault, CallFault, CompactError, UnsupportedFeature};
use crate::grammar::MARKER;
use crate::json::parse_strict;
use crate::models::{Decoded, ToolCall, ToolDef};
use crate::schema::{self, Node};
use crate::validate::validate;

/// Upper bound on one call's size, so a runaway reply cannot grow the buffer without limit.
pub const MAX_CALL_BYTES: usize = 1 << 20;

/// Upper bound on a tool name inside a marker. Generous: OpenAI's limit is 64.
const MAX_NAME_BYTES: usize = 256;

/// Decodes a model reply delivered in chunks.
///
/// ```
/// use nasiko_tool_compact::{StreamDecoder, ToolDef};
/// use serde_json::json;
///
/// let tool = ToolDef::new("ping", None, Some(json!({
///     "type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]
/// })));
/// let mut decoder = StreamDecoder::new(&[tool]).unwrap();
/// let mut text = String::new();
/// for chunk in ["On it. <<ca", "ll ping {\"n\":", "1}>", ">"] {
///     text.push_str(&decoder.push(chunk).unwrap());
/// }
/// let done = decoder.finish().unwrap();
/// text.push_str(&done.text);
/// assert_eq!(text, "On it. ");
/// assert_eq!(done.calls[0].name, "ping");
/// assert_eq!(done.calls[0].arguments_json(), r#"{"n":1}"#);
/// ```
pub struct StreamDecoder {
    tools: BTreeMap<String, Node>,
    state: State,
    /// In `Text`: the tail that could still become `<<call`.
    held: String,
    name: String,
    args: String,
    scan: JsonScan,
    calls: Vec<ToolCall>,
    failure: Option<CompactError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    /// `<<call` seen; the next character decides whether it opens a call.
    Opener,
    NameStart,
    Name,
    AfterName,
    Args,
    /// Arguments done (or omitted); `seen_first` once the first `>` arrived.
    Close {
        seen_first: bool,
    },
}

/// Tracks a JSON object's extent without parsing it: brace depth outside strings, honouring
/// escapes, so `}` or `>>` inside a string never ends the call.
#[derive(Debug, Default, Clone, Copy)]
struct JsonScan {
    depth: usize,
    in_string: bool,
    escaped: bool,
}

impl JsonScan {
    /// Feed one character; `true` once the outermost object has closed.
    fn feed(&mut self, c: char) -> bool {
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if c == '\\' {
                self.escaped = true;
            } else if c == '"' {
                self.in_string = false;
            }
            return false;
        }
        match c {
            '"' => self.in_string = true,
            '{' | '[' => self.depth += 1,
            '}' | ']' => {
                self.depth = self.depth.saturating_sub(1);
                return self.depth == 0;
            }
            _ => {}
        }
        false
    }
}

fn is_marker_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

impl StreamDecoder {
    /// A decoder for replies that may call any of `tools`.
    ///
    /// Each tool's schema is parsed for validation. A tool whose schema uses an assertion this
    /// crate cannot enforce is still accepted here; any call to it fails closed. Two tools with
    /// the same name are refused, since a call could not be attributed.
    pub fn new(tools: &[ToolDef]) -> Result<Self, CompactError> {
        let mut by_name = BTreeMap::new();
        for tool in tools {
            let root = schema::analyze(tool).root;
            if by_name.insert(tool.name.clone(), root).is_some() {
                return Err(CompactError::Unsupported {
                    tool: tool.name.clone(),
                    path: "/name".into(),
                    feature: UnsupportedFeature::DuplicateTool,
                });
            }
        }
        Ok(Self {
            tools: by_name,
            state: State::Text,
            held: String::new(),
            name: String::new(),
            args: String::new(),
            scan: JsonScan::default(),
            calls: Vec::new(),
            failure: None,
        })
    }

    /// Feed the next chunk. Returns the text that is now safe to show: everything that cannot
    /// be part of a call.
    ///
    /// The first error poisons the decoder; every later call returns the same error.
    pub fn push(&mut self, chunk: &str) -> Result<String, CompactError> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        let mut text = String::new();
        for c in chunk.chars() {
            if let Err(failure) = self.step(c, &mut text) {
                self.failure = Some(failure.clone());
                return Err(failure);
            }
        }
        Ok(text)
    }

    /// End of reply. Returns any held-back text and every call, in order — or the first error,
    /// in which case no call is returned at all.
    pub fn finish(self) -> Result<Decoded, CompactError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        match self.state {
            // `<<call` at the very end with nothing after it never opened a call.
            State::Text | State::Opener => Ok(Decoded {
                text: self.held,
                calls: self.calls,
            }),
            _ => Err(malformed(CallFault::Unterminated)),
        }
    }

    fn step(&mut self, c: char, text: &mut String) -> Result<(), CompactError> {
        match self.state {
            State::Text => self.text_char(c, text),
            State::Opener => {
                if is_marker_space(c) {
                    self.held.clear();
                    self.state = State::NameStart;
                } else if c == '{' || c == '>' {
                    return Err(malformed(CallFault::EmptyName));
                } else {
                    // `<<callback`: it was text all along.
                    text.push_str(&self.held);
                    self.held.clear();
                    self.state = State::Text;
                    self.text_char(c, text);
                }
            }
            State::NameStart => match c {
                c if is_marker_space(c) => {}
                '{' | '>' => return Err(malformed(CallFault::EmptyName)),
                c => {
                    self.name.push(c);
                    self.state = State::Name;
                }
            },
            State::Name => match c {
                c if is_marker_space(c) => {
                    self.resolve_name()?;
                    self.state = State::AfterName;
                }
                '{' => {
                    self.resolve_name()?;
                    self.start_args();
                }
                '>' => {
                    self.resolve_name()?;
                    self.state = State::Close { seen_first: true };
                }
                c => {
                    self.name.push(c);
                    if self.name.len() > MAX_NAME_BYTES {
                        return Err(malformed(CallFault::TooLarge));
                    }
                }
            },
            State::AfterName => match c {
                c if is_marker_space(c) => {}
                '{' => self.start_args(),
                '>' => self.state = State::Close { seen_first: true },
                c => return Err(malformed(CallFault::UnexpectedCharacter(c))),
            },
            State::Args => {
                self.args.push(c);
                if self.args.len() > MAX_CALL_BYTES {
                    return Err(malformed(CallFault::TooLarge));
                }
                if self.scan.feed(c) {
                    self.state = State::Close { seen_first: false };
                }
            }
            State::Close { seen_first } => match c {
                '>' if seen_first => {
                    self.complete_call()?;
                    self.state = State::Text;
                }
                '>' => self.state = State::Close { seen_first: true },
                c if !seen_first && is_marker_space(c) => {}
                _ => return Err(malformed(CallFault::MissingClose)),
            },
        }
        Ok(())
    }

    /// Release text unless it could be the start of `<<call`; keep only the longest tail that
    /// still could.
    fn text_char(&mut self, c: char, text: &mut String) {
        self.held.push(c);
        if MARKER.starts_with(self.held.as_str()) {
            if self.held.len() == MARKER.len() {
                self.state = State::Opener;
            }
            return;
        }
        let keep = (1..self.held.len())
            .rev()
            .find(|&n| {
                self.held.is_char_boundary(self.held.len() - n)
                    && self
                        .held
                        .get(self.held.len() - n..)
                        .is_some_and(|tail| MARKER.starts_with(tail))
            })
            .unwrap_or(0);
        let tail = self.held.split_off(self.held.len() - keep);
        text.push_str(&self.held);
        self.held = tail;
    }

    /// The name is complete: an unknown tool fails here, before its arguments are read, so
    /// `unknown_tool` is reported even when the arguments are malformed too.
    fn resolve_name(&mut self) -> Result<(), CompactError> {
        if self.tools.contains_key(&self.name) {
            Ok(())
        } else {
            Err(CompactError::UnknownTool {
                name: std::mem::take(&mut self.name),
            })
        }
    }

    fn start_args(&mut self) {
        self.args.clear();
        self.args.push('{');
        self.scan = JsonScan {
            depth: 1,
            ..Default::default()
        };
        self.state = State::Args;
    }

    fn complete_call(&mut self) -> Result<(), CompactError> {
        let name = std::mem::take(&mut self.name);
        let args = std::mem::take(&mut self.args);
        let invalid = |fault| CompactError::InvalidArguments {
            tool: name.clone(),
            path: String::new(),
            fault,
        };
        let value = if args.is_empty() {
            serde_json::Value::Object(Map::new())
        } else {
            parse_strict(&args).map_err(invalid)?
        };
        let root = self
            .tools
            .get(&name)
            .ok_or_else(|| CompactError::UnknownTool { name: name.clone() })?;
        validate(&name, root, &value)?;
        let serde_json::Value::Object(arguments) = value else {
            return Err(invalid(ArgumentFault::WrongType {
                expected: "object".into(),
            }));
        };
        self.calls.push(ToolCall { name, arguments });
        Ok(())
    }
}

fn malformed(fault: CallFault) -> CompactError {
    CompactError::MalformedCall { fault }
}
