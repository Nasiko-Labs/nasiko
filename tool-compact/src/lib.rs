//! Compact tool definitions for LLM prompts, plus a fail-closed decoder.
//!
//! * [`encode_tools`] renders native tool schemas as one short signature per tool and tells the
//!   model to call tools with `<<call name {json args}>>`.
//! * [`decode_calls`] / [`StreamDecoder`] turn the model's text back into standard calls, validating
//!   every call against the **original** schema. Anything unknown or invalid is an [`Error`].
//!
//! Pure library: no IO, no env, no provider code, and no dependency on the router. The router
//! converts its own types at the seam and assigns call ids. See `GRAMMAR.md`.

mod decode;
mod encode;
mod error;
mod validate;

pub use decode::{StreamDecoder, decode_calls};
pub use encode::{CompactTools, decode_tools, encode_tools};
pub use error::{Error, Result};

use serde_json::Value;

/// A native tool definition (the router's `FunctionDef`, without the OpenAI envelope).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema for the arguments.
    pub parameters: Option<Value>,
}

/// A decoded call. `arguments` is the model's own JSON text for the argument object, passed
/// through unmodified (OpenAI shape). The router assigns the call `id`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// Characters allowed in a tool name (and so in a marker's name field).
pub(crate) fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}
