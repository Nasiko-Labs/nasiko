//! Error types for tool-compact.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactError {
    #[error("unknown tool: `{0}`")]
    UnknownTool(String),

    #[error("missing required field `{field}` in tool `{tool}`")]
    MissingRequired { tool: String, field: String },

    #[error("invalid argument value for field `{field}` in tool `{tool}`: {reason}")]
    InvalidArgument {
        tool: String,
        field: String,
        reason: String,
    },

    #[error("malformed call syntax: {0}")]
    MalformedCall(String),

    #[error("unsupported schema feature in tool `{0}`: {1}")]
    UnsupportedSchema(String, String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
