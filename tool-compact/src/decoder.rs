//! Compact call decoding (batch + streaming).
//!
//! ```text
//! CALL ::= '<<' NAME WS+ JSON_OBJECT '>>'
//! ```
//!
//! `>>` inside JSON strings is literal. Text outside markers is ignored.
//! Fail-closed: unknown tools / invalid args return [`CompactError`], never a guessed call.

use serde_json::Value;

use crate::error::CompactError;
use crate::grammar::{CALL_CLOSE, CALL_OPEN};
use crate::types::{ToolCall, ToolDef};
use crate::validator::validate_args;

/// Decode all complete compact calls in `text` against `tools`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut dec = StreamDecoder::new(tools.to_vec());
    let _ = dec.push(text)?;
    dec.finish()
}

/// Incremental decoder — handles markers split across stream chunks.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buf: String,
    calls: Vec<ToolCall>,
    /// Once set, further pushes / finish return this error (fail closed).
    fatal: Option<CompactError>,
    finished: bool,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buf: String::new(),
            calls: Vec::new(),
            fatal: None,
            finished: false,
        }
    }

    /// Feed a stream chunk.
    ///
    /// Returns newly completed calls from this push (may be empty when still buffering).
    /// Returns `Err` as soon as a completed call fails validation.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        if let Some(err) = &self.fatal {
            return Err(err.clone());
        }
        if self.finished {
            return Err(CompactError::DecoderClosed);
        }
        let before = self.calls.len();
        self.buf.push_str(chunk);
        self.drain_complete(false)?;
        Ok(self.calls[before..].to_vec())
    }

    /// Finish the stream and return **all** accumulated calls.
    ///
    /// Incomplete trailing markers are a malformed-call error only when a `<<`
    /// open was seen; otherwise trailing text is ignored.
    pub fn finish(mut self) -> Result<Vec<ToolCall>, CompactError> {
        if let Some(err) = self.fatal {
            return Err(err);
        }
        self.finished = true;
        self.drain_complete(true)?;
        if self.buf.contains(CALL_OPEN) {
            return Err(CompactError::MalformedCall);
        }
        Ok(self.calls)
    }

    fn drain_complete(&mut self, at_end: bool) -> Result<(), CompactError> {
        loop {
            match extract_next(&self.buf, at_end)? {
                Extract::NeedMore => {
                    if !self.buf.contains(CALL_OPEN) {
                        let keep = partial_open_suffix(&self.buf);
                        if keep < self.buf.len() {
                            self.buf = self.buf[self.buf.len() - keep..].to_string();
                        }
                    }
                    return Ok(());
                }
                Extract::None => {
                    self.buf.clear();
                    return Ok(());
                }
                Extract::One {
                    call_src,
                    consumed_up_to,
                } => {
                    let call = match parse_and_validate(&call_src, &self.tools) {
                        Ok(c) => c,
                        Err(e) => {
                            self.fatal = Some(e.clone());
                            return Err(e);
                        }
                    };
                    self.calls.push(call);
                    self.buf = self.buf[consumed_up_to..].to_string();
                }
            }
        }
    }
}

enum Extract {
    NeedMore,
    None,
    One {
        call_src: String,
        consumed_up_to: usize,
    },
}

