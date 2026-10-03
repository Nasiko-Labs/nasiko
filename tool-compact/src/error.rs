//! Error types for compact tool schema processing.

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum CompactToolError {
    /// The model returned a <<call>> for a tool name not in the known set.
    /// Policy: return the error; never guess a nearby name.
    #[error("unknown tool '{name}'")]
    UnknownTool { name: String },

    /// A required parameter was absent from the model's JSON args.
    #[error("tool '{tool}' is missing required field '{field}'")]
    MissingRequired { tool: String, field: String },

    /// A parameter value was not in the declared enum set.
    #[error("field '{field}' has invalid value '{got}'; allowed: {}", .allowed.join(", "))]
    EnumViolation {
        field: String,
        got: String,
        allowed: Vec<String>,
    },

    /// The model's JSON arguments were not valid JSON.
    #[error("invalid JSON arguments: {0}")]
    InvalidJson(String),

    /// The <<call ...>> marker was structurally malformed (e.g. no space, no >>).
    #[error("malformed <<call>> marker: {0}")]
    MarkerMalformed(String),
}
