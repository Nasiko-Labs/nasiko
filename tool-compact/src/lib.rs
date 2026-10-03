//! `nasiko-tool-compact` — compact tool schema encoder and decoder.
//!
//! # Grammar
//!
//! The compact tool call marker is:
//!
//! ```text
//! <<call TOOL_NAME JSON_ARGS>>
//! ```
//!
//! Where:
//! - `<<call ` is the opening marker (7 bytes, including trailing space)
//! - `TOOL_NAME` is the function name (ASCII identifier, no spaces)
//! - ` ` separates the name from the JSON argument object
//! - `JSON_ARGS` is a compact (minified) JSON object with the tool arguments
//! - `>>` is the closing delimiter
//!
//! The `>>` sequence inside a JSON string value is naturally safe because JSON
//! strings serialize `>` as-is (it is not a JSON special character), and the
//! parser uses a JSON-aware state machine to find the end of the argument
//! object before looking for `>>`.
//!
//! # Example
//!
//! ```text
//! <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
//! ```
//!
//! Multiple calls may appear in a single response, separated by arbitrary text:
//!
//! ```text
//! Sure! <<call send_email {"to":["a@b.com"],"subject":"Hi","body":"Hello"}>> Done.
//! <<call create_calendar_event {"title":"Review","start":"2026-10-05T09:00:00Z"}>>
//! ```
//!
//! # Compact schema format
//!
//! [`encode_tools`] produces a `CompactTools` struct that represents the
//! full set of tool definitions in a compact, LLM-friendly textual format,
//! along with the original definitions for validation during decoding.
//!
//! # Supported JSON Schema features
//!
//! - `type`: `string`, `number`, `integer`, `boolean`, `array`, `object`
//! - `properties` with nested schemas
//! - `required` array
//! - `enum` on string properties
//! - `description` on each property
//! - `items` for array types
//!
//! # Unsupported features (fail-closed)
//!
//! The following JSON Schema features are not supported. Tools using them
//! will have their schemas encoded verbatim (bypassing compaction) rather
//! than silently dropping the unsupported clause:
//!
//! - `oneOf`, `anyOf`, `allOf`, `not`
//! - `$ref` / `$schema` / `$defs`
//! - `additionalProperties` with schema (boolean is OK)
//! - `patternProperties`, `pattern`
//! - `minimum`, `maximum`, `minLength`, `maxLength`, `minItems`, `maxItems`
//! - `format` (accepted and ignored)
//! - `default` values
//! - `nullable`

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod decode;
mod encode;
mod error;
mod schema;
mod stream;
mod types;
mod validate;

pub use decode::decode_calls;
pub use encode::encode_tools;
pub use error::{DecodeError, EncodeError};
pub use schema::decode_tools;
pub use stream::StreamDecoder;
pub use types::{CompactTools, FunctionDef, ToolCall, ToolDef};
