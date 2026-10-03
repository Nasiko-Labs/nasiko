//! Incremental decoder (owned by the decoder workstream).

use crate::types::{DecodeError, ToolCall, ToolDef};

/// Feeds model output chunk by chunk; correct when a marker is split across chunks.
pub struct StreamDecoder {
    _tools: Vec<ToolDef>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            _tools: tools.to_vec(),
        }
    }

    /// Calls completed by this chunk.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, DecodeError> {
        let _ = chunk;
        todo!("decoder workstream")
    }

    /// Errors if a call is still open.
    pub fn finish(&mut self) -> Result<Vec<ToolCall>, DecodeError> {
        todo!("decoder workstream")
    }
}
