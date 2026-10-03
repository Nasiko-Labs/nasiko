use thiserror::Error;

/// Errors produced during tool compaction encoding, decoding, streaming, or validation.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ToolCompactError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Unsupported schema: {0}")]
    UnsupportedSchema(String),
}
