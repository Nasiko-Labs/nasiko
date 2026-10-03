//! Typed errors for compact tool encoding and decoding.
//!
//! Every error is a concrete variant — never a string bag. The two variants that reach
//! eval output are [`CompactError::UnknownTool`] and [`CompactError::InvalidArguments`];
//! others are internal to the encoder/decoder pipeline.

use thiserror::Error;

/// Every error this crate surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    /// The model emitted a call to a tool name not present in the provided tool set.
    #[error("unknown_tool")]
    UnknownTool { name: String },

    /// A decoded call's arguments failed validation against the original schema.
    ///
    /// `details` carries the reason (missing required field, enum violation, type
    /// mismatch, unknown key, …) for debugging; eval output emits only
    /// `{"error":"invalid_arguments"}`.
    #[error("invalid_arguments")]
    InvalidArguments { tool: String, details: String },

    /// The tool's JSON Schema uses a feature this crate cannot represent or validate.
    /// The tool is bypassed (kept native) rather than silently dropping constraints.
    #[error("unsupported schema feature '{feature}' in tool '{tool}'")]
    UnsupportedSchema { tool: String, feature: String },

    /// The model's output contains a syntactically invalid call marker (truncated,
    /// malformed, missing closing `>>`, etc.).
    #[error("malformed output: {detail}")]
    MalformedOutput { detail: String },

    /// An internal encoding error (schema parsing failure, etc.).
    #[error("encoding error: {detail}")]
    EncodingError { detail: String },
}

impl CompactError {
    /// The two-value tag used in eval JSONL output.
    ///
    /// Maps every variant to either `"unknown_tool"` or `"invalid_arguments"`.
    pub fn eval_tag(&self) -> &'static str {
        match self {
            Self::UnknownTool { .. } => "unknown_tool",
            _ => "invalid_arguments",
        }
    }
}
