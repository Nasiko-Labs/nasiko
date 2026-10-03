use thiserror::Error;

/// Every error this crate surfaces.
///
/// Note what is absent: unsupported schemas. Those are not errors — [`crate::encode_tools`]
/// reports them as [`crate::CompactTools::Bypass`] so the caller falls back to native tools.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// The model called a tool that is not in the tool list.
    #[error("unknown_tool: '{0}'")]
    UnknownTool(String),

    /// A call was found but cannot be trusted: bad JSON, unterminated, missing a required field,
    /// wrong type, enum violation, or an argument the schema does not declare.
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),

    /// The caller's tool list is unusable as given (duplicate names, wrong OpenAI shape, or a
    /// schema outside the supported subset where no bypass is possible).
    #[error("invalid_tools: {0}")]
    InvalidTools(String),

    /// [`crate::decode_tools`] was handed text that is not this crate's compact format.
    #[error("malformed_compact: {0}")]
    MalformedCompact(String),
}

impl Error {
    /// Stable machine-readable code, matching the eval set's `expected.error` values.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::InvalidTools(_) => "invalid_tools",
            Self::MalformedCompact(_) => "malformed_compact",
        }
    }
}

/// `Result` with this crate's [`enum@Error`].
pub type Result<T> = std::result::Result<T, Error>;
