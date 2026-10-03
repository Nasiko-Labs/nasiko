//! Batch decoder (owned by the decoder workstream).

use crate::types::{DecodeError, ToolCall, ToolDef};

/// Decode every `<<call ...>>` in `text`, validating against `tools`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let _ = (text, tools);
    todo!("decoder workstream")
}
