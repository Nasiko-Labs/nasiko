//! The one error type of this crate.

/// Everything that can go wrong while encoding definitions or decoding calls.
///
/// Decoding never "repairs" model output: every problem is one of these variants, so a caller
/// can apply its failure policy instead of forwarding a guessed call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactError {
    /// The tool uses a JSON Schema feature the compact grammar cannot express. Callers should
    /// bypass compaction for the whole request (the crate docs list the supported subset).
    #[error("tool `{tool}` uses an unsupported schema feature: {reason}")]
    Unsupported { tool: String, reason: String },
    /// The tool definition itself is unusable (bad or duplicate name, non-object parameters).
    #[error("invalid tool definition `{tool}`: {reason}")]
    InvalidTool { tool: String, reason: String },
    /// The model called a tool that is not in the request's tool list.
    #[error("unknown tool `{name}`")]
    UnknownTool { name: String },
    /// The call's arguments are not valid JSON or violate the tool's schema.
    #[error("invalid arguments for `{tool}` at {path}: {reason}")]
    InvalidArguments {
        tool: String,
        path: String,
        reason: String,
    },
    /// The text contains a call marker but not a well-formed call.
    #[error("malformed tool call: {reason}")]
    MalformedCall { reason: String },
    /// `decode_tools` could not parse compact definitions.
    #[error("malformed compact definitions (line {line}): {reason}")]
    MalformedDefinitions { line: usize, reason: String },
}

impl CompactError {
    /// A stable snake_case label for metrics and eval output.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "unsupported_schema",
            Self::InvalidTool { .. } => "invalid_tool",
            Self::UnknownTool { .. } => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::MalformedCall { .. } => "malformed_call",
            Self::MalformedDefinitions { .. } => "malformed_definitions",
        }
    }
}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, CompactError>;
