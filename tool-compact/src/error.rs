use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid tool definition: {0}")]
    InvalidToolDefinition(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid tool call: {0}")]
    InvalidToolCall(String),

    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("invalid compact call syntax")]
    InvalidSyntax,

    #[error("invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
