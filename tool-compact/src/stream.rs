//! The chunk-safe call decoder.
//!
//! ```text
//! TEXT ──"<<call"+ws──► PRE_NAME ──name char──► NAME ──ws/'{'──► PRE_ARGS ──'{'──► ARGS
//!   ▲                                            │ unknown ⇒ UnknownTool               │ depth 0
//!   └────────────── emit Call ◄── parse + validate ◄── ">>" ── POST_ARGS ◄──────────────┘
//! ```
//!
//! * **Holdback** — text ending in a prefix of `<<call` is held until the next chunk decides.
//! * **Commit** — `<<call` followed by whitespace starts a call; from there anything that does
//!   not complete as `NAME {json} >>` is an error, never re-read as text. `<<caller`, `a << b`
//!   stay text.
//! * **String-aware** — inside the JSON, `{`, `}`, `>>` within strings are content.
//! * **Fail-closed** — the first error poisons the decoder; nothing after it is emitted.
//! * **Bare-name alias (opt-in, [`DecodeOptions::bare_tool_markers`])** — models often drop the
//!   `call` keyword and write `<<send_email {…}>>`. With the option on, `<<NAME` is also a call
//!   marker when `NAME` is *exactly* one of the given tools and is followed by `{` or whitespace.
//!   Arguments are validated exactly as before; an unknown `<<name` stays text. Off by default,
//!   so the brief's grammar (`<<call …>>` only) is what [`StreamDecoder::new`] accepts.

use serde_json::Value;

use crate::error::{ArgError, DecodeError, EncodeError};
use crate::schema::{Tool, build_tool};
use crate::types::{DecodeOptions, DescriptionPolicy, Event, ToolCall, ToolDef};
use crate::validate::validate_args;

/// Arguments larger than this are rejected (`invalid_arguments`), bounding memory.
pub const MAX_ARGS_BYTES: usize = 1 << 20;

const MARKER: &str = "<<call";
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    PreName,
    Name,
    PreArgs,
    Args,
    PostArgs,
}

/// Incremental decoder: [`feed`](Self::feed) chunks in order, then [`finish`](Self::finish).
#[derive(Debug)]
pub struct StreamDecoder {
    tools: Vec<Tool>,
    state: State,
    /// Unprocessed input (in TEXT: possibly a held-back partial marker).
    buf: String,
    name: String,
    args: String,
    depth: usize,
    in_string: bool,
    escape: bool,
    closers: u8,
    failed: bool,
    opts: DecodeOptions,
}

/// Result of looking for a bare-name alias marker (`<<NAME`) in text.
enum Alias {
    /// `<<NAME` at `at`; the name ends at byte `name_end` (followed by `{` or whitespace).
    Found { at: usize, name_end: usize },
    /// Text from `at` might still become an alias once more input arrives.
    Pending { at: usize },
}

impl StreamDecoder {
    /// A decoder that accepts calls to `tools` only. Fails if a tool's schema is outside the
    /// supported subset (its calls could not be validated).
    pub fn new(tools: &[ToolDef]) -> Result<Self, EncodeError> {
        Self::with_options(tools, DecodeOptions::default())
    }

