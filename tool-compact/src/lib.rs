//! Compact tool schemas for the Nasiko LLM router.
//!
//! This crate is pure: no IO, no environment reads, no provider code, and no
//! dependency on `nasiko-llm-router`. The router converts its own `ToolDef` /
//! `ToolCall` types to the ones defined here at the seam.
//!
//! # Grammar
//!
//! ## Tool definitions
//!
//! Tools are rendered as one signature line per tool, optionally followed by
//! indented argument notes:
//!
//! ```text
//! TOOLS
//! create_calendar_event(title:str, start:datetime, duration_min?:int, attendees?:[str], visibility?:public|private) - Create an event in the user's calendar.
//!   @title: Event title
//!   @start: Start time, ISO 8601
//! ```
//!
//! * Required arguments are bare (`title:str`); optional arguments carry `?`
//!   (`duration_min?:int`).
//! * Type vocabulary: `str`, `datetime` (string with `format: date-time`),
//!   `int`, `num`, `bool`, `[T]` for arrays, `{k:T, k2?:T}` for nested objects,
//!   and `a|b|c` for enums.
//! * `- text` after the signature is the tool description.
//! * `  @arg: text` lines carry per-argument descriptions, so no schema
//!   information is lost and [`decode_tools`] can rebuild the original schema.
//!
//! ## Calls
//!
//! ```text
//! <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
//! ```
//!
//! The argument payload is a single JSON object, so escaping is JSON's. The
//! closing `>>` is located by walking the JSON with string/escape awareness,
//! never by searching for the literal `>>` — a `>>` inside a string argument is
//! therefore handled correctly.
//!
//! ## Failure policy
//!
//! Decoding validates every call against the *original* JSON Schema and returns
//! an error rather than a guessed call. Unknown tool names produce
//! [`CompactError::UnknownTool`]; missing required fields, unknown fields, type
//! mismatches and enum violations produce [`CompactError::InvalidArguments`].
//!
//! ## Unsupported schema features
//!
//! `oneOf`, `anyOf`, `allOf`, `not`, `$ref`, `patternProperties` and schemas
//! whose root is not `type: object` are rejected by [`encode_tools`] with
//! [`CompactError::Unsupported`]. Callers bypass compaction for such requests
//! and send the native tool definitions unchanged.

#![forbid(unsafe_code)]

mod decode;
mod encode;
mod error;
mod stream;
mod types;
mod validate;

pub use decode::decode_calls;
pub use encode::{CALL_INSTRUCTIONS, decode_tools, encode_tools, render_call};
pub use error::CompactError;
pub use stream::StreamDecoder;
pub use types::{CompactTools, ToolCall, ToolDef};
