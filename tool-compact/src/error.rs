use thiserror::Error;

/// Fail-closed error types during tool compaction and decoding.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ToolCompactError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("missing required field: {field} for tool: {tool}")]
    MissingRequiredField { tool: String, field: String },

    #[error("invalid argument type: {field} expected {expected}, got {got}")]
    InvalidType {
        tool: String,
        field: String,
        expected: String,
        got: String,
    },

    #[error("invalid enum value: {field} got {got}, expected one of {allowed:?}")]
    InvalidEnumValue {
        tool: String,
        field: String,
        allowed: Vec<String>,
        got: String,
    },

    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("malformed call syntax: {0}")]
    MalformedSyntax(String),

    #[error("json parse error: {0}")]
    JsonError(String),
}

impl ToolCompactError {
    /// Return the canonical error code for evaluation cases:
    /// "unknown_tool" or "invalid_arguments".
    pub fn error_code(&self) -> &'static str {
        match self {
            ToolCompactError::UnknownTool(_) => "unknown_tool",
            _ => "invalid_arguments",
        }
    }
}
