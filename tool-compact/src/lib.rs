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

/// Encode `tools` into the compact signature block.
///
/// Every tool is classified first. One unsupported schema fails the batch so
/// the caller can send the native tools instead of a simplified form.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    encode::render(tools)
}

/// Decode compact call text into tool calls.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    decode::calls_from_text(text, tools)
}

/// Rebuild tool schemas from the compact signature text.
///
/// The stored originals are not consulted. A fact missing from the text is missing here.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    decode::schemas_from_text(&compact.text)
}
