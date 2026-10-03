use crate::error::Result;
use crate::types::{ToolCall, ToolDef};

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Text(String),
    Call(ToolCall),
}

pub struct StreamDecoder {
    _tools: Vec<ToolDef>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            _tools: tools.to_vec(),
        }
    }

    pub fn push(&mut self, _chunk: &str) -> Result<Vec<StreamEvent>> {
        todo!()
    }

    pub fn finish(self) -> Result<Vec<ToolCall>> {
        todo!()
    }
}