    /// [`StreamDecoder::new`] with explicit [`DecodeOptions`].
    pub fn with_options(tools: &[ToolDef], opts: DecodeOptions) -> Result<Self, EncodeError> {
        let tools = tools
            .iter()
            .map(|t| build_tool(t, DescriptionPolicy::Verbatim))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            tools,
            state: State::Text,
            buf: String::new(),
            name: String::new(),
            args: String::new(),
            depth: 0,
            in_string: false,
            escape: false,
            closers: 0,
            failed: false,
            opts,
        })
    }

    /// Feed the next chunk; returns the events it completes (adjacent text merged).
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<Event>, DecodeError> {
        if self.failed {
            return Err(poisoned());
        }
        self.buf.push_str(chunk);
        let mut events = Vec::new();
        match self.run(&mut events, false) {
            Ok(()) => Ok(merge(events)),
            Err(e) => {
                self.failed = true;
                Err(e)
            }
        }
    }

    /// End of stream: flush held text, or fail if a call is still open.
    pub fn finish(mut self) -> Result<Vec<Event>, DecodeError> {
        if self.failed {
            return Err(poisoned());
        }
        let mut events = Vec::new();
        self.run(&mut events, true)?;
        if self.state != State::Text {
            let tool = (!self.name.is_empty()).then_some(self.name.as_str());
            return Err(DecodeError::args(tool, ArgError::Incomplete));
        }
        push_text(&mut events, std::mem::take(&mut self.buf));
        Ok(merge(events))
    }

    fn run(&mut self, events: &mut Vec<Event>, at_end: bool) -> Result<(), DecodeError> {
        loop {
            if self.state == State::Text {
                if !self.scan_text(events, at_end) {
                    return Ok(());
                }
                continue;
            }
            // Inside a call: consume char by char until the call completes or input runs out.
            let mut consumed = 0;
            let mut back_to_text = false;
            let buf = std::mem::take(&mut self.buf);
            for (i, c) in buf.char_indices() {
                consumed = i + c.len_utf8();
                if let Some(call) = self.step(c)? {
                    events.push(Event::Call(call));
                }
                if self.state == State::Text {
                    back_to_text = true;
                    break;
                }
            }
            self.buf = buf.get(consumed..).unwrap_or_default().to_string();
            if !back_to_text {
                return Ok(());
            }
        }
    }

    /// TEXT state over `buf`. Returns `true` if a call was entered (caller loops again).
    fn scan_text(&mut self, events: &mut Vec<Event>, at_end: bool) -> bool {
        let mut from = 0;
        loop {
            let rest = self.buf.get(from..).unwrap_or_default();
            let marker = rest.find(MARKER);
            if self.opts.bare_tool_markers
                && let Some(alias) = self.find_alias(rest, at_end)
            {
                let alias_at = match alias {
                    Alias::Found { at, .. } | Alias::Pending { at } => at,
                };
                if marker.is_none_or(|m| alias_at < m) {
                    let at = from + alias_at;
                    let text = self.buf.get(..at).unwrap_or_default().to_string();
                    push_text(events, text);
                    return match alias {
                        Alias::Pending { .. } => {
                            self.buf = self.buf.get(at..).unwrap_or_default().to_string();
                            false
                        }
                        Alias::Found { at: a, name_end } => {
                            self.name = rest.get(a + 2..name_end).unwrap_or_default().to_string();
                            self.buf = rest.get(name_end..).unwrap_or_default().to_string();
                            self.state = State::PreArgs;
                            true
                        }
                    };
                }
            }
            let Some(rel) = marker else {
                // No marker: emit all but a trailing partial marker (unless the stream ended).
                let hold = if at_end { 0 } else { partial_marker_len(rest) };
                let cut = self.buf.len() - hold;
                let text = self.buf.get(..cut).unwrap_or_default().to_string();
                self.buf = self.buf.get(cut..).unwrap_or_default().to_string();
                push_text(events, text);
                return false;
            };
            let at = from + rel;
            let after = at + MARKER.len();
            match self.buf.get(after..).and_then(|r| r.chars().next()) {
                None if at_end => {
                    // "<<call" at the very end of the stream is just text.
                    push_text(events, std::mem::take(&mut self.buf));
                    return false;
                }
                None => {
                    // Need one more char to know whether this is a call.
                    let text = self.buf.get(..at).unwrap_or_default().to_string();
                    self.buf = self.buf.get(at..).unwrap_or_default().to_string();
                    push_text(events, text);
                    return false;
                }
                Some(c) if c.is_whitespace() => {
                    let text = self.buf.get(..at).unwrap_or_default().to_string();
                    self.buf = self.buf.get(after..).unwrap_or_default().to_string();
                    push_text(events, text);
                    self.state = State::PreName;
                    self.name.clear();
                    return true;
                }
                Some(_) => from = after, // "<<caller": text; keep scanning after it
            }
        }
    }

    /// The first `<<NAME` alias in `s` (or a trailing fragment that could still become one).
    fn find_alias(&self, s: &str, at_end: bool) -> Option<Alias> {
        let mut from = 0;
        while let Some(rel) = s.get(from..).and_then(|r| r.find("<<")) {
            let at = from + rel;
            let tail = s.get(at + 2..).unwrap_or_default();
            let name_len: usize = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                .take(MAX_NAME + 1)
                .map(char::len_utf8)
                .sum();
            let name = tail.get(..name_len).unwrap_or_default();
            match tail.get(name_len..).and_then(|r| r.chars().next()) {
                None => {
                    if !at_end && self.tools.iter().any(|t| t.name.starts_with(name)) {
                        return Some(Alias::Pending { at });
                    }
                }
                Some(c)
                    if (c == '{' || c.is_whitespace())
                        && self.tools.iter().any(|t| t.name == name) =>
                {
                    return Some(Alias::Found {
                        at,
                        name_end: at + 2 + name_len,
                    });
                }
                Some(_) => {}
            }
            from = at + 1;
        }
        // A lone trailing '<' could start "<<NAME".
        if !at_end && s.ends_with('<') && !self.tools.is_empty() {
            return Some(Alias::Pending { at: s.len() - 1 });
        }
        None
    }

    /// Advance one char inside a call. Returns a completed call, if this char completed one.
    fn step(&mut self, c: char) -> Result<Option<ToolCall>, DecodeError> {
        match self.state {
            State::Text => Ok(None),
            State::PreName => {
                if c.is_ascii_alphabetic() || c == '_' {
                    self.name.push(c);
                    self.state = State::Name;
                } else if !c.is_whitespace() {
                    return Err(malformed(None, "expected a tool name after <<call"));
                }
                Ok(None)
            }
            State::Name => {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                    if self.name.len() >= MAX_NAME {
                        return Err(malformed(None, "tool name too long"));
                    }
                    self.name.push(c);
                    return Ok(None);
                }
                // Name complete: an unknown tool is reported now, whatever follows.
                if !self.tools.iter().any(|t| t.name == self.name) {
                    return Err(DecodeError::UnknownTool {
                        name: self.name.clone(),
                    });
                }
                if c.is_whitespace() {
                    self.state = State::PreArgs;
                } else if c == '{' {
                    self.open_args();
                } else {
                    return Err(malformed(
                        Some(&self.name),
                        "expected '{' after the tool name",
                    ));
                }
                Ok(None)
            }
            State::PreArgs => {
                if c == '{' {
                    self.open_args();
                } else if !c.is_whitespace() {
                    return Err(malformed(
                        Some(&self.name),
                        "expected '{' after the tool name",
                    ));
                }
                Ok(None)
            }
            State::Args => {
                if self.args.len() + c.len_utf8() > MAX_ARGS_BYTES {
                    return Err(DecodeError::args(Some(&self.name), ArgError::TooLarge));
                }
                self.args.push(c);
                if self.in_string {
                    if self.escape {
                        self.escape = false;
                    } else if c == '\\' {
                        self.escape = true;
                    } else if c == '"' {
                        self.in_string = false;
                    }
                } else {
                    match c {
                        '"' => self.in_string = true,
                        '{' | '[' => self.depth += 1,
                        '}' | ']' => {
                            self.depth = self.depth.saturating_sub(1);
                            if self.depth == 0 {
                                self.state = State::PostArgs;
                                self.closers = 0;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(None)
            }
            State::PostArgs => {
                if c == '>' {
                    self.closers += 1;
                    if self.closers == 2 {
                        return self.complete().map(Some);
                    }
                } else if !(c.is_whitespace() && self.closers == 0) {
                    return Err(malformed(
                        Some(&self.name),
                        "expected '>>' after the arguments",
                    ));
                }
                Ok(None)
            }
        }
    }

    fn open_args(&mut self) {
        self.args.clear();
        self.args.push('{');
        self.depth = 1;
        self.in_string = false;
        self.escape = false;
        self.state = State::Args;
    }

    /// `>>` seen: parse, validate, and emit the call verbatim.
    fn complete(&mut self) -> Result<ToolCall, DecodeError> {
        let name = std::mem::take(&mut self.name);
        let args = std::mem::take(&mut self.args);
        self.state = State::Text;
        let value: Value = serde_json::from_str(&args)
            .map_err(|e| malformed(Some(&name), &format!("invalid JSON: {e}")))?;
        let Value::Object(obj) = value else {
            return Err(DecodeError::args(Some(&name), ArgError::NotObject));
        };
        let params = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.params.as_slice())
            .unwrap_or_default();
        validate_args(&obj, params).map_err(|reason| DecodeError::args(Some(&name), reason))?;
        Ok(ToolCall {
            name,
            arguments: args,
        })
    }
}

fn malformed(tool: Option<&str>, reason: &str) -> DecodeError {
    DecodeError::args(tool, ArgError::Malformed(reason.to_string()))
}

fn poisoned() -> DecodeError {
    malformed(None, "decoder already failed")
}

/// Length of the longest suffix of `s` that is a proper prefix of `<<call`.
fn partial_marker_len(s: &str) -> usize {
    (1..MARKER.len())
        .rev()
        .find(|&k| MARKER.get(..k).is_some_and(|p| s.ends_with(p)))
        .unwrap_or(0)
}

fn push_text(events: &mut Vec<Event>, text: String) {
    if !text.is_empty() {
        events.push(Event::Text(text));
    }
}

fn merge(events: Vec<Event>) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::with_capacity(events.len());
    for e in events {
        match (out.last_mut(), e) {
            (Some(Event::Text(prev)), Event::Text(t)) => prev.push_str(&t),
            (_, e) => out.push(e),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, decode_calls};
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
                        "duration_min": {"type": "integer", "minimum": 1, "maximum": 600},
                        "attendees": {"type": "array", "items": {"type": "string"}},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
                name: "send_email".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
            ToolDef {
                name: "ping".into(),
                description: None,
                parameters: None,
            },
        ]
    }

    const CAL: &str = r#"{"title":"Retro","start":"2026-10-04T10:00:00+05:30"}"#;

    fn code(text: &str) -> &'static str {
        decode(text, &tools()).unwrap_err().code()
    }

    #[test]
    fn single_call_verbatim_arguments() {
        let text = format!("<<call create_calendar_event {CAL}>>");
        let calls = decode_calls(&text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments, CAL);
    }

    #[test]
    fn whitespace_variants_and_no_space_before_brace() {
        let spaced = r#"<<call   create_calendar_event   { "title" : "Retro", "start":"2026-10-04T10:00:00+05:30" }   >>"#;
        let calls = decode_calls(spaced, &tools()).unwrap();
        assert_eq!(
            calls[0].arguments,
            r#"{ "title" : "Retro", "start":"2026-10-04T10:00:00+05:30" }"#
        );
        let tight = format!("<<call create_calendar_event{CAL}>>");
        assert_eq!(decode_calls(&tight, &tools()).unwrap().len(), 1);
        assert_eq!(
            decode_calls("<<call ping {}>>", &tools()).unwrap()[0].arguments,
            "{}"
        );
    }

    #[test]
    fn multiple_calls_and_text_around_them() {
        let text = format!(
            "Sure.\n<<call send_email {{\"to\":[\"sam@example.com\"],\"subject\":\"s\",\"body\":\"b\"}}>>\nand\n<<call create_calendar_event {CAL}>>\nDone."
        );
        let d = decode(&text, &tools()).unwrap();
        assert_eq!(
            d.calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["send_email", "create_calendar_event"]
        );
        assert_eq!(d.text, "Sure.\n\nand\n\nDone.");
    }

    #[test]
    fn plain_text_and_lookalikes_stay_text() {
        for text in [
            "I've invited riya@example.com.",
            "a << b and c >> d",
            "<<caller>> <<callback {}>> <<call",
            "<<<call",
            "",
        ] {
            let d = decode(text, &tools()).unwrap();
            assert!(d.calls.is_empty(), "{text}");
            assert_eq!(d.text, text);
        }
    }

    #[test]
    fn calls_only_response_has_empty_text_even_in_fences() {
        let text = format!("```\n<<call create_calendar_event {CAL}>>\n```\n");
        let d = decode(&text, &tools()).unwrap();
        assert_eq!(d.calls.len(), 1);
        assert_eq!(d.text, "");
    }

    #[test]
    fn strings_with_braces_escapes_and_closers() {
        let args = r#"{"to":["sam@example.com"],"subject":"a >> b } { \" \\","body":"日本語 ✓"}"#;
        let calls = decode_calls(&format!("<<call send_email {args}>>"), &tools()).unwrap();
        assert_eq!(calls[0].arguments, args);
    }

    #[test]
    fn unknown_tool_wins_even_with_broken_json() {
        assert_eq!(code("<<call delete_everything {}>>"), "unknown_tool");
        assert_eq!(
            code("<<call delete_everything {{{ not json"),
            "unknown_tool"
        );
        assert_eq!(code("<<call delete_everything"), "invalid_arguments"); // name never ended
    }

    #[test]
    fn invalid_arguments_cases() {
        for bad in [
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
            r#"<<call create_calendar_event {"title":"x"}>>"#,
            r#"<<call create_calendar_event {"title":1,"start":"2026-10-05T15:00:00+05:30"}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":30.0}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":0}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","color":"red"}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"Monday 3pm"}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","attendees":["a",2]}>>"#,
            r#"<<call create_calendar_event {'title':'x'}>>"#,
            r#"<<call create_calendar_event {"title":"x",}>>"#,
            r#"<<call create_calendar_event({"title":"x"})>>"#,
            r#"<<call create_calendar_event ["title"]>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30"}> >"#,
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30"} trailing>>"#,
            r#"<<call 9lives {}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30"}"#,
            "<<call ",
        ] {
            assert_eq!(code(bad), "invalid_arguments", "{bad}");
        }
    }

    #[test]
    fn one_bad_call_fails_everything_and_first_error_wins() {
        let text = format!(
            "<<call create_calendar_event {CAL}>>\n<<call create_calendar_event {{\"title\":\"x\"}}>>"
        );
        assert!(decode(&text, &tools()).is_err());
        let text = "<<call ping {\"x\":1}>> <<call nope {}>>";
        assert_eq!(code(text), "invalid_arguments");
        let text = "<<call nope {}>> <<call ping {\"x\":1}>>";
        assert_eq!(code(text), "unknown_tool");
    }

    #[test]
    fn poisoned_after_error() {
        let mut d = StreamDecoder::new(&tools()).unwrap();
        assert!(d.feed("<<call nope ").is_err());
        assert!(d.feed("text").is_err());
        assert!(d.finish().is_err());
    }

    #[test]
    fn size_cap() {
        let big = format!(
            "<<call send_email {{\"to\":[],\"subject\":\"{}\",\"body\":\"b\"}}>>",
            "x".repeat(MAX_ARGS_BYTES)
        );
        let err = decode(&big, &tools()).unwrap_err();
        assert_eq!(err.code(), "invalid_arguments");
        assert!(matches!(
            err,
            DecodeError::InvalidArguments {
                reason: ArgError::TooLarge,
                ..
            }
        ));
    }

    #[test]
    fn holdback_emits_safe_prefix_immediately() {
        let mut d = StreamDecoder::new(&tools()).unwrap();
        assert_eq!(
            d.feed("see <<ca").unwrap(),
            vec![Event::Text("see ".into())]
        );
        assert_eq!(d.feed("t").unwrap(), vec![Event::Text("<<cat".into())]);
        assert_eq!(d.feed(" <<call").unwrap(), vec![Event::Text(" ".into())]);
        assert!(
            d.feed("\nping {}>>")
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::Call(_)))
        );
        assert!(d.finish().unwrap().is_empty());
    }

    fn lenient(text: &str) -> Result<crate::Decoded, DecodeError> {
        crate::decode_with(
            text,
            &tools(),
            crate::DecodeOptions {
                bare_tool_markers: true,
            },
        )
    }

    #[test]
    fn bare_name_alias_is_opt_in() {
        let text = format!("On it. <<create_calendar_event {CAL}>>");
        // Strict (the brief's grammar): plain text.
        let strict = decode(&text, &tools()).unwrap();
        assert!(strict.calls.is_empty());
        assert_eq!(strict.text, text);
        // Lenient: the same call as `<<call create_calendar_event …>>`.
        let d = lenient(&text).unwrap();
        assert_eq!(d.calls.len(), 1);
        assert_eq!(d.calls[0].arguments, CAL);
        assert_eq!(d.text, "On it. ");
        assert_eq!(lenient("<<ping{}>>").unwrap().calls.len(), 1);
        assert_eq!(lenient("<<ping   {} >>").unwrap().calls.len(), 1);
    }

    #[test]
    fn bare_name_alias_is_narrow() {
        for text in [
            "<<pings {}>>",
            "<<unknown_tool {}>>",
            "<<ping>>",
            "a << ping",
            "<<send_email_v2 {}>>",
            "<<<",
        ] {
            let d = lenient(text).unwrap();
            assert!(d.calls.is_empty(), "{text}");
            assert_eq!(d.text, text, "{text}");
        }
        // Still validated exactly like a `<<call` call.
        assert_eq!(
            lenient(r#"<<create_calendar_event {"title":1}>>"#)
                .unwrap_err()
                .code(),
            "invalid_arguments"
        );
        assert_eq!(
            lenient(r#"<<create_calendar_event {"title":"x","start":"2026-10-04T10:00:00Z"}}>>"#)
                .unwrap_err()
                .code(),
            "invalid_arguments"
        );
        // Mixed with the canonical marker.
        let mixed = format!("<<call ping {{}}>>\n<<create_calendar_event {CAL}>>");
        assert_eq!(lenient(&mixed).unwrap().calls.len(), 2);
    }

    #[test]
    fn bare_name_alias_is_chunk_invariant() {
        let opts = crate::DecodeOptions {
            bare_tool_markers: true,
        };
        let text = format!("x <<create_calendar_event {CAL}>> y <<pi <<ping {{}}>> <");
        let whole = lenient(&text).map_err(|e| e.code());
        let cuts: Vec<usize> = text.char_indices().map(|(i, _)| i).skip(1).collect();
        for &i in &cuts {
            for &j in cuts.iter().filter(|&&j| j > i) {
                let mut d = StreamDecoder::with_options(&tools(), opts).unwrap();
                let mut ev = Vec::new();
                for part in [
                    text.get(..i).unwrap(),
                    text.get(i..j).unwrap(),
                    text.get(j..).unwrap(),
                ] {
                    ev.extend(d.feed(part).unwrap());
                }
                ev.extend(d.finish().unwrap());
                let got = Ok(crate::Decoded::from_events(ev));
                assert_eq!(got, whole, "split {i},{j}");
            }
        }
    }

    #[test]
    fn decoder_over_unsupported_schema_fails_construction() {
        let t = ToolDef {
            name: "x".into(),
            description: None,
            parameters: Some(json!({"type":"object","properties":{"a":{"anyOf":[]}}})),
        };
        assert!(StreamDecoder::new(std::slice::from_ref(&t)).is_err());
        assert_eq!(decode("hi", &[t]).unwrap_err().code(), "invalid_arguments");
    }
}
