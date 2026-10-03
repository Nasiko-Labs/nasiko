//! Incremental decoder for `<<call NAME {json}>>` markers.
//!
//! One char-level state machine backs both [`decode_calls`] and [`StreamDecoder`],
//! so batch and streaming results are identical for any chunking.

use serde_json::Value;

use crate::{is_name_char, validate::validate, DecodeError, ToolCall, ToolDef};

const MARKER: &str = "<<call";

/// Upper bound on the bytes buffered for one call (name + arguments).
pub const MAX_CALL_BYTES: usize = 1 << 20;

/// Output of the stream decoder.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeEvent {
    /// Plain text outside any call. Consecutive events concatenate.
    Text(String),
    /// A fully parsed and validated call.
    Call(ToolCall),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// Plain text.
    Text,
    /// Matched this many leading chars of `<<call`.
    Marker(usize),
    /// Full `<<call` seen; expecting whitespace.
    AfterMarker,
    /// Reading the tool name.
    Name,
    /// Name complete; expecting `{` or `>>`.
    BeforeArgs,
    /// Inside the JSON argument object.
    Args {
        depth: usize,
        in_string: bool,
        escaped: bool,
    },
    /// JSON object closed; expecting `>>`.
    AfterArgs,
    /// First `>` of the closing `>>` seen.
    Close,
}

/// Incremental decoder. Chunks may split the input at any character.
///
/// After any error the decoder is poisoned and keeps returning that error.
#[derive(Debug)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    state: State,
    text: String,
    name: String,
    args: String,
    call_bytes: usize,
    events: Vec<DecodeEvent>,
    failed: Option<DecodeError>,
}

