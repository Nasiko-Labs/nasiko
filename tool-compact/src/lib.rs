//! Compact tool schema encoder/decoder.
//!
//! Encodes full OpenAI-shaped tool JSON-Schema definitions into a compact,
//! model-friendly textual representation and decodes model output back to
//! standard OpenAI-shaped tool calls.
//!
//! # Public API
//!
//! - [`encode_tools`] — compress `Vec<ToolDef>` into a [`CompactTools`] text block.
//! - [`decode_calls`] — parse `<<call ToolName {json}>>` markers back into `Vec<ToolCall>`.
//! - [`decode_tools`] — round-trip: reconstruct full `Vec<ToolDef>` from compact metadata.
//! - [`StreamDecoder`] — incremental streaming decoder that handles split markers.
//! - [`CompactId`] — content-addressable cache key for multi-turn token savings.
//!
//! # Fail-closed safety model
//!
//! The decoder never guesses. Every decoded call is validated against the original
//! schema:
//!
//! - Unknown tool name → [`DecodeError::UnknownTool`]
//! - Missing required field → [`DecodeError::InvalidArguments`]
//! - Invalid enum value → [`DecodeError::InvalidEnumValue`]
//!
//! If compaction cannot preserve schema semantics (e.g. `oneOf`/`anyOf` constructs,
//! deeply nested objects), the encoder **bypasses** that tool and includes the full
//! JSON schema verbatim.

pub mod compact_id;
pub mod decode;
pub mod encode;
pub mod error;
pub mod stream;
pub mod types;

pub use compact_id::CompactId;
pub use decode::{decode_calls, decode_tools};
pub use encode::encode_tools;
pub use error::{DecodeError, EncodeError};
pub use stream::StreamDecoder;
pub use types::{CompactTools, FieldType, FunctionCall, FunctionDef, ToolCall, ToolDef, ToolSchema};

/// Grammar version for forward compatibility.
pub const GRAMMAR_VERSION: &str = "1.0";
