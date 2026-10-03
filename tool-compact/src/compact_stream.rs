use crate::compact_decoder::{parse_one_call, CALL_OPEN};
use crate::error::ToolCompactError;
use crate::schema::ToolRegistry;
use crate::validator::validate_call;
use serde_json::Value;

/// Items produced while decoding a streamed model reply.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamItem {
    /// Plain assistant text that is not part of a tool call.
    Text(String),
    /// A complete, validated tool call.
    Call { name: String, arguments: Value },
    /// The stream can never produce a valid call. Decoding stops after this.
    Error(ToolCompactError),
}

/// Incremental decoder for `<<call name {json}>>`.
///
/// * A marker split across chunks (`<<ca` + `ll ...`) is held back, never leaked as text.
/// * A call is emitted only once its closing `>>` has arrived and it validates.
/// * If the stream ends inside a call, `finish` returns an error, never a guessed call.
/// * Output depends only on the concatenated input, not on how it was chunked.
pub struct StreamDecoder {
    buffer: String,
    registry: ToolRegistry,
    failed: bool,
}

impl StreamDecoder {
    pub fn new(registry: ToolRegistry) -> Self {
        Self {
            buffer: String::new(),
            registry,
            failed: false,
        }
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<StreamItem> {
        let mut out = Vec::new();
        if self.failed {
            return out;
        }
        self.buffer.push_str(chunk);

        loop {
            match self.buffer.find(CALL_OPEN) {
                Some(start) => {
                    if start > 0 {
                        let text: String = self.buffer.drain(..start).collect();
                        out.push(StreamItem::Text(text));
                    }
                    match parse_one_call(&self.buffer[CALL_OPEN.len()..]) {
                        Ok(Some((name, args, consumed))) => {
                            match validate_call(&name, &args, &self.registry) {
                                Ok(arguments) => out.push(StreamItem::Call { name, arguments }),
                                Err(e) => {
                                    self.fail();
                                    out.push(StreamItem::Error(e));
                                    return out;
                                }
                            }
                            self.buffer.drain(..CALL_OPEN.len() + consumed);
                        }
                        Ok(None) => return out, // wait for more input
                        Err(e) => {
                            self.fail();
                            out.push(StreamItem::Error(e));
                            return out;
                        }
                    }
                }
                None => {
                    let keep = partial_marker_len(&self.buffer);
                    let emit_len = self.buffer.len() - keep;
                    if emit_len > 0 {
                        let text: String = self.buffer.drain(..emit_len).collect();
                        out.push(StreamItem::Text(text));
                    }
                    return out;
                }
            }
        }
    }

    /// Call once when the stream ends.
    pub fn finish(&mut self) -> Vec<StreamItem> {
        let mut out = Vec::new();
        if self.failed {
            return out;
        }
        if self.buffer.starts_with(CALL_OPEN) {
            self.fail();
            out.push(StreamItem::Error(ToolCompactError::Malformed(
                "stream ended inside a tool call".into(),
            )));
        } else if !self.buffer.is_empty() {
            out.push(StreamItem::Text(std::mem::take(&mut self.buffer)));
        }
        out
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.failed = false;
    }

    fn fail(&mut self) {
        self.failed = true;
        self.buffer.clear();
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of the call marker.
fn partial_marker_len(s: &str) -> usize {
    let marker = CALL_OPEN.as_bytes();
    (1..marker.len())
        .rev()
        .find(|&k| k <= s.len() && s.as_bytes().ends_with(&marker[..k]))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ParameterSchema, ToolSchema, ValueType};

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(
            ToolSchema::new("create_calendar_event", "Create an event")
                .with_parameter(ParameterSchema::new("title", ValueType::String, true))
                .with_parameter(ParameterSchema::new("start", ValueType::String, true)),
        );
        r
    }

    /// Feeds chunks, then returns (all text, calls, errors).
    fn run(chunks: &[&str]) -> (String, Vec<(String, Value)>, Vec<ToolCompactError>) {
        let mut d = StreamDecoder::new(registry());
        let mut items = Vec::new();
        for c in chunks {
            items.extend(d.feed(c));
        }
        items.extend(d.finish());
        let (mut text, mut calls, mut errs) = (String::new(), Vec::new(), Vec::new());
        for it in items {
            match it {
                StreamItem::Text(t) => text.push_str(&t),
                StreamItem::Call { name, arguments } => calls.push((name, arguments)),
                StreamItem::Error(e) => errs.push(e),
            }
        }
        (text, calls, errs)
    }

    #[test]
    fn marker_split_across_chunks() {
        let (text, calls, errs) = run(&[
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ]);
        assert!(errs.is_empty());
        assert_eq!(text, "");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1["title"], "Retro");
    }

    #[test]
    fn partial_marker_is_not_leaked_as_text() {
        let mut d = StreamDecoder::new(registry());
        let first = d.feed("Hello <<ca");
        assert_eq!(first, vec![StreamItem::Text("Hello ".to_string())]);
    }

    #[test]
    fn lone_angle_brackets_are_plain_text() {
        let (text, calls, errs) = run(&["a << b ", "and 1 < 2"]);
        assert_eq!(text, "a << b and 1 < 2");
        assert!(calls.is_empty() && errs.is_empty());
    }

    #[test]
    fn one_character_at_a_time_with_text_around() {
        let full = "Sure! <<call create_calendar_event {\"title\":\"a >> b\",\"start\":\"x\"}>> Done.";
        let chars: Vec<String> = full.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chars.iter().map(|s| s.as_str()).collect();
        let (text, calls, errs) = run(&refs);
        assert!(errs.is_empty());
        assert_eq!(text, "Sure!  Done.");
        assert_eq!(calls[0].1["title"], "a >> b");
    }

    #[test]
    fn chunking_does_not_change_the_result() {
        let full = "Hi <<call create_calendar_event {\"title\":\"T\",\"start\":\"s\"}>> bye";
        let whole = run(&[full]);
        let chars: Vec<String> = full.chars().map(|c| c.to_string()).collect();
        let refs: Vec<&str> = chars.iter().map(|s| s.as_str()).collect();
        assert_eq!(whole, run(&refs));
    }

    #[test]
    fn stream_ending_inside_a_call_is_an_error() {
        let (_, calls, errs) = run(&["<<call create_calendar_event {\"title\":\"T\""]);
        assert!(calls.is_empty());
        assert!(matches!(errs[0], ToolCompactError::Malformed(_)));
    }

    #[test]
    fn unknown_tool_fails_closed() {
        let (_, calls, errs) = run(&["<<call nope {\"a\":1}>>"]);
        assert!(calls.is_empty());
        assert!(matches!(errs[0], ToolCompactError::UnknownTool(_)));
    }

    #[test]
    fn invalid_arguments_fail_closed() {
        let (_, calls, errs) = run(&["<<call create_calendar_event {\"title\":\"T\"}>>"]);
        assert!(calls.is_empty());
        assert!(matches!(errs[0], ToolCompactError::InvalidArguments { .. }));
    }
}
