//! Compact, reversible tool definitions and a fail-closed decoder for textual tool calls.
//!
//! An LLM request that carries function tools spends prompt tokens on JSON Schema. This crate
//! renders a supported subset of those schemas as one signature line per tool
//! (`search(query:str, limit?:int(1..100)) - Web search`), tells the model to answer with
//! `<<call name {json}>>`, and turns that answer back into tool calls whose arguments have been
//! validated against the **original** schema. Nothing here talks to a model or a network; the
//! router owns the request, this crate owns the two transformations.
//!
//! # Invariants
//!
//! * **Fail-closed.** Every schema the crate cannot carry faithfully is reported as
//!   [`ToolCompactError::UnsupportedSchema`] so the caller can keep the native request. Every
//!   call the model writes is either returned fully validated or rejected with a typed error.
//!   There is no schema-free decoding path and no argument repair.
//! * **Atomic release.** [`StreamDecoder::finish`] returns all validated calls or an error with
//!   no calls. A decoder that has seen an error stays failed.
//! * **Reversible.** For every accepted catalog, `decode_tools(encode_tools(t)) == t`
//!   ([`encode_tools`] re-parses its own output and refuses to return a line that does not
//!   round-trip). This check catches encoder mistakes; it is not evidence that the validator is
//!   correct or that a model understands the notation, which the tests and live runs cover.
//! * **Deterministic.** Pure functions of their input: no clock, RNG, I/O or environment.
//!   Object keys are sorted explicitly, so output does not depend on `serde_json` features.
//! * **Bounded.** Every buffer, count and recursion depth is capped by a constant in
//!   [`limits`], checked before the corresponding allocation or recursion where possible.
//!
//! # Config
//!
//! This crate never reads the environment. Enablement, placement in the prompt and call-id
//! assignment belong to the caller.

#![forbid(unsafe_code)]
// Slicing is confined to `lexeme`, which only slices on byte offsets it has verified to be char
// boundaries via `str::get`. The panicking escape hatches are forbidden outright: a model's output
// is untrusted input and must become an `Err`, never a panic.
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod call;
mod catalog;
mod encode;
mod error;
mod instructions;
mod json;
mod lexeme;
pub mod limits;
mod parse;
mod render;
mod schema;
mod stream;
mod types;
mod validate;

pub use call::{encode_call, validate_call, validate_calls};
pub use encode::{decode_tools, encode_tools};
pub use error::{Result, ToolCompactError};
pub use instructions::{HEADER, INSTRUCTIONS};
pub use json::canonical_json;
pub use stream::{StreamDecoder, decode_calls};
pub use types::{CompactTool, CompactTools, Decoded, ToolCall, ToolDef};
