//! Lossless compact tool definitions and validated compact-call decoding.
//!
//! The crate is deliberately pure: it performs no I/O, reads no environment variables, and
//! does not assign provider-facing tool-call ids.

#![forbid(unsafe_code)]

mod codec;
mod decoder;
mod error;
mod models;
mod schema;

pub use codec::{decode_tools, encode_tools};
pub use decoder::{StreamDecoder, decode_calls, render_call};
pub use error::{CompactError, ErrorCode};
pub use models::{CompactTools, FunctionDef, ToolCall, ToolDef};

