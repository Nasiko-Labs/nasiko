use thiserror::Error;

/// Errors that can occur during tool schema compaction.
#[derive(Debug, Error, PartialEq, Clone)]
pub enum CompactError {
    #[error("Empty tools list")]
    EmptyTools,

    #[error("Missing function name")]
    MissingFunctionName,

    #[error("Unsupported schema construct: {0}")]
    UnsupportedSchema(String),

    #[error("Invalid parameter schema: {0}")]
    InvalidParameterSchema(String),
}

/// Type alias for CompactError.
pub type CompactToolError = CompactError;

/// Errors that can occur during call decoding and validation.
#[derive(Debug, Error, PartialEq, Clone)]
pub enum DecodeError {
    #[error("unknown_tool: {0}")]
    UnknownTool(String),

    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),

    #[error("malformed_syntax: {0}")]
    MalformedSyntax(String),

    #[error("json_error: {0}")]
    JsonError(String),

    #[error("unexpected_eof")]
    UnexpectedEof,
}

impl DecodeError {
    /// Canonical category name matching test harness expectations:
    /// "unknown_tool", "invalid_arguments", or "malformed_syntax".
    pub fn category(&self) -> &'static str {
        match self {
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::MalformedSyntax(_) => "malformed_syntax",
            Self::JsonError(_) => "invalid_arguments",
            Self::UnexpectedEof => "malformed_syntax",
        }
    }
}
