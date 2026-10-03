//! Compact call decoding (batch + streaming).
//!
//! # Grammar
//!
//! ```text
//! call        := '<<call' WS+ NAME WS+ JSON_OBJECT '>>'
//! NAME        := [A-Za-z_][A-Za-z0-9_]*
//! JSON_OBJECT := a single JSON object; `>>` inside JSON strings is literal
//! WS          := space | tab
//! ```
//!
//! Text outside markers is ignored. Multiple calls may appear. Empty / no-marker
//! text yields an empty call list (a plain answer).
//!
//! Fail-closed: unknown tool names and schema-invalid arguments return
//! [`CompactError`] and never a guessed call.

use serde_json::Value;

use crate::schema::validate_args;
use crate::types::{CompactError, ToolCall, ToolDef};

const OPEN: &str = "<<call";
const CLOSE: &str = ">>";

/// Decode all complete compact calls in `text` against `tools`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut dec = StreamDecoder::new(tools.to_vec());
    dec.push(text)?;
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

    /// Feed a stream chunk. Returns `Err` as soon as a completed call fails validation.
    pub fn push(&mut self, chunk: &str) -> Result<(), CompactError> {
        if let Some(err) = &self.fatal {
            return Err(err.clone());
        }
        if self.finished {
            return Err(CompactError::DecoderClosed);
        }
        self.buf.push_str(chunk);
        self.drain_complete(false)
    }

    /// Finish the stream. Incomplete trailing markers are a malformed-call error
    /// only when a `<<call` open was seen; otherwise trailing text is ignored.
    pub fn finish(mut self) -> Result<Vec<ToolCall>, CompactError> {
        if let Some(err) = self.fatal {
            return Err(err);
        }
        self.finished = true;
        self.drain_complete(true)?;
        // Leftover open marker without a complete call.
        if self.buf.contains(OPEN) {
            return Err(CompactError::MalformedCall);
        }
        Ok(self.calls)
    }

    fn drain_complete(&mut self, at_end: bool) -> Result<(), CompactError> {
        loop {
            match extract_next(&self.buf, at_end)? {
                Extract::NeedMore => {
                    // Drop leading junk that cannot start `<<call`, keep a partial prefix.
                    if !self.buf.contains(OPEN) {
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

/// Find the next complete `<<call ...>>` in `buf`.
fn extract_next(buf: &str, at_end: bool) -> Result<Extract, CompactError> {
    let Some(open_at) = buf.find(OPEN) else {
        // No marker — discard buffer unless a partial prefix of OPEN remains.
        if at_end {
            return Ok(Extract::None);
        }
        let keep = partial_open_suffix(buf);
        if keep == buf.len() {
            return Ok(Extract::NeedMore);
        }
        // Caller clears via None only when nothing to keep; here we need a custom path.
        // Represent "trim to keep" by returning None when keep==0, else NeedMore with
        // the suffix left in place by not clearing — StreamDecoder only clears on None.
        // So: if keep > 0, NeedMore (buf unchanged); if keep == 0 and no OPEN, we can
        // drop the buffer content that cannot start OPEN.
        if keep > 0 {
            return Ok(Extract::NeedMore);
        }
        // Drop everything — nothing can start a marker.
        return Ok(Extract::None);
    };

    // Drop leading non-marker text by shifting — handled by consumed_up_to on success.
    // Scan from open_at.
    let after_open = open_at + OPEN.len();
    let rest = &buf[after_open..];

    // Need whitespace then name.
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
            if !tail.starts_with(CLOSE) {
                // Might still be streaming the closing marker.
                if !at_end && CLOSE.starts_with(tail) {
                    return Ok(Extract::NeedMore);
                }
                return Err(CompactError::MalformedCall);
            }
            let call_end = after_json + CLOSE.len();
            let call_src = buf[open_at..call_end].to_string();
            Ok(Extract::One {
                call_src,
                consumed_up_to: call_end,
            })
        }
    }
}

/// Longest suffix of `buf` that is a prefix of `OPEN`.
fn partial_open_suffix(buf: &str) -> usize {
    let max = OPEN.len().saturating_sub(1).min(buf.len());
    for len in (1..=max).rev() {
        if OPEN.starts_with(&buf[buf.len() - len..]) {
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

/// Scan a JSON object starting at `s[0] == '{'`, aware of strings so `>>` inside
/// string literals does not confuse the outer call closer (closer is checked after).
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
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Scan::Done { end: i + 1 };
                }
                if depth < 0 {
                    return Scan::Error;
                }
            }
            b'[' => depth += 1, // treat arrays as depth for brace balance inside object
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
    // call_src is `<<call NAME {json}>>`
    let inner = call_src
        .strip_prefix(OPEN)
        .and_then(|s| s.strip_suffix(CLOSE))
        .ok_or(CompactError::MalformedCall)?;
    let inner = inner.trim();
    let (name, rest) = split_name(inner)?;
    let args_str = rest.trim();
    if !args_str.starts_with('{') {
        return Err(CompactError::MalformedCall);
    }

    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or(CompactError::UnknownTool)?;

    let args: Value =
        serde_json::from_str(args_str).map_err(|_| CompactError::InvalidArguments)?;
    validate_args(tool.parameters.as_ref(), &args)?;

    let arguments =
        serde_json::to_string(&args).map_err(|_| CompactError::InvalidArguments)?;
    Ok(ToolCall {
        name: name.to_string(),
        arguments,
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
    let name = &s[..name_len];
    let rest = s[name_len..].trim_start();
    Ok((name, rest))
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
        let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
    }

    #[test]
    fn split_across_chunks() {
        let chunks = [
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];
        let mut d = StreamDecoder::new(tools());
        for c in chunks {
            d.push(c).unwrap();
        }
        let calls = d.finish().unwrap();
        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["title"], "Retro");
    }

    #[test]
    fn close_inside_string() {
        let text =
            r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["subject"], "a >> b");
    }

    #[test]
    fn unknown_tool() {
        let err = decode_calls("<<call delete_everything {}>>", &tools()).unwrap_err();
        assert_eq!(err, CompactError::UnknownTool);
    }

    #[test]
    fn invalid_arguments() {
        let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let err = decode_calls(text, &tools()).unwrap_err();
        assert_eq!(err, CompactError::InvalidArguments);
    }

    #[test]
    fn plain_answer_no_calls() {
        let calls = decode_calls("The weather looks fine today.", &tools()).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn text_around_call() {
        let text = "Sure.\n<<call create_calendar_event {\"title\":\"X\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>\nDone.";
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
    }
}
