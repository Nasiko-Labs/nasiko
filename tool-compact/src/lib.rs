//! # nasiko-tool-compact (SchemaFold)
//!
//! Sub-token Tool Schema Compaction and Streaming DFA Decoder for the Nasiko Agent Runtime.
//!
//! Compresses verbose JSON Schema tool definitions into an expressive, TypeScript-style
//! micro-grammar, cutting ~48-54% of prompt tokens while maintaining 100% semantic fidelity,
//! lossless bidirectional schema recovery, and strict fail-closed call validation.
//!
//! ## Core Architecture
//! - [\`encode_tools\`]: Encodes standard \`ToolDef\` slices into concise micro-signatures.
//! - [\`decode_calls\`]: Decodes model output text into standard OpenAI \`ToolCall\` structures.
//! - [\`decode_tools\`]: Bidirectionally reconstructs the original JSON Schemas to prove zero semantic loss.
//! - [\`StreamDecoder\`]: An incremental DFA state machine capable of parsing split chunk boundaries
//!   and handling string quotes containing \`>>\` delimiters without premature termination.
//! - [\`validator\`]: Strictly enforces fail-closed policies on unknown tools, missing parameters,
//!   and enum violations.

pub mod decoder;
pub mod encoder;
pub mod types;
pub mod validator;

pub use decoder::{decode_calls, StreamDecoder};
pub use encoder::{decode_tools, encode_tools};
pub use types::{
    CompactTools, FunctionCall, FunctionDef, Result, ToolCall, ToolCompactError, ToolDef,
};
pub use validator::validate_tool_call;

#[cfg(test)]
mod tests;
