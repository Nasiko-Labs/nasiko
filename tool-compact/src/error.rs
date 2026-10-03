//! Fail-closed errors for compact tool schemas.
//!
//! Model output is untrusted. Unknown tools, invalid arguments, and malformed
//! markers become explicit errors — never guessed into valid-looking calls.

use thiserror::Error;

/// Errors produced by encode / decode / stream paths.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CompactError {
    #[error("invalid schema: {0}")]
    InvalidSchema(String),

    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("malformed call: {0}")]
    MalformedCall(String),

    #[error("invalid JSON: {0}")]
    InvalidJson(String),

    #[error("missing required field: {0}")]
    MissingRequiredField(String),
}

impl CompactError {
    /// Stable machine label for eval harnesses (`unknown_tool`, `invalid_arguments`, …).
    pub fn label(&self) -> &'static str {
        match self {
            Self::InvalidSchema(_) => "invalid_schema",
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments(_) | Self::MissingRequiredField(_) => "invalid_arguments",
            Self::MalformedCall(_) => "malformed_call",
            Self::InvalidJson(_) => "invalid_json",
        }
    }
}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, CompactError>;
