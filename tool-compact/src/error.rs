use thiserror::Error;

/// Every error this crate surfaces.
///
/// Encoding errors ([`Error::Unsupported`], [`Error::InvalidTool`]) mean "bypass compaction and
/// send the native tools". Decoding errors mean "the model's output is not a valid call" — the
/// caller must surface them, never repair them.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// The tool uses a schema feature the compact format cannot represent losslessly.
    #[error("unsupported schema in tool '{tool}' at {path}: {feature}")]
    Unsupported {
        tool: String,
        path: String,
        feature: String,
    },
    /// The tool definition itself is invalid (bad or duplicate name, non-object parameters).
    #[error("invalid tool definition '{tool}': {reason}")]
    InvalidTool { tool: String, reason: String },
    /// The model called a tool that was not offered.
    #[error("unknown tool '{0}'")]
    UnknownTool(String),
    /// The call's arguments do not satisfy the tool's schema.
    #[error("invalid arguments for '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },
    /// The output contains a call marker that does not follow the call grammar.
    #[error("malformed call: {0}")]
    MalformedCall(String),
    /// [`crate::decode_tools`] was given text that is not in the definitions grammar.
    #[error("malformed compact definitions at line {line}: {reason}")]
    MalformedDefinitions { line: usize, reason: String },
}

impl Error {
    /// Stable, machine-readable code. Changing one changes an eval output value, so they are
    /// spelled out rather than derived from the variant name.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "unsupported_schema",
            Self::InvalidTool { .. } => "invalid_tool",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::MalformedCall(_) => "malformed_call",
            Self::MalformedDefinitions { .. } => "malformed_definitions",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
