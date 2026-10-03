//! Error types for encode and decode operations.

use thiserror::Error;

/// Errors that can occur while encoding tool definitions.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum EncodeError {
    /// The tool list is empty; nothing to encode.
    #[error("no tools provided")]
    NoTools,
}

/// Errors that can occur while decoding a compact tool call response.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum DecodeError {
    /// A call references a tool name that was not in the provided set.
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },

    /// Required argument missing, wrong type, or an enum field has an invalid value.
    #[error("invalid arguments for {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },

    /// The marker syntax is fundamentally broken (truncated, no JSON, etc.).
    #[error("malformed call marker: {detail}")]
    MalformedCall { detail: String },
}