fn extract_next(buf: &str, at_end: bool) -> Result<Extract, CompactError> {
    let Some(open_at) = buf.find(CALL_OPEN) else {
        if at_end {
            return Ok(Extract::None);
        }
        let keep = partial_open_suffix(buf);
        if keep > 0 {
            return Ok(Extract::NeedMore);
        }
        return Ok(Extract::None);
    };

    let after_open = open_at + CALL_OPEN.len();
    let rest = &buf[after_open..];

    let rest_trim_start = rest
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i);
    let Some(name_start) = rest_trim_start else {
        return if at_end {
            Err(CompactError::MalformedCall)
        } else {
            Ok(Extract::NeedMore)
        };
    };
    let after_ws = &rest[name_start..];
    let name_len = after_ws
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .map(|c| c.len_utf8())
        .sum::<usize>();
    if name_len == 0 {
        return Err(CompactError::MalformedCall);
    }
    let after_name = &after_ws[name_len..];
    let json_start_rel = after_name
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i);
    let Some(json_rel) = json_start_rel else {
        return if at_end {
            Err(CompactError::MalformedCall)
        } else {
            Ok(Extract::NeedMore)
        };
    };
    let json_abs = after_open + name_start + name_len + json_rel;
    if !buf[json_abs..].starts_with('{') {
        return Err(CompactError::MalformedCall);
    }

    match scan_json_object(&buf[json_abs..]) {
        Scan::NeedMore => Ok(Extract::NeedMore),
        Scan::Error => Err(CompactError::MalformedCall),
        Scan::Done { end } => {
            let after_json = json_abs + end;
            let tail = &buf[after_json..];
            if !tail.starts_with(CALL_CLOSE) {
                if !at_end && CALL_CLOSE.starts_with(tail) {
                    return Ok(Extract::NeedMore);
                }
                return Err(CompactError::MalformedCall);
            }
            let call_end = after_json + CALL_CLOSE.len();
            Ok(Extract::One {
                call_src: buf[open_at..call_end].to_string(),
                consumed_up_to: call_end,
            })
        }
    }
}

fn partial_open_suffix(buf: &str) -> usize {
    // Suffix lengths are byte counts; skip any start index that is not a UTF-8
    // char boundary (e.g. trailing emoji / CJK) so slicing never panics.
    let max = CALL_OPEN.len().saturating_sub(1).min(buf.len());
    for len in (1..=max).rev() {
        let start = buf.len() - len;
        if !buf.is_char_boundary(start) {
            continue;
        }
        if CALL_OPEN.starts_with(&buf[start..]) {
            return len;
        }
    }
    0
}

enum Scan {
    NeedMore,
    Done { end: usize },
    Error,
}

/// Scan a JSON object with string awareness so `>>` inside strings is not a closer.
fn scan_json_object(s: &str) -> Scan {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'{') {
        return Scan::Error;
    }
    let mut depth = 0i32;
    let mut i = 0usize;
    let mut in_string = false;
    let mut escape = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Scan::Done { end: i + 1 };
                }
                if depth < 0 {
                    return Scan::Error;
                }
            }
            b']' => {
                depth -= 1;
                if depth < 0 {
                    return Scan::Error;
                }
            }
            _ => {}
        }
        i += 1;
    }
    Scan::NeedMore
}

fn parse_and_validate(call_src: &str, tools: &[ToolDef]) -> Result<ToolCall, CompactError> {
    let inner = call_src
        .strip_prefix(CALL_OPEN)
        .and_then(|s| s.strip_suffix(CALL_CLOSE))
        .ok_or(CompactError::MalformedCall)?;
    // `<<name {json}>>` — optional WS after `<<` before the name.
    let inner = inner.trim_start();
    let (name, rest) = split_name(inner)?;
    let args_str = rest.trim();
    if !args_str.starts_with('{') {
        return Err(CompactError::MalformedCall);
    }

    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or(CompactError::UnknownTool)?;

    let args: Value = serde_json::from_str(args_str).map_err(|_| CompactError::InvalidArguments)?;
    validate_args(tool.parameters.as_ref(), &args)?;

    Ok(ToolCall {
        name: name.to_string(),
        arguments: args,
    })
}

