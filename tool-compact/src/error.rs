use thiserror::Error;

/// Every error this crate surfaces.
///
/// Each variant has a stable label from [`ToolCompactError::kind`] that callers may put on the
/// wire (usage metadata, evaluation output). Messages carry the tool name, a JSON-pointer-like
/// path and a reason, never the offending value: arguments can be large or sensitive.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ToolCompactError {
    /// The model named a tool that is not in the catalog.
    #[error("unknown tool '{name}'")]
    UnknownTool { name: String },

    /// The call's arguments parsed as JSON but violate the tool's schema.
    #[error("invalid arguments for '{tool}' at {path}: {reason}")]
    InvalidArguments {
        tool: String,
        path: String,
        reason: String,
    },

    /// The call's framing or its JSON object is not well formed (this includes duplicate keys).
    #[error("malformed call: {reason}")]
    MalformedCall { reason: String },

    /// The output ended while a call was still open.
    #[error("output ended inside a tool call")]
    IncompleteCall,

    /// A size, count or depth limit from [`crate::limits`] was exceeded.
    #[error("limit exceeded: {limit} > {max}")]
    LimitExceeded { limit: &'static str, max: usize },

    /// A tool's schema uses a keyword or shape the notation cannot carry faithfully.
    #[error("unsupported schema in tool '{tool}' at {path}: {reason}")]
    UnsupportedSchema {
        tool: String,
        path: String,
        reason: String,
    },

    /// The catalog itself is unusable: duplicate or invalid names, or a compact line that does
    /// not parse back.
    #[error("invalid tool catalog: {reason}")]
    InvalidCatalog { reason: String },
}

impl ToolCompactError {
    /// Stable snake_case label for metadata and evaluation output.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::UnknownTool { .. } => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::MalformedCall { .. } => "malformed_call",
            Self::IncompleteCall => "incomplete_call",
            Self::LimitExceeded { .. } => "limit_exceeded",
            Self::UnsupportedSchema { .. } => "unsupported_schema",
            Self::InvalidCatalog { .. } => "invalid_catalog",
        }
    }

    pub(crate) fn unsupported(tool: &str, path: &str, reason: impl Into<String>) -> Self {
        Self::UnsupportedSchema {
            tool: tool.to_owned(),
            path: path.to_owned(),
            reason: reason.into(),
        }
    }

    pub(crate) fn malformed(reason: impl Into<String>) -> Self {
        Self::MalformedCall {
            reason: reason.into(),
        }
    }

    pub(crate) fn catalog(reason: impl Into<String>) -> Self {
        Self::InvalidCatalog {
            reason: reason.into(),
        }
    }

    pub(crate) const fn limit(limit: &'static str, max: usize) -> Self {
        Self::LimitExceeded { limit, max }
    }
}

pub type Result<T> = std::result::Result<T, ToolCompactError>;
