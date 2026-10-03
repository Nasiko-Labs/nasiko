use crate::decoder::decode_calls;
use crate::error::ToolCompactError;
use crate::types::{ToolCall, ToolDef};

/// Incremental decoder for streaming responses, buffering chunks and handling split markers.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
}

impl StreamDecoder {
    /// Create a new StreamDecoder configured with allowed tools.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            buffer: String::new(),
            tools,
        }
    }

    /// Feed an incoming text chunk into the decoder.
    pub fn push_chunk(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    /// Retrieve the current accumulated text buffer.
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Decode all calls currently available in the accumulated buffer.
    pub fn current_calls(&self) -> Result<Vec<ToolCall>, ToolCompactError> {
        decode_calls(&self.buffer, &self.tools)
    }

    /// Finish decoding at the end of stream and return decoded tool calls.
    pub fn finish(self) -> Result<Vec<ToolCall>, ToolCompactError> {
        decode_calls(&self.buffer, &self.tools)
    }
}
