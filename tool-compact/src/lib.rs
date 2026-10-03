//! Compact tool schemas for LLM tool calling.
//!
//! This crate shrinks the prompt cost of tool definitions without changing what the
//! model can express. A verbose JSON Schema tool definition:
//!
//! ```json
//! {
//!   "name": "create_calendar_event",
//!   "description": "Create an event in the user's calendar.",
//!   "parameters": {
//!     "type": "object",
//!     "properties": {
//!       "title": {"type": "string"},
//!       "start": {"type": "string", "format": "date-time"},
//!       "attendees": {"type": "array", "items": {"type": "string"}}
//!     },
//!     "required": ["title", "start"]
//!   }
//! }
//! ```
//!
//! becomes one signature line:
//!
//! ```text
//! create_calendar_event(title:str, start:datetime, attendees?:[str]) - Create an event in the user's calendar.
//! ```
//!
//! # Grammar
//!
//! ```text
//! signature  := name "(" params ")" (" - " description)?
//! params     := param (", " param)*
//! param      := name "?"? ":" type          // trailing "?" marks an optional parameter
//! type       := "str" | "int" | "num" | "bool" | "datetime" | "date"
//!             | "[" type "]"                 // array
//!             | "{" params "}"               // inline object (nesting allowed)
//!             | value ("|" value)*           // string enum, e.g. red|green|blue
//! ```
//!
//! Descriptions are kept verbatim (newlines collapsed) — they are what disambiguate
//! tools for the model, so they are never dropped or shortened.
//!
//! # Calling convention
//!
//! The model emits tool calls inline in its text, one marker per call:
//!
//! ```text
//! <<call tool_name {"param": "value", "other": 3}>>
//! ```
//!
//! Arguments stay JSON: models write JSON reliably, and the savings come from the
//! definitions, not the calls. Markers may be surrounded by plain text, and several
//! calls may appear in one message. A message with no marker is a plain answer.
//!
//! # Fail-closed decoding
//!
//! [`decode_calls`] and [`StreamDecoder`] never guess. Unknown tool names, missing
//! required fields, wrong types, bad enum values, and malformed markers all produce
//! [`Error`] instead of a best-effort call.
//!
//! # Scope
//!
//! Pure library: no IO, no environment reads, no provider code, and no dependency on
//! `nasiko-llm-router`. The router converts its own IR types to [`ToolDef`]/[`ToolCall`]
//! at the seam.

mod decode;
mod encode;
mod schema;
mod stream;
mod types;

pub use decode::decode_calls;
pub use encode::{COMPACT_CALL_FORMAT, encode_tools};
pub use schema::decode_tools;
pub use stream::StreamDecoder;
pub use types::{CompactTools, Error, ToolCall, ToolDef};

#[cfg(test)]
mod tests;
