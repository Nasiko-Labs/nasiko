//! Incremental decoder for compact tool-call markers across arbitrary chunk boundaries.

use crate::decode::try_parse_complete;
use crate::error::{CompactError, Result};
use crate::types::{ToolCall, ToolDef};

/// Streaming decoder: feed chunks, pull completed calls.
///
/// Marker / JSON / string boundaries need not align with chunk boundaries.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    buf: String,
    tools: Vec<ToolDef>,
    completed: Vec<ToolCall>,
    fatal: Option<CompactError>,
}

impl StreamDecoder {
    /// Create a decoder bound to the given tool schemas.
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            buf: String::new(),
            tools: tools.to_vec(),
            completed: Vec::new(),
            fatal: None,
        }
    }

    /// Append a chunk. Returns any newly completed calls from this push.
    ///
    /// After a fatal error, further pushes return that same error.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        if let Some(err) = &self.fatal {
            return Err(err.clone());
        }
        self.buf.push_str(chunk);
        let mut newly = Vec::new();
        loop {
            match try_parse_complete(&self.buf, &self.tools) {
                Ok(Some((call, end))) => {
                    // Drop consumed prefix (text + call).
                    self.buf = self.buf[end..].to_string();
                    self.completed.push(call.clone());
                    newly.push(call);
                }
                Ok(None) => break,
                Err(e) => {
                    self.fatal = Some(e.clone());
                    return Err(e);
                }
            }
        }
        // Bound buffer growth: if there is no call prefix, keep only a short suffix
        // that could still be the start of `<<call `.
        if !self.buf.contains("<<") {
            let keep = CALL_PREFIX_MAX_PARTIAL;
            if self.buf.len() > keep {
                let start = self.buf.len() - keep;
                // UTF-8 safe: scan back to char boundary.
                let start = (start..=self.buf.len())
                    .find(|i| self.buf.is_char_boundary(*i))
                    .unwrap_or(0);
                self.buf = self.buf[start..].to_string();
            }
        }
        Ok(newly)
    }

    /// Finish the stream. Returns all completed calls, or an error if a partial
    /// call marker remains or a fatal error was seen.
    pub fn finish(self) -> Result<Vec<ToolCall>> {
        if let Some(err) = self.fatal {
            return Err(err);
        }
        // Any remaining `<<call` that never completed is malformed.
        if self.buf.contains(crate::decode::CALL_PREFIX)
            || self.buf.contains("<<ca")
            || self.buf.ends_with('<')
            || self.buf.ends_with("<<")
            || self.buf.contains("<<c")
        {
            // Only error if it looks like an unfinished call marker, not random `<`.
            if looks_like_partial_call(&self.buf) {
                return Err(CompactError::MalformedCall(
                    "incomplete call marker at end of stream".into(),
                ));
            }
        }
        Ok(self.completed)
    }

    /// All calls completed so far (clone).
    pub fn calls(&self) -> &[ToolCall] {
        &self.completed
    }
}

/// Longest prefix of `<<call ` we may need to retain across chunks.
const CALL_PREFIX_MAX_PARTIAL: usize = "<<call ".len();

fn looks_like_partial_call(buf: &str) -> bool {
    const PREFIX: &str = "<<call ";
    if buf.contains(PREFIX) {
        return true;
    }
    // Check if buf ends with a prefix of `<<call `.
    for n in 1..=PREFIX.len() {
        if buf.ends_with(&PREFIX[..n]) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::ToolDef;

    fn calendar() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "create_calendar_event".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "start": { "type": "string", "format": "date-time" }
                },
                "required": ["title", "start"]
            })),
        }]
    }

    #[test]
    fn split_marker_chunks() {
        let mut d = StreamDecoder::new(&calendar());
        assert!(d.push("<<ca").unwrap().is_empty());
        assert!(
            d.push(r#"ll create_calendar_event {"title":"Ret"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            d.push(r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#)
                .unwrap()
                .is_empty()
        );
        let got = d.push(">").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].arguments["title"], json!("Retro"));
        let all = d.finish().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn plain_text_finish() {
        let mut d = StreamDecoder::new(&calendar());
        d.push("The weather is sunny today.").unwrap();
        assert!(d.finish().unwrap().is_empty());
    }
}
