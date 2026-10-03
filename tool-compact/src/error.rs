use thiserror::Error;

/// Error taxonomy conforming to Section 1.11 of the CTP/1 specification.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    #[error("unsupported schema: {reason}{}", pointer.as_ref().map(|p| format!(" at {}", p)).unwrap_or_default())]
    UnsupportedSchema {
        reason: String,
        pointer: Option<String>,
    },

    #[error("invalid schema: {reason}{}", pointer.as_ref().map(|p| format!(" at {}", p)).unwrap_or_default())]
    InvalidSchema {
        reason: String,
        pointer: Option<String>,
    },

    #[error("duplicate tool name: {name}")]
    DuplicateToolName {
        name: String,
    },

    #[error("malformed call: {reason} at byte offset {offset}")]
    MalformedCall {
        reason: String,
        offset: usize,
    },

    #[error("incomplete call: {reason} at byte offset {offset}")]
    IncompleteCall {
        reason: String,
        offset: usize,
    },

    #[error("invalid UTF-8: {reason} at byte offset {offset}")]
    InvalidUtf8 {
        reason: String,
        offset: usize,
    },

    #[error("invalid JSON: {reason} at byte offset {offset}")]
    InvalidJson {
        reason: String,
        offset: usize,
    },

    #[error("duplicate object key '{key}' at byte offset {offset}")]
    DuplicateKey {
        key: String,
        offset: usize,
    },

    #[error("unknown tool '{name}' at byte offset {offset}")]
    UnknownTool {
        name: String,
        offset: usize,
    },

    #[error("invalid arguments for tool '{tool}': {reason}{}", pointer.as_ref().map(|p| format!(" at {}", p)).unwrap_or_default())]
    InvalidArguments {
        tool: String,
        reason: String,
        pointer: Option<String>,
    },

    #[error("numeric range error: {reason} at byte offset {offset}")]
    NumericRange {
        reason: String,
        offset: usize,
    },

    #[error("limit exceeded: {limit} ({value} > {max})")]
    LimitExceeded {
        limit: &'static str,
        value: usize,
        max: usize,
    },
}

impl CompactError {
    /// Maps to public evaluation error category string.
    pub fn public_eval_error(&self) -> &'static str {
        match self {
            Self::UnknownTool { .. } => "unknown_tool",
            Self::UnsupportedSchema { .. } => "unsupported_schema",
            Self::InvalidSchema { .. } => "invalid_schema",
            Self::DuplicateToolName { .. } => "duplicate_tool_name",
            Self::MalformedCall { .. }
            | Self::IncompleteCall { .. }
            | Self::InvalidUtf8 { .. }
            | Self::InvalidJson { .. }
            | Self::DuplicateKey { .. }
            | Self::InvalidArguments { .. }
            | Self::NumericRange { .. }
            | Self::LimitExceeded { .. } => "invalid_arguments",
        }
    }

    /// Exact Section 1.11 evaluation category mapping.
    pub fn eval_label(&self) -> &'static str {
        match self {
            Self::UnknownTool { .. } => "unknown_tool",
            Self::MalformedCall { .. }
            | Self::IncompleteCall { .. }
            | Self::InvalidUtf8 { .. }
            | Self::InvalidJson { .. }
            | Self::DuplicateKey { .. } => "model_parse_failed",
            Self::InvalidArguments { .. }
            | Self::NumericRange { .. } => "schema_violation",
            Self::UnsupportedSchema { .. } => "unsupported_schema",
            Self::InvalidSchema { .. } => "invalid_schema",
            Self::DuplicateToolName { .. } => "duplicate_tool_name",
            Self::LimitExceeded { .. } => "limit_exceeded",
        }
    }

    /// True if this error signifies that the request cannot be handled compactly and should bypass.
    pub fn should_bypass(&self) -> bool {
        matches!(
            self,
            Self::UnsupportedSchema { .. }
                | Self::InvalidSchema { .. }
                | Self::DuplicateToolName { .. }
        )
    }
}
