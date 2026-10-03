//! Incremental decoding for streamed model output.
//!
//! Feed chunks with [`StreamDecoder::push`]; when the stream ends, [`StreamDecoder::finish`]
//! returns the decoded calls — or the first error, fail-closed. A marker split across
//! chunks is reassembled from the internal buffer.

use crate::decode::{build_schemas, find_marker_start, parse_one_marker};
use crate::schema::CompactSchema;
use crate::types::{Error, ToolCall, ToolDef};

/// Incremental `<<call …>>` decoder over streamed text.
pub struct StreamDecoder {
    pub(crate) schemas: Vec<(String, CompactSchema)>,
    /// Text after the last fully-consumed marker.
    pub(crate) buf: String,
    pub(crate) calls: Vec<ToolCall>,
    error: Option<Error>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            schemas: build_schemas(tools).unwrap_or_default(),
            buf: String::new(),
            calls: Vec::new(),
            error: None,
        }
    }

    /// Feed the next stream chunk. Complete markers are decoded eagerly; an
    /// incomplete trailing marker waits for more input. Once an error is recorded,
    /// further chunks are ignored (fail-closed).
    pub fn push(&mut self, chunk: &str) {
        if self.error.is_some() {
            return;
        }
        self.buf.push_str(chunk);
        self.drain();
    }

    /// End of stream. A dangling partial marker is [`Error::Malformed`]; trailing
    /// plain text is ignored.
    pub fn finish(mut self) -> Result<Vec<ToolCall>, Error> {
        self.drain();
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        if self.buf.contains("<<call") {
            return Err(Error::Malformed(
                "unterminated <<call marker at end of stream".to_string(),
            ));
        }
        Ok(self.calls)
    }

    /// Extract every complete marker currently buffered.
    fn drain(&mut self) {
        while self.error.is_none() {
            let Some(idx) = find_marker_start(&self.buf) else {
                // No marker start. A "<<call" prefix may still be arriving split
                // across chunks, so keep the longest trailing stretch that can
                // still become a marker start; anything else is prose that can
                // never become a marker.
                let mut keep_from = self.buf.len();
                for (i, _) in self.buf.match_indices('<') {
                    if "<<call ".starts_with(&self.buf[i..]) {
                        keep_from = i;
                        break;
                    }
                }
                self.buf.drain(..keep_from);
                break;
            };
            match parse_one_marker(&self.buf[idx..], &self.schemas) {
                Ok(Some((call, consumed))) => {
                    self.calls.push(call);
                    self.buf.drain(..idx + consumed);
                }
                Ok(None) => {
                    // Incomplete marker: drop plain text before it, wait for more.
                    self.buf.drain(..idx);
                    break;
                }
                Err(e) => {
                    self.error = Some(e);
                    break;
                }
            }
        }
    }
}