impl StreamDecoder {
    /// Create a decoder that validates calls against `tools`.
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            state: State::Text,
            text: String::new(),
            name: String::new(),
            args: String::new(),
            call_bytes: 0,
            events: Vec::new(),
            failed: None,
        }
    }

    /// Feed a chunk; returns the events completed so far.
    ///
    /// Text that might be the start of a marker (e.g. a trailing `<<ca`) is held
    /// back until the next chunk decides what it is.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<DecodeEvent>, DecodeError> {
        if let Some(err) = &self.failed {
            return Err(err.clone());
        }
        for c in chunk.chars() {
            if let Err(err) = self.step(c) {
                return Err(self.fail(err));
            }
        }
        Ok(self.take_events())
    }

    /// Signal end of stream. Errors if the stream ended inside a call.
    pub fn finish(&mut self) -> Result<Vec<DecodeEvent>, DecodeError> {
        if let Some(err) = &self.failed {
            return Err(err.clone());
        }
        match self.state {
            State::Text => {}
            State::Marker(n) => self.text.push_str(&MARKER[..n]),
            _ => {
                let err = DecodeError::IncompleteStream("stream ended inside a tool call".into());
                return Err(self.fail(err));
            }
        }
        self.state = State::Text;
        Ok(self.take_events())
    }

    fn fail(&mut self, err: DecodeError) -> DecodeError {
        self.failed = Some(err.clone());
        self.events.clear();
        self.text.clear();
        err
    }

    fn take_events(&mut self) -> Vec<DecodeEvent> {
        self.flush_text();
        std::mem::take(&mut self.events)
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            self.events
                .push(DecodeEvent::Text(std::mem::take(&mut self.text)));
        }
    }

    fn step(&mut self, c: char) -> Result<(), DecodeError> {
        if !matches!(self.state, State::Text | State::Marker(_)) {
            self.call_bytes += c.len_utf8();
            if self.call_bytes > MAX_CALL_BYTES {
                return Err(DecodeError::TooLarge(MAX_CALL_BYTES));
            }
        }
        match self.state {
            State::Text => {
                if c == '<' {
                    self.state = State::Marker(1);
                } else {
                    self.text.push(c);
                }
            }
            State::Marker(n) => {
                if MARKER.as_bytes()[n] as char == c {
                    self.state = if n + 1 == MARKER.len() {
                        State::AfterMarker
                    } else {
                        State::Marker(n + 1)
                    };
                } else {
                    // Not a marker: emit the first held char, replay the rest.
                    self.state = State::Text;
                    self.text.push('<');
                    let mut replay = MARKER[1..n].to_string();
                    replay.push(c);
                    for ch in replay.chars() {
                        self.step(ch)?;
                    }
                }
            }
            State::AfterMarker => {
                if c.is_whitespace() {
                    self.name.clear();
                    self.args.clear();
                    self.state = State::Name;
                } else {
                    // `<<callx` is ordinary text.
                    self.text.push_str(MARKER);
                    self.state = State::Text;
                    self.call_bytes = 0;
                    self.step(c)?;
                }
            }
            State::Name => {
                if is_name_char(c) {
                    self.name.push(c);
                } else if c.is_whitespace() {
                    if !self.name.is_empty() {
                        self.state = State::BeforeArgs;
                    }
                } else if !self.name.is_empty() && c == '{' {
                    self.begin_args();
                } else if !self.name.is_empty() && c == '>' {
                    self.args.push_str("{}");
                    self.state = State::Close;
                } else {
                    return Err(DecodeError::Malformed(format!(
                        "unexpected `{c}` in tool name"
                    )));
                }
            }
            State::BeforeArgs => {
                if c.is_whitespace() {
                } else if c == '{' {
                    self.begin_args();
                } else if c == '>' {
                    self.args.push_str("{}");
                    self.state = State::Close;
                } else {
                    return Err(DecodeError::Malformed(format!(
                        "expected `{{` or `>>` after tool name, found `{c}`"
                    )));
                }
            }
            State::Args {
                depth,
                in_string,
                escaped,
            } => {
                self.args.push(c);
                self.state = if in_string {
                    if escaped {
                        State::Args {
                            depth,
                            in_string: true,
                            escaped: false,
                        }
                    } else if c == '\\' {
                        State::Args {
                            depth,
                            in_string: true,
                            escaped: true,
                        }
                    } else if c == '"' {
                        State::Args {
                            depth,
                            in_string: false,
                            escaped: false,
                        }
                    } else {
                        self.state
                    }
                } else {
                    match c {
                        '"' => State::Args {
                            depth,
                            in_string: true,
                            escaped: false,
                        },
                        '{' | '[' => State::Args {
                            depth: depth + 1,
                            in_string: false,
                            escaped: false,
                        },
                        '}' | ']' if depth == 1 => State::AfterArgs,
                        '}' | ']' => State::Args {
                            depth: depth - 1,
                            in_string: false,
                            escaped: false,
                        },
                        _ => self.state,
                    }
                };
            }
            State::AfterArgs => {
                if c.is_whitespace() {
                } else if c == '>' {
                    self.state = State::Close;
                } else {
                    return Err(DecodeError::Malformed(format!(
                        "expected `>>` after arguments, found `{c}`"
                    )));
                }
            }
            State::Close => {
                if c == '>' {
                    self.finish_call()?;
                } else {
                    return Err(DecodeError::Malformed(
                        "expected `>>`, found a single `>`".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn begin_args(&mut self) {
        self.args.clear();
        self.args.push('{');
        self.state = State::Args {
            depth: 1,
            in_string: false,
            escaped: false,
        };
    }

    fn finish_call(&mut self) -> Result<(), DecodeError> {
        let name = std::mem::take(&mut self.name);
        let raw = std::mem::take(&mut self.args);
        self.state = State::Text;
        self.call_bytes = 0;

        let tool = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| DecodeError::UnknownTool(name.clone()))?;
        let arguments: Value =
            serde_json::from_str(&raw).map_err(|e| DecodeError::InvalidJson(e.to_string()))?;
        let verdict = match &tool.parameters {
            Some(schema) => validate(&arguments, schema, "arguments"),
            None if arguments.as_object().map_or(false, |o| o.is_empty()) => Ok(()),
            None => Err("tool takes no arguments".to_string()),
        };
        verdict.map_err(|reason| DecodeError::InvalidArguments {
            tool: name.clone(),
            reason,
        })?;

        self.flush_text();
        self.events
            .push(DecodeEvent::Call(ToolCall { name, arguments }));
        Ok(())
    }
}

/// Decode every call in `text`. Plain answers yield an empty list.
///
/// Fails closed: any malformed or invalid call fails the whole decode.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut decoder = StreamDecoder::new(tools);
    let mut events = decoder.push(text)?;
    events.extend(decoder.finish()?);
    Ok(events
        .into_iter()
        .filter_map(|e| match e {
            DecodeEvent::Call(call) => Some(call),
            DecodeEvent::Text(_) => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tools;
    use serde_json::json;

    type Outcome = Result<(String, Vec<ToolCall>), DecodeError>;

    fn absorb(events: Vec<DecodeEvent>, text: &mut String, calls: &mut Vec<ToolCall>) {
        for e in events {
            match e {
                DecodeEvent::Text(t) => text.push_str(&t),
                DecodeEvent::Call(c) => calls.push(c),
            }
        }
    }

    fn run(chunks: &[&str]) -> Outcome {
        let mut d = StreamDecoder::new(&tools());
        let (mut text, mut calls) = (String::new(), Vec::new());
        for chunk in chunks {
            absorb(d.push(chunk)?, &mut text, &mut calls);
        }
        absorb(d.finish()?, &mut text, &mut calls);
        Ok((text, calls))
    }

    /// Whole input, every two-way split, and char-by-char must agree.
    fn assert_chunking_invariant(input: &str) -> Outcome {
        let whole = run(&[input]);
        for (i, _) in input.char_indices() {
            let (a, b) = input.split_at(i);
            assert_eq!(run(&[a, b]), whole, "split at {i} of {input:?}");
        }
        let singles: Vec<String> = input.chars().map(String::from).collect();
        let refs: Vec<&str> = singles.iter().map(String::as_str).collect();
        assert_eq!(run(&refs), whole, "char-by-char {input:?}");
        whole
    }

    #[test]
    fn single_call() {
        let (text, calls) = assert_chunking_invariant(r#"<<call ping {}>>"#).unwrap();
        assert_eq!(text, "");
        assert_eq!(
            calls,
            vec![ToolCall {
                name: "ping".into(),
                arguments: json!({})
            }]
        );
    }

    #[test]
    fn text_before_and_after_and_multiple() {
        let input = "Sure:\n<<call ping {}>>\nthen\n<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>\nDone.";
        let (text, calls) = assert_chunking_invariant(input).unwrap();
        assert_eq!(text, "Sure:\n\nthen\n\nDone.");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].arguments["title"], "A");
    }

    #[test]
    fn no_calls_keeps_text_verbatim() {
        for input in [
            "Just an answer.",
            "a < b << c",
            "<<callx",
            "x <",
            "<<<call ping {}>>",
            "<<cal",
            "",
        ] {
            let outcome = assert_chunking_invariant(input).unwrap();
            if input == "<<<call ping {}>>" {
                assert_eq!(outcome.0, "<");
                assert_eq!(outcome.1.len(), 1);
            } else {
                assert_eq!(outcome.0, input);
                assert!(outcome.1.is_empty());
            }
        }
    }

    #[test]
    fn tricky_strings() {
        let input = r#"<<call create_calendar_event {"title":"a >> b } { \" \\ <<call ping {}>> é ✓","start":"2026-10-04T10:00:00Z"}>>"#;
        let (_, calls) = assert_chunking_invariant(input).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].arguments["title"],
            "a >> b } { \" \\ <<call ping {}>> é ✓"
        );
    }

    #[test]
    fn arrays_and_nested_json() {
        let input = r#"<<call create_calendar_event {"title":"x","start":"2026-10-04","attendees":["a","b"]}>>"#;
        // A bare date is not a date-time: must fail closed.
        assert!(matches!(
            assert_chunking_invariant(input),
            Err(DecodeError::InvalidArguments { .. })
        ));
        let ok = r#"<<call create_calendar_event {"title":"x","start":"2026-10-04T10:00:00","attendees":["a","b"]}>>"#;
        let (_, calls) = assert_chunking_invariant(ok).unwrap();
        assert_eq!(calls[0].arguments["attendees"], json!(["a", "b"]));
    }

    #[test]
    fn name_only_call_means_empty_arguments() {
        let (_, calls) = assert_chunking_invariant("<<call ping>>").unwrap();
        assert_eq!(calls[0].arguments, json!({}));
    }

    #[test]
    fn errors_fail_closed() {
        let cases = [
            (r#"<<call nope {}>>"#, "unknown"),
            (r#"<<call ping {"a":}>>"#, "json"),
            (r#"<<call ping {"a":1}>>"#, "args"),
            (r#"<<call create_calendar_event {"title":"x"}>>"#, "args"),
            (
                r#"<<call create_calendar_event {"title":"x","start":"2026-10-04T10:00:00Z","visibility":"secret"}>>"#,
                "args",
            ),
            (r#"<<call ping {} >"#, "incomplete"),
            (r#"<<call ping {}> x"#, "malformed"),
            (r#"<<call ping [1]>>"#, "malformed"),
            (r#"<<call pi$ng {}>>"#, "malformed"),
            (r#"<<call ping {"a":"#, "incomplete"),
            ("<<call", "incomplete"),
        ];
        for (input, kind) in cases {
            let err = assert_chunking_invariant(input).unwrap_err();
            let got = match err {
                DecodeError::UnknownTool(_) => "unknown",
                DecodeError::InvalidJson(_) => "json",
                DecodeError::InvalidArguments { .. } => "args",
                DecodeError::IncompleteStream(_) => "incomplete",
                DecodeError::Malformed(_) => "malformed",
                DecodeError::TooLarge(_) => "large",
            };
            assert_eq!(got, kind, "{input}");
        }
    }

    #[test]
    fn decoder_is_poisoned_after_error() {
        let mut d = StreamDecoder::new(&tools());
        let err = d.push("<<call nope {}>>").unwrap_err();
        assert_eq!(d.push("hello").unwrap_err(), err);
        assert_eq!(d.finish().unwrap_err(), err);
    }

    #[test]
    fn oversized_call_is_rejected() {
        let mut d = StreamDecoder::new(&tools());
        d.push("<<call ping {\"a\":\"").unwrap();
        let big = "x".repeat(MAX_CALL_BYTES);
        assert_eq!(
            d.push(&big).unwrap_err(),
            DecodeError::TooLarge(MAX_CALL_BYTES)
        );
    }

    #[test]
    fn decode_calls_batch_api() {
        let calls = decode_calls("hi <<call ping {}>> <<call ping {}>>", &tools()).unwrap();
        assert_eq!(calls.len(), 2);
        assert!(decode_calls("no tools here", &tools()).unwrap().is_empty());
    }

    #[test]
    fn held_back_prefix_is_not_emitted_early() {
        let mut d = StreamDecoder::new(&tools());
        assert!(d.push("hello <<ca").unwrap() == vec![DecodeEvent::Text("hello ".into())]);
        let rest = d.push("ll ping {}>>").unwrap();
        assert!(matches!(rest.as_slice(), [DecodeEvent::Call(_)]));
    }
}
