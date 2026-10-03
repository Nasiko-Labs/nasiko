use crate::decode::{CALL_OPEN, Parsed, build_call, parse_call_at};
use crate::error::CompactError;
use crate::types::{ToolCall, ToolDef};

/// Incremental decoder for streamed model output.
///
/// Markers may be split across chunk boundaries in any way: the decoder retains
/// any tail that could still become a call (a prefix of `<<call `, an unfinished
/// JSON object, or a lone `>`), and flushes everything else as plain text.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buf: String,
    calls: Vec<ToolCall>,
    text: String,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self { tools, buf: String::new(), calls: Vec::new(), text: String::new() }
    }

    /// Feed one chunk. Returns the calls completed by this chunk.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        self.buf.push_str(chunk);
        let mut produced = Vec::new();

        loop {
            let Some(rel) = self.buf.find("<<") else {
                // No marker at all: keep only a tail that could still open one.
                let keep = partial_open_len(&self.buf);
                let split = self.buf.len() - keep;
                self.text.push_str(&self.buf[..split]);
                self.buf = self.buf[split..].to_string();
                break;
            };

            match parse_call_at(&self.buf, rel)? {
                Parsed::Call { call, end } => {
                    self.text.push_str(&self.buf[..rel]);
                    let built = build_call(call, &self.tools)?;
                    self.calls.push(built.clone());
                    produced.push(built);
                    self.buf = self.buf[end..].to_string();
                }
                Parsed::Incomplete => {
                    // Everything before the marker is settled text; hold the rest.
                    self.text.push_str(&self.buf[..rel]);
                    self.buf = self.buf[rel..].to_string();
                    break;
                }
                Parsed::NotACall => {
                    let upto = rel + 2;
                    self.text.push_str(&self.buf[..upto]);
                    self.buf = self.buf[upto..].to_string();
                }
            }
        }

        Ok(produced)
    }

    /// Finish the stream, returning all calls and the plain text around them.
    /// A trailing partial call is treated as text, never as a guessed call.
    pub fn finish(mut self) -> (Vec<ToolCall>, String) {
        self.text.push_str(&self.buf);
        self.buf.clear();
        (self.calls, self.text)
    }

    /// Calls decoded so far.
    pub fn calls(&self) -> &[ToolCall] {
        &self.calls
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of `<<call `.
fn partial_open_len(s: &str) -> usize {
    let max = CALL_OPEN.len().min(s.len());
    for n in (1..=max).rev() {
        let start = s.len() - n;
        if !s.is_char_boundary(start) {
            continue;
        }
        if CALL_OPEN.starts_with(&s[start..]) {
            return n;
        }
    }
    0
}
