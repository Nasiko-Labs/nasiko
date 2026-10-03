//! # nasiko-tool-compact
//!
//! Compact tool schema encoding and fail-closed tool call decoding for LLM routers.
//!
//! Provides a dense signature format for OpenAI-shaped function tools, cutting prompt
//! token overhead while preserving standard client-facing tool calls.
//!
//! ## Key Invariants
//! 1. **Pure library**: Zero IO, zero network, zero env reads, zero provider coupling.
//! 2. **Lossless schema verification**: `decode_tools` confirms that schema definitions survive compaction.
//! 3. **Fail-closed decoding**: Unknown tools, missing required fields, or enum violations return explicit errors.
//! 4. **Robust streaming**: Handles split markers (`<<ca` + `ll ...`), nested braces, and `>>` inside quoted strings.

pub mod decoder;
pub mod encoder;
pub mod error;
pub mod types;
pub mod validator;

pub use decoder::{decode_calls, StreamDecoder};
pub use encoder::{decode_tools, encode_tools, parse_compact_schema};
pub use error::{DecodeError, EncodeError};
pub use types::{
    CompactTools, FunctionCall, FunctionCallDelta, FunctionDef, ToolCall, ToolCallDelta, ToolDef,
};
pub use validator::validate_call;
