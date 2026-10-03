//! Incremental decoding for streamed model output.
//!
//! A model streams its reply in arbitrary chunks, so a `<<call ...>>` marker can be split
//! anywhere — across the `<<call` token itself, inside the JSON body, or right before the
//! closing `>>`. [`StreamDecoder`] buffers across pushes and emits each call the moment it is
//! complete, applying the same fail-closed validation as [`crate::decode_calls`].

use crate::decode::{next_marker, validate_call, Marker};
use crate::{Result, ToolCall, ToolDef};

const MARKER_OPEN: &str = "<<call";

/// Stateful, incremental tool-call decoder. Feed it chunks with [`StreamDecoder::push`]; each
/// call returns the tool calls (or errors) that became complete with that chunk. Holds its own
/// copy of the tool schemas so it can validate without borrowing the caller's set.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
}

impl StreamDecoder {
    /// Create a decoder that validates against `tools`.
    pub fn new(tools: &[ToolDef]) -> Self {
        StreamDecoder {
            tools: tools.to_vec(),
            buffer: String::new(),
        }
    }

    /// Feed one streamed chunk; returns every call that completed within the accumulated
    /// buffer. An invalid completed call yields `Err` (fail-closed) in the returned vec; a
    /// still-incomplete marker is retained for the next push.
    pub fn push(&mut self, chunk: &str) -> Vec<Result<ToolCall>> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();
        loop {
            match next_marker(&self.buffer) {
                Marker::Call { end, name, args_text } => {
                    out.push(validate_call(&name, &args_text, &self.tools));
                    self.buffer.drain(..end);
                }
                Marker::Malformed { end, detail } => {
                    out.push(Err(crate::CompactError::MalformedMarker(detail)));
                    self.buffer.drain(..end);
                }
                Marker::Incomplete => break,
                Marker::None => {
                    // No marker remains. Keep only a trailing fragment that could still grow
                    // into `<<call`; discard the rest so the buffer stays bounded.
                    let keep = longest_marker_prefix_suffix(&self.buffer);
                    let drop_to = self.buffer.len() - keep;
                    self.buffer.drain(..drop_to);
                    break;
                }
            }
        }
        out
    }

    /// Bytes currently buffered (an unfinished marker or a partial `<<call` fragment). Exposed
    /// for tests and diagnostics; a non-empty value at end-of-stream means a truncated marker.
    pub fn pending(&self) -> &str {
        &self.buffer
    }
}

/// Length of the longest suffix of `buf` that is a proper prefix of `<<call` — the only tail
/// worth keeping once no complete marker remains.
fn longest_marker_prefix_suffix(buf: &str) -> usize {
    let max = MARKER_OPEN.len() - 1;
    let start = buf.len().saturating_sub(max);
    for i in start..buf.len() {
        let tail = &buf[i..];
        if MARKER_OPEN.starts_with(tail) {
            return tail.len();
        }
    }
    0
}
