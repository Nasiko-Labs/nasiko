use crate::types::{ToolCall, not_built};

/// Incremental call decoder. Chunk handling lands in a later task.
#[derive(Debug, Default)]
pub struct StreamDecoder {
    _private: (),
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self { _private: () }
    }

    pub fn push(&mut self, _chunk: &str) -> Result<Vec<ToolCall>, crate::types::CompactError> {
        let _ = not_built;
        Ok(vec![])
    }
}
