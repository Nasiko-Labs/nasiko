//! Compact tool schemas for LLM routers.
//!
//! Encodes OpenAI-style tool schemas into a short text form and decodes model
//! output back into tool calls, validating every call against the original schema
//! (**fail closed**). See `docs/compact-tools.md` for the full grammar.
//!
//! # Invariants
//!
//! * **Fail-closed** — unknown tool / invalid args ⇒ error, never a guessed call.
//! * **Deterministic** — pure functions; no clock, RNG, I/O, or env reads.
//! * **No provider code** — the router converts to/from its IR at the seam.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decoder;
mod encoder;
mod error;
mod grammar;
mod names;
mod types;
mod validator;

pub use decoder::{StreamDecoder, decode_calls};
pub use encoder::{decode_tools, encode_tools, render_calls, tool_call};
pub use error::CompactError;
pub use grammar::CALL_FORMAT;
pub use names::{assign_aliases, is_valid_compact_ident, parse_gateway_tool_name};
pub use types::{CompactTools, ToolCall, ToolDef};

#[cfg(test)]
mod suite;
