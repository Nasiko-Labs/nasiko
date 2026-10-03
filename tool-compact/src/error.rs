use thiserror::Error;

/// Custom error types for `nasiko-tool-compact`.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ToolCompactError {
    /// Triggered when a tool call references a tool not registered in the schema registry.
    #[error("Unknown tool: '{0}'")]
    UnknownTool(String),

    /// Triggered when tool arguments fail schema validation (type mismatch, missing required parameter, etc.).
    #[error("Invalid arguments for tool '{tool}': {reasoning}")]
    InvalidArguments {
        tool: String,
        reasoning: String,
    },

    /// Triggered when the input syntax violates the compact tool grammar.
    #[error("Malformed compact syntax: {0}")]
    Malformed(String),

    /// Triggered when a schema definition is invalid.
    #[error("Schema error: {0}")]
    SchemaError(String),

    /// Triggered during serialization or deserialization issues.
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

pub type Result<T> = std::result::Result<T, ToolCompactError>;
