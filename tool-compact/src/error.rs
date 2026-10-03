/// Stable error categories; messages never include model argument values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactError {
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),
    #[error("invalid tool definition: {0}")]
    InvalidSchema(String),
    #[error("unknown tool")]
    UnknownTool,
    #[error("invalid arguments")]
    InvalidArguments,
    #[error("malformed compact format")]
    MalformedOutput,
    #[error("incomplete compact call")]
    IncompleteCall,
    #[error("compact tool resource limit exceeded")]
    LimitExceeded,
}

impl CompactError {
    /// Machine-readable codes used by the JSONL evaluation contract.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::InvalidSchema(_) => "invalid_schema",
            Self::UnknownTool => "unknown_tool",
            Self::InvalidArguments => "invalid_arguments",
            Self::MalformedOutput => "malformed_output",
            Self::IncompleteCall => "incomplete_call",
            Self::LimitExceeded => "limit_exceeded",
        }
    }
}

pub type Result<T> = std::result::Result<T, CompactError>;
