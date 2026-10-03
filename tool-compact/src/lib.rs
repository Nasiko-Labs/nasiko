//! Compact tool schemas and a decoder back to standard tool calls.
//!
//! Pure library: no I/O, no environment, no clock, no RNG. Decoding fails closed —
//! an unknown tool or invalid arguments is an [`Error`], never a guessed call.
//!
//! The grammar is generic. Nothing in this crate names a particular tool.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

#[cfg(test)]
mod tests;

mod decode;
mod schema;

pub use decode::StreamDecoder;

use serde_json::Value;

pub type Result<T> = std::result::Result<T, Error>;

/// A function tool. The router maps its own IR onto this at the seam.
/// `parameters` is a JSON Schema object when the tool has one.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

/// One decoded call. `arguments` is a JSON object encoded as a string.
/// The router assigns the call id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// Compact definitions plus the call-format instruction, ready to inject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("unknown_tool")]
    UnknownTool,
    #[error("invalid_arguments")]
    InvalidArguments,
    /// Schema uses a feature this crate will not compact. Caller bypasses compaction.
    #[error("unsupported_schema")]
    Unsupported,
    /// Compact text is not in the grammar this crate writes.
    #[error("invalid_compact")]
    InvalidCompact,
    #[error("not implemented")]
    Unimplemented,
}

/// One line per tool, then the call-format trailer. See `GRAMMAR.md`.
///
/// Object properties are sorted by name. [`Error::Unsupported`] means the schema
/// cannot be represented exactly; the caller should send that tool in its native form.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    schema::encode_tools(tools)
}

/// Rebuild tool definitions from [`encode_tools`] output.
///
/// Required fields, types, enums, arrays and nested objects survive. Field-level
/// `description` values do not: only the tool description is kept.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    schema::decode_tools(&compact.text)
}

/// Decode every `<<call name {json}>>` in `text`.
///
/// The end of a call is the `>>` after the JSON value closes: track string state and
/// brace depth. Never stop at the first `>>`, which may sit inside a string argument.
/// Text with no call is an empty vec. Unknown tools and schema violations are errors.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    decode::decode_calls(text, tools)
}
