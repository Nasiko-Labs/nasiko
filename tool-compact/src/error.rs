use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CompactError {
    #[error("invalid tool definition: {0}")]
    InvalidToolDefinition(String),

    #[error("invalid JSON: {0}")]
    InvalidJson(String),

    #[error("unsupported JSON Schema feature: {0}")]
    UnsupportedSchema(String),

    #[error("invalid compact syntax: {0}")]
    InvalidSyntax(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid arguments for tool '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },

    #[error("malformed tool call: {0}")]
    MalformedCall(String),

    #[error("incomplete tool call")]
    IncompleteCall,
}

pub type Result<T> = std::result::Result<T, CompactError>;
