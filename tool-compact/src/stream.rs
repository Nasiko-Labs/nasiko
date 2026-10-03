//! Incremental decoder. A marker split across chunks is held until the closer arrives.

use crate::decode;
use crate::grammar::{self, Taken};
use crate::types::{CompactError, ToolCall, ToolDef};

/// Feeds model text one chunk at a time and emits a call only when it is complete
/// and valid.
pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    buf: String,
    emitted_chars: usize,
}

impl<'a> StreamDecoder<'a> {
    pub fn new(tools: &'a [ToolDef]) -> Self {
        Self {
            tools,
            buf: String::new(),
            emitted_chars: 0,
        }
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        self.buf.push_str(chunk);
        let mut emitted = Vec::new();
        loop {
            let pending: String = self.buf.chars().skip(self.emitted_chars).collect();
            let Some((prefix, after_marker)) = pending.split_once("<<call ") else {
                break;
            };
            match grammar::take_call(after_marker)? {
                Taken::Incomplete => break,
                Taken::Ready(raw, consumed) => {
                    emitted.push(decode::accept_call(raw, self.tools)?);
                    self.emitted_chars +=
                        prefix.chars().count() + "<<call ".chars().count() + consumed;
                }
            }
        }
        Ok(emitted)
    }

    pub fn finish(&mut self) -> Result<Vec<ToolCall>, CompactError> {
        let pending: String = self.buf.chars().skip(self.emitted_chars).collect();
        if pending.contains("<<call ") {
            return Err(grammar::malformed(""));
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{calendar, design_review};
    use crate::types::{ArgumentFault, CompactError};
    use serde_json::Value;

    #[test]
    fn marker_split_across_chunks_emits_one_call_at_the_end() {
        let tools = [calendar()];
        let mut dec = StreamDecoder::new(&tools);
        assert!(dec.push("<<ca").unwrap().is_empty());
        assert!(
            dec.push(r#"ll create_calendar_event {"title":"Ret"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            dec.push(r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#)
                .unwrap()
                .is_empty()
        );
        let calls = dec.push(">").unwrap();
        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["title"], "Retro");
    }

    #[test]
    fn one_chunk_matches_decode_calls() {
        let tools = [calendar()];
        let mut dec = StreamDecoder::new(&tools);
        let streamed = dec.push(design_review()).unwrap();
        let direct = crate::decode_calls(design_review(), &[calendar()]).unwrap();
        assert_eq!(streamed, direct);
    }

    #[test]
    fn bad_enum_across_chunks_is_an_error_with_no_call() {
        let tools = [calendar()];
        let mut dec = StreamDecoder::new(&tools);
        dec.push(r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","visibility":"sec"#).unwrap();
        let err = dec
            .push(r#"ret"}>>"#)
            .unwrap_err();
        assert!(matches!(
            err,
            CompactError::InvalidArguments { reason: ArgumentFault::BadEnum { .. }, .. }
        ));
    }
}
