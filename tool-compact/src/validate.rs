//! Argument validation against the original schema (owned by the decoder workstream).

use serde_json::Value;

use crate::types::{DecodeError, ToolDef};

/// Check that `args` is valid for the tool called `call_name`.
pub fn validate_call(call_name: &str, args: &Value, tools: &[ToolDef]) -> Result<(), DecodeError> {
    let _ = (call_name, args, tools);
    todo!("decoder workstream")
}
