//! Fail-closed error types for compact tool encode/decode.

/// Failures from encode / decode / stream. Never invent a call on error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactError {
    /// Schema uses a feature this encoder does not support — caller should bypass.
    #[error("unsupported schema feature: {0}")]
    UnsupportedSchema(String),
    /// Tool name is empty or not a compact identifier.
    #[error("invalid tool name: {0}")]
    InvalidToolName(String),
    /// Model named a tool that is not in the provided set.
    #[error("unknown_tool")]
    UnknownTool,
    /// Arguments failed JSON parse or schema validation.
    #[error("invalid_arguments")]
    InvalidArguments,
    /// Compact call marker / grammar was malformed.
    #[error("malformed_call")]
    MalformedCall,
    /// A previous stream error left the decoder closed.
    #[error("decoder_closed")]
    DecoderClosed,
}

impl CompactError {
    /// Stable wire label for eval / scorer (`unknown_tool`, `invalid_arguments`, …).
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::UnknownTool => "unknown_tool",
            Self::InvalidArguments => "invalid_arguments",
            Self::MalformedCall => "malformed_call",
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::InvalidToolName(_) => "invalid_tool_name",
            Self::DecoderClosed => "decoder_closed",
        }
    }
}
