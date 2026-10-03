use thiserror::Error as ThisError;

/// Everything that can go wrong. Encoding failures mean "send native tools"; decoding
/// failures mean "the model's output is not a valid call" and must never become one.
#[derive(Debug, Clone, PartialEq, Eq, ThisError)]
pub enum Error {
    /// The schema uses a feature the compact form cannot express exactly.
    #[error("unsupported schema at {path}: {reason}")]
    Unsupported { path: String, reason: String },
    /// A call names a tool that was not offered.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// A call's arguments do not satisfy the tool's schema.
    #[error("invalid arguments for {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },
    /// Output that starts like a call but is not well-formed.
    #[error("malformed call: {0}")]
    Malformed(String),
}

impl Error {
    /// Stable label used in eval output and router metadata.
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "unsupported",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::Malformed(_) => "malformed",
        }
    }
}
