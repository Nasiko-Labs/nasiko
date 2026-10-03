//! Encoder: tool definitions to compact signature lines.

use crate::types::{CompactTools, EncodeError, ToolDef};

/// Render `tools` as call-format instructions plus one signature line per compactable tool.
/// Tools using schema features outside the supported subset are listed in `bypassed`.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    let _ = tools;
    todo!("iteration 1")
}
