use thiserror::Error;

/// Every error this crate surfaces.
///
/// Encoding errors ([`Error::Unsupported`]) mean "bypass compaction and send the native tools";
/// decoding errors mean "the model's output is not a call we can vouch for" — the caller applies
/// its failure policy and never receives a guessed call.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// A tool uses a schema feature the compact format cannot express losslessly.
    #[error("tool '{tool}' cannot be compacted: {reason}")]
    Unsupported { tool: String, reason: String },

    /// [`crate::decode_tools`] could not parse a compact definition.
    #[error("invalid compact definition: {0}")]
    InvalidCompact(String),

    /// The model called a tool that was not offered.
    #[error("unknown tool '{0}'")]
    UnknownTool(String),

    /// The call's arguments are not valid JSON or violate the tool's schema.
    #[error("invalid arguments for '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },

    /// A `<<call` marker was opened but not followed by a well-formed call.
    #[error("malformed call: {0}")]
    MalformedCall(String),
}

impl Error {
    /// Stable snake_case label, used in eval output and telemetry.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "unsupported_schema",
            Self::InvalidCompact(_) => "invalid_compact",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::MalformedCall(_) => "malformed_call",
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
