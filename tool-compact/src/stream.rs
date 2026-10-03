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

