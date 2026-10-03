//! Compact tool schemas: a smaller prompt representation of OpenAI function tools, and a
//! strict decoder that turns the model's compact calls back into standard tool calls.
//!
//! # Invariants
//!
//! * **Fail-closed** — a schema feature the compact form cannot express exactly is never
//!   approximated; the caller sends native tools instead. A call that does not validate
//!   against the original schema is an error, never a guessed call.
//! * **Deterministic** — the same tools always render to the same bytes. No clock, no RNG.
//! * **Pure** — no I/O, no environment reads, no tokenizer, no provider code. The router
//!   converts its own IR to and from the types here at the seam.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod calls;
mod compact;
mod error;
mod schema;
mod stream;
mod validate;

pub use calls::{MARKER, decode_calls};
pub use compact::{
    CompactTools, HEADER, INSTRUCTION, canonical_schema, decode_tools, encode_tools,
};
pub use error::Error;
pub use schema::{Format, Kind, Node, Object, normalize_tool};
pub use stream::{MAX_CALL_BYTES, StreamDecoder, StreamEvent};
pub use validate::validate_arguments;

use serde_json::Value;

/// One function tool, as a client sent it. `parameters` is the raw JSON Schema; the crate
/// parses it into its own [`Node`] tree and never depends on the router's types.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

/// A decoded tool call. `arguments` is the JSON object the model wrote, already validated
/// against the tool's schema. The router assigns the call id.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}
