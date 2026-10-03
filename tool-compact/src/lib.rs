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
mod grammar;
mod schema;
mod stream;
mod types;

pub use stream::StreamDecoder;
pub use types::CompactError;

use types::not_built;

/// Compact form of a tool list. The body is filled in when encoding lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    _private: (),
}

/// Encode `tools` into the compact signature block.
pub fn encode_tools(_tools: &[()]) -> Result<CompactTools, CompactError> {
    Err(not_built())
}

/// Decode compact call text into tool calls.
pub fn decode_calls(_text: &str, _tools: &[()]) -> Result<Vec<()>, CompactError> {
    Err(not_built())
}

/// Rebuild tool schemas from a compact block.
pub fn decode_tools(_compact: &CompactTools) -> Result<Vec<()>, CompactError> {
    Err(not_built())
}
