//! Error types for encode and decode operations.
//!
//! All errors are explicit and descriptive — the library never guesses or
//! silently drops a malformed call.

use thiserror::Error;

/// Errors from [`crate::encode_tools`].
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("tool has no function name")]
    MissingName,

    #[error("unsupported schema construct in tool `{tool}`: {detail}")]
    UnsupportedSchema { tool: String, detail: String },

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Errors from [`crate::decode_calls`], [`crate::decode_tools`], and
/// [`crate::StreamDecoder`].
///
/// Every variant is fail-closed: the caller sees exactly what went wrong
/// instead of receiving a guess.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("unknown tool: `{0}`")]
    UnknownTool(String),

    #[error("missing required field `{field}` in tool `{tool}`")]
    InvalidArguments { tool: String, field: String },

    #[error(
        "invalid enum value `{value}` for field `{field}` in tool `{tool}`, \
         expected one of: {expected:?}"
    )]
    InvalidEnumValue {
        tool: String,
        field: String,
        value: String,
        expected: Vec<String>,
    },

    #[error("invalid JSON in call arguments for tool `{tool}`: {detail}")]
    MalformedArguments { tool: String, detail: String },

    #[error("no call markers found in output")]
    NoCallsFound,

    #[error("incomplete call marker (stream not finished)")]
    IncompleteMarker,

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}