fn split_name(s: &str) -> Result<(&str, &str), CompactError> {
    let name_len = s
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .map(|c| c.len_utf8())
        .sum::<usize>();
    if name_len == 0 {
        return Err(CompactError::MalformedCall);
    }
    Ok((&s[..name_len], s[name_len..].trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
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
        ]
    }

    #[test]
    fn single_call() {
        let text = r#"<<create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Design review");
    }

    #[test]
    fn split_across_chunks() {
        let chunks = [
            "<<cre",
            "ate_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];
        let mut d = StreamDecoder::new(tools());
        for c in chunks {
            let _ = d.push(c).unwrap();
        }
        let calls = d.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["title"], "Retro");
    }

    #[test]
    fn close_inside_string() {
        let text = r#"<<send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls[0].arguments["subject"], "a >> b");
    }

    #[test]
    fn unknown_tool() {
        let err = decode_calls("<<delete_everything {}>>", &tools()).unwrap_err();
        assert_eq!(err, CompactError::UnknownTool);
    }

    #[test]
    fn invalid_arguments() {
        let text = r#"<<create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        assert_eq!(
            decode_calls(text, &tools()).unwrap_err(),
            CompactError::InvalidArguments
        );
    }

    #[test]
    fn plain_answer_no_calls() {
        assert!(
            decode_calls("The weather looks fine today.", &tools())
                .unwrap()
                .is_empty()
        );
    }

    /// Prose with trailing multibyte UTF-8 must not panic in `partial_open_suffix`
    /// (byte-sliced suffix that split a character).
    #[test]
    fn long_prose_with_emoji_no_panic_zero_calls() {
        let text = "Please create a calendar event 📅 for next week. \
A calendar event is a scheduled activity — 会议 / Termin — recorded in a \
digital or physical calendar. It typically includes a title, start time, \
and optional location. Long normal prose with no compact call markers.";
        let result = std::panic::catch_unwind(|| decode_calls(text, &tools()));
        let calls = result.expect("decode_calls must not panic on Unicode prose");
        let calls = calls.expect("decode succeeds");
        assert!(calls.is_empty(), "expected zero tool calls, got {calls:?}");
    }

    #[test]
    fn text_around_call() {
        let text = "Sure.\n<<create_calendar_event {\"title\":\"X\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>\nDone.";
        assert_eq!(decode_calls(text, &tools()).unwrap().len(), 1);
    }

    #[test]
    fn empty_chunk_ok() {
        let mut d = StreamDecoder::new(tools());
        assert!(d.push("").unwrap().is_empty());
        assert!(d.finish().unwrap().is_empty());
    }

    #[test]
    fn escaped_quotes_in_args() {
        let text = r#"<<send_email {"to":["a@b.com"],"subject":"say \"hi\"","body":"x"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls[0].arguments["subject"], "say \"hi\"");
    }

    #[test]
    fn malformed_json() {
        let text = r#"<<create_calendar_event {"title":>>"#;
        assert!(decode_calls(text, &tools()).is_err());
    }

    #[test]
    fn incomplete_call_at_finish_errors() {
        let mut d = StreamDecoder::new(tools());
        let _ = d
            .push(r#"<<create_calendar_event {"title":"test""#)
            .unwrap();
        let err = d.finish().unwrap_err();
        assert_eq!(err, CompactError::MalformedCall);
    }

    #[test]
    fn multiple_streamed_calls() {
        let chunks = [
            r#"<<send_email {"to":["a@b.com"],"subject":"s","bod"#,
            r#"y":"x"}>><<create_calendar_event {"title":"T","star"#,
            r#"t":"2026-10-05T15:00:00+05:30"}>>"#,
        ];
        let mut d = StreamDecoder::new(tools());
        for c in chunks {
            let _ = d.push(c).unwrap();
        }
        let calls = d.finish().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
        assert_eq!(calls[1].arguments["title"], "T");
    }

    #[test]
    fn escaped_backslash_in_stream() {
        // JSON: "C:\\Users\\test" → path value C:\Users\test
        let chunks = [
            r#"<<send_email {"to":["a@b.com"],"subject":"p","body":"C:\\Us"#,
            r#"ers\\test"}>>"#,
        ];
        let mut d = StreamDecoder::new(tools());
        for c in chunks {
            let _ = d.push(c).unwrap();
        }
        let calls = d.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["body"], "C:\\Users\\test");
    }
}
