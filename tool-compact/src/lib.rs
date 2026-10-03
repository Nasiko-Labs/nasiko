//! Compact tool schemas without breaking tool calls.
//!
//! Native tool definitions (JSON Schema inside an OpenAI-shaped `tools` array) are rendered as a
//! short, quote-free, line-based text block the model reads in a system message, and the model
//! answers with `<<call NAME {JSON args}>>` markers instead of native `tool_calls`. A chunk-safe
//! state machine ([`StreamDecoder`]) turns that text back into calls, validating every call
//! against the original schema before anything leaves this crate.
//!
//! ```text
//! ToolDef[] ─► encode_tools ─► CompactTools (instructions + definitions) ─► model
//!                  │ unsupported / not smaller ⇒ Err (caller sends native tools)
//! model text ─► StreamDecoder ─► Text / Call events ─► validated ToolCall[] | DecodeError
//! ```
//!
//! # Invariants
//!
//! * **Fail-closed** — an invalid call never yields a call, and one invalid call fails the whole
//!   decode (no partial results). The first error in stream order sets the error code.
//! * **Lossless where it compacts** — [`decode_tools`] rebuilds every schema from the rendered
//!   text alone; the encoder checks that round trip and refuses ([`EncodeError::RoundTrip`])
//!   rather than ship a lossy encoding.
//! * **Never grows** — compact text that is not smaller than the native tools is refused
//!   ([`EncodeError::NotSmaller`]).
//! * **Deterministic** — no clock, RNG, IO or hash-map iteration; same input, same bytes.
//! * **Chunking-invariant** — every way of splitting the same text into chunks decodes the same.
//! * **Bounded** — call arguments over [`MAX_ARGS_BYTES`] are rejected.
//! * **No environment** — this crate never reads env vars; the caller passes everything in.
//!
//! Grammars, the supported schema subset and the deliberate deviations from JSON Schema are
//! documented in `README.md`.

#![forbid(unsafe_code)]
// Panic-freedom and UTF-8 safety are compiler rules here, not review rules (mirrors
// `nasiko-compress`): no string slicing by index, no unwrap/expect/panic outside tests.
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod calls;
mod encode;
mod error;
mod parse;
mod schema;
mod stream;
mod types;
mod validate;

pub use calls::render_calls;
pub use encode::{INSTRUCTIONS, encode_tools, encode_tools_with};
pub use error::{ArgError, DecodeError, EncodeError, ParseError};
pub use parse::decode_tools;
pub use schema::normalize;
pub use stream::{MAX_ARGS_BYTES, StreamDecoder};
pub use types::{
    CompactTools, Decoded, DescriptionPolicy, EncodeOptions, Event, ToolCall, ToolDef,
};

/// Decode a complete model response into its text and validated calls.
///
/// Implemented on [`StreamDecoder`] (one `feed` + `finish`), so the eval, the round trip and the
/// router all exercise the same state machine.
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded, DecodeError> {
    let mut decoder = StreamDecoder::new(tools).map_err(DecodeError::from_encode)?;
    let mut events = decoder.feed(text)?;
    events.extend(decoder.finish()?);
    Ok(Decoded::from_events(events))
}

/// Decode a complete model response into validated calls only (the brief's shape).
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    decode(text, tools).map(|d| d.calls)
}
