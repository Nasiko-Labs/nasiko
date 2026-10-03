//! The one decoding path. [`crate::decode_calls`] is this decoder fed a single chunk, so the
//! streaming and non-streaming results cannot differ.

use serde_json::Value;

use crate::encode::CALL_OPEN;
use crate::error::{CompactError, Result};
use crate::schema::{self, Tool};
use crate::text;
use crate::types::{Event, ToolCall, ToolDef};
use crate::validate;

/// A call's arguments may not exceed this. Bounds what a model that never closes its call can
/// make the decoder hold.
const MAX_ARGS_BYTES: usize = 1 << 20;
const MAX_NAME_BYTES: usize = 256;

/// Incremental decoder for model output arriving in chunks.
///
/// Text is released as soon as it cannot be the start of a call marker; a call is released only
/// once it is complete and has passed validation. The first error is final: every later
/// [`push`](Self::push) and [`finish`](Self::finish) returns it again, so a caller cannot read
/// past a bad call by accident.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<Tool>,
    state: State,
    failed: Option<CompactError>,
}

#[derive(Debug, Clone)]
enum State {
    /// Outside a call. `held` is the tail that could still turn out to open one.
    Text {
        held: String,
    },
    /// Just read `<<call`.
    Opened,
    Name {
        name: String,
    },
    BeforeArgs {
        tool: usize,
    },
    /// Inside the arguments object. String and escape state are tracked so that braces, brackets
    /// and `>>` inside a JSON string are never taken for structure.
    Args {
        tool: usize,
        buf: String,
        depth: usize,
        in_string: bool,
        escaped: bool,
    },
    /// Arguments complete; waiting for `>>`.
    Closing {
        tool: usize,
        args: String,
        seen_one: bool,
    },
}

impl StreamDecoder {
    /// Fails with [`CompactError::Unsupported`] if `tools` could not have been compacted — there
    /// is then no compact output to decode.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        Ok(Self {
            tools: schema::compile(tools)?,
            state: State::Text {
                held: String::new(),
            },
            failed: None,
        })
    }

    /// Feed the next chunk. Returns what became certain because of it.
    ///
    /// On error nothing from this chunk is returned, including calls completed earlier in it.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<Event>> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let mut events = Vec::new();
        let mut text = String::new();
        for c in chunk.chars() {
            if let Err(error) = self.step(c, &mut text, &mut events) {
                self.failed = Some(error.clone());
                return Err(error);
            }
        }
        flush(&mut text, &mut events);
        Ok(events)
    }

    /// End of output. Releases any held-back text; output that stops inside a call is an error.
    pub fn finish(self) -> Result<Vec<Event>> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        match self.state {
            State::Text { held } if held.is_empty() => Ok(Vec::new()),
            State::Text { held } => Ok(vec![Event::Text(held)]),
            _ => Err(CompactError::Malformed("output ended inside a call".into())),
        }
    }

    fn step(&mut self, c: char, text: &mut String, events: &mut Vec<Event>) -> Result<()> {
        let idle = State::Text {
            held: String::new(),
        };
        self.state = match std::mem::replace(&mut self.state, idle) {
            State::Text { mut held } => {
                if held.is_empty() && !CALL_OPEN.starts_with(c) {
                    text.push(c);
                    State::Text { held }
                } else {
                    held.push(c);
                    if held == CALL_OPEN {
                        flush(text, events);
                        State::Opened
                    } else {
                        State::Text {
                            held: release_text(&held, text),
                        }
                    }
                }
            }
            State::Opened if c.is_whitespace() => State::Name {
                name: String::new(),
            },
            State::Opened => return Err(malformed("expected a space after `<<call`")),
            State::Name { mut name } => {
                if text::is_name_char(c) {
                    name.push(c);
                    if name.len() > MAX_NAME_BYTES {
                        return Err(malformed("tool name is too long"));
                    }
                    State::Name { name }
                } else if name.is_empty() {
                    if !c.is_whitespace() {
                        return Err(malformed("missing tool name"));
                    }
                    State::Name { name }
                } else {
                    // Looked up as soon as the name ends, so an unknown tool is reported without
                    // waiting for arguments that may never arrive.
                    let tool = self
                        .tools
                        .iter()
                        .position(|t| t.name == name)
                        .ok_or(CompactError::UnknownTool(name))?;
                    match c {
                        '{' => args_started(tool),
                        c if c.is_whitespace() => State::BeforeArgs { tool },
                        _ => return Err(malformed("unexpected character after the tool name")),
                    }
                }
            }
            State::BeforeArgs { tool } => match c {
                '{' => args_started(tool),
                c if c.is_whitespace() => State::BeforeArgs { tool },
                _ => return Err(malformed("arguments must be a JSON object")),
            },
            State::Args {
                tool,
                mut buf,
                mut depth,
                mut in_string,
                mut escaped,
            } => {
                buf.push(c);
                if buf.len() > MAX_ARGS_BYTES {
                    return Err(malformed("arguments are too long"));
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
                if depth == 0 {
                    State::Closing {
                        tool,
                        args: buf,
                        seen_one: false,
                    }
                } else {
                    State::Args {
                        tool,
                        buf,
                        depth,
                        in_string,
                        escaped,
                    }
                }
            }
            State::Closing {
                tool,
                args,
                seen_one,
            } => match c {
                '>' if seen_one => {
                    events.push(Event::Call(self.checked_call(tool, args)?));
                    State::Text {
                        held: String::new(),
                    }
                }
                '>' => State::Closing {
                    tool,
                    args,
                    seen_one: true,
                },
                c if c.is_whitespace() && !seen_one => State::Closing {
                    tool,
                    args,
                    seen_one,
                },
                _ => return Err(malformed("expected `>>` after the arguments")),
            },
        };
        Ok(())
    }

    /// The fail-closed gate: no call leaves the decoder without passing through here.
    fn checked_call(&self, tool: usize, args: String) -> Result<ToolCall> {
        let tool = self
            .tools
            .get(tool)
            .ok_or_else(|| malformed("decoder lost track of the tool"))?;
        let invalid = |reason: String| CompactError::InvalidArguments {
            tool: tool.name.clone(),
            reason,
        };
        let value: Value = serde_json::from_str(&args).map_err(|e| invalid(e.to_string()))?;
        validate::arguments(tool.params.as_ref(), &value).map_err(invalid)?;
        Ok(ToolCall {
            name: tool.name.clone(),
            arguments: args,
        })
    }
}

