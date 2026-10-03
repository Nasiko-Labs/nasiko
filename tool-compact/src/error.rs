/// Everything here is fail-closed: an error means *no* call is returned, never a guess.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    #[error("invalid arguments for `{tool}`: {reason}")]
    InvalidArguments { tool: String, reason: String },
    /// The marker itself is broken: bad name, bad JSON, missing `>>`, or never terminated.
    #[error("malformed call: {0}")]
    Malformed(String),
    /// Compaction is not applicable (unsupported schema, or the result would not be smaller).
    /// The caller should send the native tool definitions instead.
    #[error("compaction bypassed: {0}")]
    Bypass(String),
}

impl Error {
    /// Stable code used by the eval harness: `unknown_tool` or `invalid_arguments`
    /// (`bypass` is not a decode error).
    pub fn code(&self) -> &'static str {
        match self {
            Error::UnknownTool(_) => "unknown_tool",
            Error::InvalidArguments { .. } | Error::Malformed(_) => "invalid_arguments",
            Error::Bypass(_) => "bypass",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
