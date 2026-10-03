//! Compact tool-schema encoding and decoding for the Nasiko LLM Router.
//!
//! This crate provides a compact text representation for OpenAI-style tool definitions
//! that reduces prompt token usage, plus a decoder that converts compact model output
//! back into standard OpenAI-shaped tool calls.
//!
//! # Grammar (explicit specification)
//!
//! ## Tool definition format
//! ```text
//! tool_name(param:type, param?:type) - Description (omitted when name is self-describing)
//! ```
//!
//! ## Types
//! | Compact | JSON Schema equivalent |
//! |---------|----------------------|
//! | `str`   | `{"type": "string"}` |
//! | `int`   | `{"type": "integer"}` |
//! | `float` | `{"type": "number"}` |
//! | `bool`  | `{"type": "boolean"}` |
//! | `dt`    | `{"type": "string", "format": "date-time"}` |
//! | `a\|b\|c` | `{"type": "string", "enum": ["a","b","c"]}` |
//! | `[T]`   | `{"type": "array", "items": T}` |
//! | `{k:T}` | `{"type": "object", "properties": ...}` |
//! | `any`   | `{}` (no type constraint) |
//!
//! ## Required vs optional
//! - `param:type` = required (in JSON Schema `required` array)
//! - `param?:type` = optional (not in `required` array)
//!
//! ## Call format
//! ```text
//! <<call tool_name {"arg": "value"}>>
//! ```
//! - Markers: `<<call ` (open) and `>>` (close)
//! - Arguments: JSON object (validated against schema)
//! - Multiple calls: one `<<call ...>>` per line
//! - No tool needed: model answers normally without `<<call>>`
//! - `>>` inside JSON strings is handled correctly (string-aware parser)
//!
//! ## Error handling (fail-closed)
//! - Unknown tool name → `CompactError::UnknownTool`
//! - Missing required field → `CompactError::MissingRequired`
//! - Invalid enum value → `CompactError::InvalidArgument`
//! - Wrong type → `CompactError::InvalidArgument`
//! - Malformed JSON or syntax → `CompactError::MalformedCall`
//!
//! ## Unsupported schema features (bypass compaction)
//!
//! - `oneOf` / `anyOf` / `allOf` combinators
//! - `$ref` references
//! - `additionalProperties` constraints
//! - `patternProperties` / `pattern` regex constraints
//! - `minLength` / `maxLength` / `minimum` / `maximum` numeric bounds
//! - `const` values
//!
//! These are documented but not blocked: schemas with these features are encoded
//! with `type: any` for the unsupported parts. The decoder is permissive for
//! unknown fields.
//!
//! # Design invariants
//!
//! - **Pure library**: no IO, no env reads, no network, no RNG.
//! - **Fail-closed**: unknown tools or invalid arguments return errors, never guessed calls.
//! - **Deterministic**: same input always produces the same output.
//! - **Schema-preserving**: required/optional, types, enums, nested objects all survive encoding.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod decode;
pub mod encode;
pub mod error;
pub mod roundtrip;
pub mod stream;
pub mod types;

// Re-export public API
pub use decode::decode_calls;
pub use encode::encode_tools;
pub use error::CompactError;
pub use roundtrip::decode_tools;
pub use stream::{StreamDecoder, StreamEvent};
pub use types::{CompactTools, ToolCall, ToolDef};
