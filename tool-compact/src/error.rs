use thiserror::Error;

/// Every error this crate surfaces. Decoding never guesses: anything it cannot prove valid
/// against the original schema is one of these.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// The tool list uses a schema feature the compact format cannot carry. The caller should
    /// send the native request instead (bypass compaction).
    #[error("tool `{tool}` cannot be compacted: {reason}")]
    Unsupported { tool: String, reason: String },

    /// Compact definitions text that does not follow the grammar (from [`crate::decode_tools`]).
    #[error("invalid compact definitions: {0}")]
    InvalidDefinitions(String),

    /// The model called a tool that was not offered.
    #[error("unknown tool `{0}`")]
    UnknownTool(String),

    /// The call names a known tool but its arguments are not valid JSON or break the schema.
    #[error("invalid arguments for `{tool}`: {reason}")]
    InvalidArguments { tool: String, reason: String },

    /// A call marker was opened but the call is structurally broken or never closed.
    #[error("malformed tool call: {0}")]
    Malformed(String),
}

impl Error {
    /// Stable machine-readable code, used in eval output.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Unsupported { .. } => "unsupported_schema",
            Error::InvalidDefinitions(_) => "invalid_definitions",
            Error::UnknownTool(_) => "unknown_tool",
            Error::InvalidArguments { .. } => "invalid_arguments",
            Error::Malformed(_) => "malformed_call",
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
