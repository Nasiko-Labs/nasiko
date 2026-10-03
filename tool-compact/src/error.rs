use thiserror::Error;

/// Stable evaluator-facing error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    UnsupportedSchema,
    DuplicateTool,
    InvalidToolDefinition,
    InvalidCompactEncoding,
    UnknownTool,
    InvalidArguments,
    MalformedSyntax,
    TruncatedCall,
}

impl ErrorCode {
    /// Machine-readable error name used by the evaluation JSONL contract.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "unsupported_schema",
            Self::DuplicateTool => "duplicate_tool",
            Self::InvalidToolDefinition => "invalid_tool_definition",
            Self::InvalidCompactEncoding => "invalid_compact_encoding",
            Self::UnknownTool => "unknown_tool",
            Self::InvalidArguments => "invalid_arguments",
            Self::MalformedSyntax => "malformed_syntax",
            Self::TruncatedCall => "truncated_call",
        }
    }
}

/// Errors returned by compact encoding and decoding.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    #[error("unsupported schema feature '{feature}' at {path}")]
    UnsupportedSchema { path: String, feature: String },

    #[error("duplicate tool name '{0}'")]
    DuplicateTool(String),

    #[error("invalid tool definition: {0}")]
    InvalidToolDefinition(String),

    #[error("invalid compact encoding: {0}")]
    InvalidCompactEncoding(String),

    #[error("unknown tool '{0}'")]
    UnknownTool(String),

    #[error("invalid arguments for '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },

    #[error("malformed compact call: {0}")]
    MalformedSyntax(String),

    #[error("truncated compact call")]
    TruncatedCall,
}

impl CompactError {
    /// Stable category for callers that should not match error text.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedSchema { .. } => ErrorCode::UnsupportedSchema,
            Self::DuplicateTool(_) => ErrorCode::DuplicateTool,
            Self::InvalidToolDefinition(_) => ErrorCode::InvalidToolDefinition,
            Self::InvalidCompactEncoding(_) => ErrorCode::InvalidCompactEncoding,
            Self::UnknownTool(_) => ErrorCode::UnknownTool,
            Self::InvalidArguments { .. } => ErrorCode::InvalidArguments,
            Self::MalformedSyntax(_) => ErrorCode::MalformedSyntax,
            Self::TruncatedCall => ErrorCode::TruncatedCall,
        }
    }
}

