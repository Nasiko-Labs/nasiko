//! Compact function-tool schemas and decode the calls a model writes back.
//!
//! Pure library: no I/O, no environment reads, no clock. The router converts at
//! the seam. Fail closed — a call that does not match the original schema is an
//! error, never a guessed call.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod encode;
#[cfg(test)]
mod fixtures;
mod grammar;
mod schema;
mod stream;
mod types;

pub use stream::StreamDecoder;
pub use types::{ArgumentFault, CompactError, CompactTools, ToolCall, ToolDef};

use types::not_built;

/// Encode `tools` into the compact signature block.
pub fn encode_tools(_tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    Err(not_built())
}

/// Decode compact call text into tool calls.
pub fn decode_calls(_text: &str, _tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    Err(not_built())
}

/// Rebuild tool schemas from a compact block.
pub fn decode_tools(_compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    Err(not_built())
}
