//! # nasiko-tool-compact
//!
//! Compact tool-definition format for LLM requests, plus a decoder that converts
//! model output back into standard OpenAI-shaped tool calls.
//!
//! ## Why
//!
//! OpenAI-shaped tool definitions are JSON Schema objects that can consume hundreds
//! of prompt tokens per tool. This crate renders them as terse single-line
//! descriptions and teaches the model to emit calls in a compact grammar
//! (`<<call name {json}>>`), then decodes that grammar back into standard tool-call
//! objects the rest of the router can handle unchanged.
//!
//! ## Grammar (EBNF)
//!
//! ```text
//! call        := "<<call" SP name SP json_object ">>"
//! name        := [A-Za-z_][A-Za-z0-9_.-]*
//! json_object := RFC 8259 JSON object
//! SP          := " "
//! ```
//!
//! - Zero-argument call: `<<call name {}>>`.
//! - Multiple calls, text before/after calls, and plain answers with no call are
//!   all valid; text outside markers is ignored by [`decode_calls`].
//! - A stray `<` or `<<` that never becomes `<<call` is plain text; no error.
//! - The end marker `>>` is located by a string-aware, escape-aware JSON scanner
//!   that tracks `{}`/`[]` depth. A `>>` inside a string argument does **not** end
//!   the call.
//!
//! ## Type mapping table
//!
//! | JSON Schema type / format | Compact notation |
//! |---------------------------|-----------------|
//! | `string` (no format)      | `str`            |
//! | `string` + `format:date-time` | `datetime`  |
//! | `string` + `format:date`  | `date`           |
//! | `integer`                 | `int`            |
//! | `number`                  | `num`            |
//! | `boolean`                 | `bool`           |
//! | `array` of T              | `[T]`            |
//! | `enum` values             | `a\|b\|c`        |
//! | nested `object`           | `{k:T, k2?:T}`  |
//! | nullable (allows null)    | appended `?`     |
//!
//! ## Unsupported schema features (native fallback)
//!
//! Tools using any of the following stay in their original native JSON form with
//! a recorded reason; they are **never silently truncated**:
//!
//! - `$ref`, `oneOf`, `anyOf`, `allOf`, `not`
//! - `patternProperties`, `if`/`then`/`else`
//! - Tuple `items` (array `items` that is not a single schema object)
//! - Numeric constraints: `minimum`, `maximum`, `multipleOf`, `exclusiveMinimum/Maximum`
//! - String constraints: `minLength`, `maxLength`, `pattern`
//! - `default` values
//!
//! ## Failure policy
//!
//! If **any** call in a decoded output is invalid (unknown tool, wrong type,
//! missing required field, extra property under `additionalProperties:false`,
//! out-of-range enum, null where not allowed, etc.), the **entire** decode returns
//! an error. There are no silent partial results.
//!
//! ## Determinism guarantees
//!
//! - `encode_tools` output is deterministic: same input → byte-identical output.
//!   Properties are iterated in the order they appear in the JSON Schema
//!   (`properties` is a JSON object; insertion order is preserved by `serde_json`).
//! - `arguments` in decoded calls preserves the key order emitted by the model
//!   (no re-sorting); the schema validator checks types, not key order.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod decode;
mod encode;
mod scan;
mod validate;

pub use decode::{StreamDecoder, decode_calls};
pub use encode::{CompactTools, NativeTool, decode_tools, encode_tools};
pub use types::{FunctionCall, FunctionDef, ToolCall, ToolDef};

mod types;

use thiserror::Error;

/// All errors the library can produce.
///
/// The library never panics; every invalid situation is represented here.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum Error {
    /// The decoded call references a tool name that was not in the original set.
    #[error("unknown tool: {0:?}")]
    UnknownTool(String),

    /// The decoded arguments do not satisfy the tool's JSON Schema.
    ///
    /// The string contains a human-readable description of the violation.
    #[error("invalid arguments for {tool:?}: {reason}")]
    InvalidArguments {
        /// Name of the tool.
        tool: String,
        /// Human-readable description of the validation failure.
        reason: String,
    },

    /// The call grammar is malformed (not a well-formed `<<call name {...}>>`).
    #[error("invalid call syntax: {0}")]
    InvalidCall(String),

    /// The tool's schema contains a feature not supported by the compact encoder
    /// (should not appear at decode time; only from encode internals).
    #[error("unsupported schema in tool {0:?}: {1}")]
    UnsupportedSchema(String, String),

    /// [`StreamDecoder::finish`] was called while still inside an unterminated call.
    #[error("unterminated call")]
    UnterminatedCall,

    /// The buffered call body exceeded the configured byte limit.
    #[error("call body too large (limit: {limit} bytes)")]
    BodyTooLarge {
        /// The configured limit.
        limit: usize,
    },

    /// JSON nesting depth exceeded the configured limit.
    #[error("JSON nesting depth exceeded (limit: {limit})")]
    DepthExceeded {
        /// The configured limit.
        limit: usize,
    },
}

/// Convenience `Result` type for this crate.
pub type Result<T> = std::result::Result<T, Error>;