fn args_started(tool: usize) -> State {
    State::Args {
        tool,
        buf: "{".into(),
        depth: 1,
        in_string: false,
        escaped: false,
    }
}

fn malformed(reason: &str) -> CompactError {
    CompactError::Malformed(reason.into())
}

fn flush(text: &mut String, events: &mut Vec<Event>) {
    if !text.is_empty() {
        events.push(Event::Text(std::mem::take(text)));
    }
}

/// `held` is not the marker. Move everything that can no longer be part of one into `text` and
/// return the tail that still can: the longest suffix of `held` that is a prefix of the marker.
fn release_text(held: &str, text: &mut String) -> String {
    let chars: Vec<char> = held.chars().collect();
    let keep = (1..=chars.len())
        .rev()
        .find(|&k| {
            let tail: String = chars.iter().skip(chars.len() - k).collect();
            CALL_OPEN.starts_with(&tail)
        })
        .unwrap_or(0);
    let split = chars.len() - keep;
    text.extend(chars.iter().take(split));
    chars.iter().skip(split).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "echo".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"msg": {"type": "string"}},
                "required": ["msg"]
            })),
        }]
    }

    fn run(chunks: &[&str]) -> Result<Vec<Event>> {
        let mut decoder = StreamDecoder::new(&tools())?;
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(decoder.push(chunk)?);
        }
        events.extend(decoder.finish()?);
        Ok(events)
    }

    fn text_of(events: &[Event]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                Event::Call(_) => None,
            })
            .collect()
    }

    #[test]
    fn text_is_released_without_waiting_for_the_end() {
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        assert_eq!(
            decoder.push("hello ").unwrap(),
            vec![Event::Text("hello ".into())]
        );
    }

    #[test]
    fn a_possible_marker_start_is_held_back_then_released_as_text() {
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        assert_eq!(
            decoder.push("a <<ca").unwrap(),
            vec![Event::Text("a ".into())]
        );
        assert_eq!(
            decoder.push("b").unwrap(),
            vec![Event::Text("<<cab".into())]
        );
        assert_eq!(decoder.push("<").unwrap(), vec![]);
        assert_eq!(decoder.finish().unwrap(), vec![Event::Text("<".into())]);
    }

    #[test]
    fn overlapping_marker_starts_lose_no_characters() {
        let events =
            run(&["<<<call echo {\"msg\":\"x\"}>> <<c<<call echo {\"msg\":\"y\"}>>"]).unwrap();
        assert_eq!(text_of(&events), "< <<c");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Call(_)))
                .count(),
            2
        );
    }

    #[test]
    fn a_call_is_released_only_when_it_closes() {
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        assert_eq!(
            decoder.push("<<call echo {\"msg\":\"x\"}>").unwrap(),
            vec![]
        );
        assert_eq!(
            decoder.push(">").unwrap(),
            vec![Event::Call(ToolCall {
                name: "echo".into(),
                arguments: "{\"msg\":\"x\"}".into()
            })]
        );
    }

    #[test]
    fn structure_inside_json_strings_is_ignored() {
        let args = r#"{"msg":"} ] >> <<call \" \\"}"#;
        let events = run(&[&format!("<<call echo {args}>>")]).unwrap();
        assert_eq!(
            events,
            vec![Event::Call(ToolCall {
                name: "echo".into(),
                arguments: args.into()
            })]
        );
    }

    #[test]
    fn an_unknown_tool_is_reported_before_its_arguments_arrive() {
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        assert_eq!(
            decoder.push("<<call nope "),
            Err(CompactError::UnknownTool("nope".into()))
        );
    }

    #[test]
    fn the_first_error_is_final() {
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        let error = decoder.push("<<call nope {}>>").unwrap_err();
        assert_eq!(decoder.push("plain text"), Err(error.clone()));
        assert_eq!(decoder.finish(), Err(error));
    }

    #[test]
    fn a_bad_call_takes_earlier_calls_in_the_same_chunk_with_it() {
        let result = run(&["<<call echo {\"msg\":\"x\"}>> <<call echo {}>>"]);
        assert_eq!(result.unwrap_err().as_label(), "invalid_arguments");
    }

    #[test]
    fn output_ending_inside_a_call_is_an_error() {
        for cut in [
            "<<call",
            "<<call ",
            "<<call echo",
            "<<call echo {\"msg\":",
            "<<call echo {\"msg\":\"x\"}",
            "<<call echo {\"msg\":\"x\"}>",
        ] {
            assert!(
                matches!(run(&[cut]), Err(CompactError::Malformed(_))),
                "accepted {cut:?}"
            );
        }
    }

    #[test]
    fn broken_markers_are_errors() {
        for bad in [
            "<<callecho {\"msg\":\"x\"}>>",
            "<<call {\"msg\":\"x\"}>>",
            "<<call echo [\"x\"]>>",
            "<<call echo \"x\">>",
            "<<call echo {\"msg\":\"x\"} trailing>>",
            "<<call echo {\"msg\":\"x\"}> >",
            "<<call echo {\"msg\":\"x\"}]>>",
            "<<call echo {\"msg\":\"x\"]>>",
            "<<call echo {\"msg\":}>>",
            "<<call echo {'msg':'x'}>>",
        ] {
            assert!(run(&[bad]).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn oversized_arguments_are_cut_off() {
        let mut decoder = StreamDecoder::new(&tools()).unwrap();
        decoder.push("<<call echo {\"msg\":\"").unwrap();
        let filler = "x".repeat(MAX_ARGS_BYTES);
        assert!(matches!(
            decoder.push(&filler),
            Err(CompactError::Malformed(_))
        ));
    }

    #[test]
    fn whitespace_inside_the_marker_is_tolerated() {
        let events = run(&["<<call\n  echo\n{ \"msg\" : \"x\" }\n>>"]).unwrap();
        assert_eq!(events.len(), 1);
    }
}
