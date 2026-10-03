use std::fmt;

/// Decoding and encoding failures. Decoding never guesses: any violation of the
/// original schema produces an error here instead of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactError {
    /// The model named a tool that was not offered.
    UnknownTool { name: String },
    /// Arguments violate the tool's JSON Schema, or the payload is not valid
    /// JSON.
    InvalidArguments { tool: String, reason: String },
    /// The text looked like a call but did not parse as one.
    MalformedCall { reason: String },
    /// A schema feature this format does not represent. Callers bypass
    /// compaction and send native tool definitions instead.
    Unsupported { reason: String },
}

impl CompactError {
    /// Stable machine-readable code, as used by the evaluation set.
    pub fn code(&self) -> &'static str {
        match self {
            CompactError::UnknownTool { .. } => "unknown_tool",
            // A call that does not parse is not a call we can safely guess at,
            // so it is reported as an argument failure rather than silently
            // dropped.
            CompactError::InvalidArguments { .. } | CompactError::MalformedCall { .. } => {
                "invalid_arguments"
            }
            CompactError::Unsupported { .. } => "unsupported_schema",
        }
    }
}

impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompactError::UnknownTool { name } => write!(f, "unknown tool `{name}`"),
            CompactError::InvalidArguments { tool, reason } => {
                write!(f, "invalid arguments for `{tool}`: {reason}")
            }
            CompactError::MalformedCall { reason } => write!(f, "malformed call: {reason}"),
            CompactError::Unsupported { reason } => write!(f, "unsupported schema: {reason}"),
        }
    }
}

impl std::error::Error for CompactError {}
