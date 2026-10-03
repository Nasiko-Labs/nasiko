//! Compact tool schemas, and a strict decoder for the calls a model writes back.
//!
//! Native tool definitions are verbose JSON Schema. [`encode_tools`] renders them as one
//! signature line per tool plus a short call contract; the model replies with
//! `<<call name {json}>>` markers, and [`decode_calls`] / [`StreamDecoder`] turn those back into
//! standard OpenAI-shaped calls, validating every call against the **original** schema.
//!
//! # Invariants
//!
//! * **Fail-closed** — an unknown tool, a missing required argument, a wrong type, a value
//!   outside an enum, an unknown argument, malformed JSON or a malformed marker is an
//!   [`Error`]. Nothing is guessed, repaired or coerced, and one bad call fails the whole reply.
//! * **Bypass, never approximate** — a schema feature the compact form cannot express exactly
//!   makes [`encode_tools`] return [`Error::UnsupportedSchema`]; the caller sends native tools.
//! * **Faithful arguments** — [`ToolCall::arguments`] is the model's own JSON text, unaltered.
//! * **Deterministic** — pure string processing. No clock, no RNG, no I/O, no env reads.
//!
//! # Definition grammar
//!
//! ```text
//! line     := name "(" [field ("," " "? field)*] ")" [" - " tool_description]
//! field    := name ["?"] ":" type ["=" json_default] [" " json_string_description]
//! type     := "str" | "int" | "num" | "bool"
//!           | "datetime" | "date" | "time" | "email" | "uri" | "uuid"   (string + format)
//!           | "[" type "]"                                              (array of type)
//!           | "{" [field ("," " "? field)*] "}"                          (nested object)
//!           | literal ("|" literal)*                                    (enum)
//! literal  := bare_word | json_string | json_integer
//! name     := [A-Za-z0-9_.-]+
//! ```
//!
//! `?` marks an optional field; required fields come first. A string enum value is written bare
//! only when it is an identifier that is not a type keyword, and a single-value enum is always
//! quoted, so enums and types never collide.
//!
//! # Call grammar
//!
//! ```text
//! output := (text | call)*
//! call   := "<<call" ws+ name ws* json_object ws* ">>"
//! ```
//!
//! No escaping is needed: the argument object is scanned with a string-aware bracket matcher,
//! so `>>`, `}` or `<<call` inside a JSON string are inert. `<<call` followed by a
//! non-whitespace character (`<<caller`) is ordinary text.
//!
//! # Supported schema subset
//!
//! Root `type: object` with `properties` / `required`; property types `string` (optionally with
//! `format` date-time, date, time, email, uri, uuid), `integer`, `number`, `boolean`, `array`
//! with `items`, nested `object` with `properties`; all-string or all-integer `enum`;
//! `description` and `default` on properties; `additionalProperties: false`; `title` and
//! `$schema` (dropped). **Unsupported → bypass**: `$ref`/`$defs`, `oneOf`/`anyOf`/`allOf`/`not`,
//! `const`, type unions (`["string","null"]`), numeric/length/pattern constraints, other
//! formats, free-form objects, open `additionalProperties`, descriptions on array items, and
//! property names that are not plain identifiers.
//!
//! `format` is a hint to the model; as in JSON Schema, validation does not enforce it.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod encode;
mod schema;

pub use decode::{Decoded, MAX_CALL_BYTES, StreamDecoder, StreamEvent, decode, decode_calls};
pub use encode::{CompactTools, call, decode_tools, encode_tools, render_calls};

use serde_json::Value;

/// Opens a call in model output.
pub const CALL_OPEN: &str = "<<call";
/// Closes a call in model output.
pub const CALL_CLOSE: &str = ">>";

/// A function tool. Mirrors the router's `FunctionDef` without depending on the router.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema for the arguments.
    pub parameters: Option<Value>,
}

/// A decoded call. The caller assigns the call id.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    /// The arguments as a JSON object string (OpenAI's `function.arguments` contract).
    pub arguments: String,
}

impl ToolCall {
    pub fn arguments_value(&self) -> serde_json::Result<Value> {
        serde_json::from_str(&self.arguments)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The tool set cannot be compacted exactly; send native tools instead.
    #[error("tool `{tool}` cannot be compacted: {reason}")]
    UnsupportedSchema { tool: String, reason: String },
    /// The model named a tool that was not offered.
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    /// The arguments are not JSON, not an object, or do not satisfy the tool's schema.
    #[error("invalid arguments for `{tool}`: {reason}")]
    InvalidArguments { tool: String, reason: String },
    /// A `<<call` marker that does not follow the call grammar, or never closes.
    #[error("malformed call: {0}")]
    MalformedCall(String),
    /// Compact definition text that does not follow the definition grammar.
    #[error("malformed compact definition: {0}")]
    MalformedDefinition(String),
}

impl Error {
    /// Stable machine-readable code (the eval's `{"error": …}` vocabulary).
    pub fn code(&self) -> &'static str {
        match self {
            Error::UnsupportedSchema { .. } => "unsupported_schema",
            Error::UnknownTool(_) => "unknown_tool",
            Error::InvalidArguments { .. } => "invalid_arguments",
            Error::MalformedCall(_) => "malformed_call",
            Error::MalformedDefinition(_) => "malformed_definition",
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests;
