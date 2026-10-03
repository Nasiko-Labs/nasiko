//! Error types for compact tool encoding and decoding.

use std::fmt;

/// Top-level crate error.
#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    /// The tool definition contains schema constructs we cannot compact losslessly.
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),

    /// A decoding error occurred while parsing model output.
    #[error("decode error: {0}")]
    Decode(#[from] DecodeError),

    /// JSON serialization / deserialization failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Errors produced when decoding compact model output back into tool calls.
///
/// Every variant represents a fail-closed rejection — the decoder never guesses.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    /// The model referenced a tool name that does not exist in the tool set.
    UnknownTool(String),
    /// A required parameter is missing from the call arguments.
    MissingRequired {
        tool: String,
        field: String,
    },
    /// An argument value does not match the expected type or enum constraint.
    InvalidArgument {
        tool: String,
        field: String,
        reason: String,
    },
    /// The `<<call ...>>` marker is syntactically malformed.
    MalformedCall(String),
    /// JSON parsing of arguments failed.
    InvalidJson {
        tool: String,
        reason: String,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool(name) => write!(f, "unknown tool: {name}"),
            Self::MissingRequired { tool, field } => {
                write!(f, "{tool}: missing required field '{field}'")
            }
            Self::InvalidArgument { tool, field, reason } => {
                write!(f, "{tool}.{field}: {reason}")
            }
            Self::MalformedCall(detail) => write!(f, "malformed call: {detail}"),
            Self::InvalidJson { tool, reason } => {
                write!(f, "{tool}: invalid JSON arguments: {reason}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}
