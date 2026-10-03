//! `nasiko-tool-compact` — pure library for compact tool schemas.
//!
//! # Purpose
//!
//! Tool definitions sent to LLMs are verbose JSON Schema objects. This crate encodes
//! them into a terse one-liner grammar that an LLM can call with far fewer prompt tokens,
//! and decodes the model's compact reply back into standard OpenAI-shaped tool calls.
//!
//! # Grammar  (BNF)
//!
//! ```text
//! call     ::= "<<call" SP name SP json_object ">>"
//! name     ::= [a-zA-Z0-9_]+
//! json_object ::= '{' ... '}'   (well-formed JSON object; ">>" terminates only
//!                                  when outside all string literals)
//! ```
//!
//! A `>>` that appears **inside a JSON string value** is treated as literal text, not a
//! terminator, because the decoder tracks JSON string escaping state. The marker `<<call`
//! can span stream chunks; the decoder buffers the in-progress marker.
//!
//! # Fail-closed invariant
//!
//! Every path that cannot produce a valid, schema-conforming call returns an error. The
//! crate never guesses a call, truncates a required field, or silently skips a violation.

pub mod decode;
pub mod encode;
pub mod types;

pub use decode::{DecodeResult, StreamDecoder, decode_calls};
pub use encode::{CompactTools, encode_tools};
pub use types::{FunctionCall, FunctionDef, ToolCall, ToolDef};
