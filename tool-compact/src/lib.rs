//! Compact tool schemas: render OpenAI function tools as one-line signatures, and decode the
//! model's `<<call NAME {json}>>` replies back into validated tool calls.
//!
//! A JSON-Schema tool list is re-sent on every turn of an agent loop and is mostly punctuation.
//! [`encode_tools`] turns each tool into a line such as
//!
//! ```text
//! create_calendar_event(title:str "Event title", start:datetime, attendees?:[str]) - Create an event.
//! ```
//!
//! and appends one call instruction. [`decode_calls`] (or [`StreamDecoder`] for streamed output)
//! parses the model's reply and validates every call against the original schema. The grammar is
//! documented in `README.md`.
//!
//! # Invariants
//!
//! * **Fail-closed** — a schema this crate cannot represent *exactly* yields
//!   [`CompactTools::Bypass`] so the caller sends native tools; a call that is unknown, malformed
//!   or fails validation is an error, never a best guess.
//! * **Lossless** — for every schema that is compacted, [`decode_tools`] reproduces it (up to the
//!   normalizations listed in `README.md`).
//! * **Deterministic** — output depends only on the input. Fields are ordered by rule (required
//!   in `required` order, then optional by name), never by hash-map iteration.
//! * **No I/O, no environment, no clock, no RNG.**
//! * **UTF-8 safe** — no byte-index string slicing; enforced by `clippy::string_slice`.
//! * **Bounded** — schema and grammar nesting is capped at [`MAX_DEPTH`].
//!
//! # Scope
//!
//! This crate does not depend on the router. It defines its own [`ToolDef`] and [`ToolCall`];
//! converting from the router IR is the caller's job.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod calls;
mod cursor;
mod encode;
mod error;
mod formats;
mod schema;
mod signature;
mod types;

pub use calls::{Decoded, StreamDecoder, decode_calls, render_call};
pub use encode::{
    BALANCED_DATETIME_NOTE, BALANCED_INSTRUCTION, BypassReason, CALL_INSTRUCTION, CompactTools,
    DATETIME_NOTE, EncodeOptions, Instructions, NOTES, TERSE_CALL_INSTRUCTION, TERSE_DATETIME_NOTE,
    TERSE_NOTES, decode_tools, encode_tools, encode_tools_with, encode_tools_with_choice,
    tool_choice_allows_compact,
};
pub use error::{Error, Result};
pub use schema::MAX_DEPTH;
pub use types::{ToolCall, ToolDef};
