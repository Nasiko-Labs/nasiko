//! Compact tool schemas for LLM requests, with a fail-closed decoder for the calls a model
//! writes back.
//!
//! Native tool definitions are JSON Schema, and every request pays for them in prompt tokens.
//! [`encode_tools`] renders the same definitions as a short, line-oriented text the model reads
//! in a system message; the model answers with `<<call NAME {json}>>`; [`decode_calls`] and
//! [`StreamDecoder`] turn that back into standard tool calls, validated against the original
//! schema. [`decode_tools`] parses the compact text back into JSON Schema, which is how the
//! round trip is proven rather than asserted.
//!
//! The grammar, the supported schema subset and the bypass rules are specified in this crate's
//! `README.md`.
//!
//! # Invariants
//!
//! * **Lossless or bypass** — a schema the compact text cannot carry exactly is refused with
//!   [`CompactError::Unsupported`], and the caller sends the native definition instead. Nothing
//!   is dropped to save tokens; descriptions are kept verbatim.
//! * **Fail-closed decoding** — an unknown tool, a missing required argument, a wrong type, an
//!   enum violation, malformed JSON or a broken marker is an error. A call is never guessed,
//!   repaired or completed with invented values, and one bad call voids the whole reply.
//! * **Chunking-invariant** — [`StreamDecoder`] gives the same result however the reply is split.
//! * **Deterministic and pure** — no I/O, no environment, no clock, no randomness.
//!
//! # Config
//!
//! This crate never reads the environment. Whether to compact at all is the caller's decision.

#![forbid(unsafe_code)]
// Request-path code must never panic on model output: these are compiler rules here, the same
// ones `nasiko-compress` applies, not review conventions.
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod decode_tools;
mod encode;
mod error;
mod grammar;
mod json;
mod models;
mod schema;
mod stream;
mod validate;

pub use decode::{decode_calls, decode_reply};
pub use decode_tools::decode_tools;
pub use encode::{INSTRUCTION, encode_tools, render_calls};
pub use error::{ArgumentFault, CallFault, CompactError, UnsupportedFeature};
pub use models::{CompactTools, Decoded, ToolCall, ToolDef};
pub use stream::{MAX_CALL_BYTES, StreamDecoder};
