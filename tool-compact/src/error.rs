/// Failures from encoding, decoding, or validating compact tool calls.
///
/// Every variant is a refusal. Callers must not infer a tool, fill in a missing
/// field, or repair an argument.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The call named a tool that is not in the supplied definition list.
    #[error("unknown tool '{0}'")]
    UnknownTool(String),

    /// The call parsed, and the tool exists, but the arguments do not satisfy
    /// the original JSON Schema.
    #[error("invalid arguments for '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },

    /// The compact call syntax itself is broken (truncated marker, bad JSON,
    /// missing terminator).
    #[error("malformed compact call: {0}")]
    Malformed(String),

    /// The schema uses a construct this crate will not compact or check.
    /// Compaction is bypassed for the whole tool list rather than dropping it.
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),
}
