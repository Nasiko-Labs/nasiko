//! Error types for tool compaction and decoding.

use thiserror::Error;

/// All errors surfaced by this crate.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// The schema uses features not supported by the compact format,
    /// signalling the router to bypass compaction for this request.
    #[error("unsupported schema for tool '{tool}' at '{path}': {reason}")]
    UnsupportedSchema {
        tool: String,
        path: String,
        reason: String,
    },

    /// The model emitted a call for a tool name not in the provided tool definitions.
    #[error("unknown tool '{0}'")]
    UnknownTool(String),

    /// Arguments failed strict JSON parsing or schema validation.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    /// Syntactic violation of the call marker grammar.
    #[error("malformed call: {0}")]
    MalformedCall(String),

    /// Stream input ended while inside an unclosed tool call.
    #[error("unterminated tool call")]
    UnterminatedCall,

    /// Argument buffer or nesting depth exceeded safety thresholds.
    #[error("limit exceeded: {0}")]
    LimitExceeded(&'static str),
}

impl Error {
    /// Maps to evaluator error code string: "unknown_tool" or "invalid_arguments".
    pub fn code(&self) -> &'static str {
        match self {
            Error::UnknownTool(_) => "unknown_tool",
            _ => "invalid_arguments",
        }
    }
}
