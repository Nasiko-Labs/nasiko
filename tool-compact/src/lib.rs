//! Reversible tool definitions and fail-closed tool-call decoding.
//!
//! Pure library: no environment, filesystem, network, provider, or router access.
//! See the crate README for the wire grammar and the supported JSON Schema subset.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod codec;
mod decoder;
mod error;
mod models;
mod schema;
mod strict_json;

pub use codec::{decode_tools, encode_tools};
pub use decoder::{StreamDecoder, decode_calls, decode_output, render_calls};
pub use error::{CompactError, Result};
pub use models::{CALL_INSTRUCTIONS, CompactTools, DecodedOutput, Limits, ToolCall, ToolDef};
