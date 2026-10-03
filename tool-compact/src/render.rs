//! Renders tool calls in the compact call format.

use crate::types::ToolCall;

/// Write each call as `<<call name {json}>>`, one per line.
pub fn render_calls(calls: &[ToolCall]) -> String {
    let _ = calls;
    todo!("iteration 1")
}
