use thiserror::Error;

/// Errors that can occur when decoding model output into tool calls.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("malformed tool call: {0}")]
    MalformedCall(String),

    #[error("schema violation: {0}")]
    SchemaViolation(String),
}

impl DecodeError {
    /// Returns the canonical error string expected by evaluation suites and telemetry.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments(_) | Self::SchemaViolation(_) | Self::MalformedCall(_) => {
                "invalid_arguments"
            }
        }
    }
}

/// Errors that can occur when encoding tool schemas into compact representation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    #[error("unsupported schema feature: {0}")]
    UnsupportedSchema(String),

    #[error("invalid tool definition: {0}")]
    InvalidDefinition(String),
}
