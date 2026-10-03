//! Errors. Decoding has a rich internal taxonomy but exactly two external codes
//! ([`DecodeError::code`]): `unknown_tool` and `invalid_arguments`.

/// Why a tool list cannot be compacted. Every variant means "send the native tools instead";
/// none is a decode failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    #[error("no tools to encode")]
    NoTools,
    #[error("tool {tool:?}: unsupported schema at {path:?} ({keyword})")]
    Unsupported {
        tool: String,
        path: String,
        keyword: String,
    },
    #[error("tool {tool:?}: rendered definition does not round-trip")]
    RoundTrip { tool: String },
    #[error("compact definitions are not smaller than the native tools")]
    NotSmaller,
}

/// A definition text that does not follow the grammar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("definition line {line}: {reason}")]
pub struct ParseError {
    pub line: usize,
    pub reason: String,
}

/// Why a call's arguments (or the call syntax around them) were rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgError {
    #[error("malformed call: {0}")]
    Malformed(String),
    #[error("stream ended inside a call")]
    Incomplete,
    #[error("arguments exceed the size limit")]
    TooLarge,
    #[error("arguments are not a JSON object")]
    NotObject,
    #[error("missing required field {0}")]
    MissingRequired(String),
    #[error("unknown field {0}")]
    UnknownKey(String),
    #[error("{path}: expected {expected}")]
    WrongType {
        path: String,
        expected: &'static str,
    },
    #[error("{path}: value not in enum")]
    NotInEnum { path: String },
    #[error("{path}: value out of range")]
    OutOfRange { path: String },
    #[error("{path}: not a valid {format}")]
    BadFormat { path: String, format: &'static str },
    #[error("tool schema cannot be validated: {0}")]
    UnsupportedSchema(String),
}

/// A decode failure. Never accompanied by calls.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("unknown tool {name:?}")]
    UnknownTool { name: String },
    #[error("invalid arguments{}: {reason}", tool.as_deref().map(|t| format!(" for {t:?}")).unwrap_or_default())]
    InvalidArguments {
        tool: Option<String>,
        reason: ArgError,
    },
}

impl DecodeError {
    /// The external error code: `"unknown_tool"` or `"invalid_arguments"`.
    pub fn code(&self) -> &'static str {
        match self {
            DecodeError::UnknownTool { .. } => "unknown_tool",
            DecodeError::InvalidArguments { .. } => "invalid_arguments",
        }
    }

    pub(crate) fn args(tool: Option<&str>, reason: ArgError) -> Self {
        DecodeError::InvalidArguments {
            tool: tool.map(str::to_string),
            reason,
        }
    }

    /// A decoder cannot be built over tools whose schemas are outside the supported subset;
    /// that is reported as `invalid_arguments` (nothing can be validated, so nothing passes).
    pub(crate) fn from_encode(e: EncodeError) -> Self {
        DecodeError::args(None, ArgError::UnsupportedSchema(e.to_string()))
    }
}
