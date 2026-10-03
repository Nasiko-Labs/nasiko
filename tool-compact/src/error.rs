use thiserror::Error;

#[derive(Debug, Error, PartialEq, Clone)]
pub enum CompactError {
    #[error("unsupported schema in tool '{tool}': {reason}")]
    UnsupportedSchema { tool: String, reason: String },

    #[error("invalid schema in tool '{tool}': {reason}")]
    InvalidSchema { tool: String, reason: String },

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid arguments for tool '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },

    #[error("invalid syntax: {0}")]
    InvalidSyntax(String),

    #[error("stream ended in the middle of a call")]
    IncompleteStream,
}

impl CompactError {
    pub fn code(&self) -> &'static str {
        match self {
            CompactError::UnknownTool(_) => "unknown_tool",
            CompactError::InvalidArguments { .. } => "invalid_arguments",
            CompactError::UnsupportedSchema { .. } => "unsupported_schema",
            CompactError::InvalidSchema { .. } => "invalid_schema",
            CompactError::InvalidSyntax(_) => "invalid_syntax",
            CompactError::IncompleteStream => "incomplete_stream",
        }
    }
}

pub type Result<T> = std::result::Result<T, CompactError>;
