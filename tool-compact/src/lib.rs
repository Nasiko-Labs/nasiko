//! Compact tool schemas for LLM prompt-token reduction.
//!
//! Native OpenAI tool definitions are verbose JSON Schema. This crate encodes them into a
//! short, deterministic, human/LLM-readable form for the **router ↔ model** hop, then decodes
//! model text back into validated tool calls for the client (which still sees standard
//! OpenAI-compatible `tool_calls`).
//!
//! # Grammar
//!
//! ## Tool signatures
//!
//! ```text
//! create_calendar_event(title:str, start:datetime, duration_min?:int, attendees?:[str], visibility?:public|private) - Create an event…
//! send_email(to:[str], subject:str, body:str) - Send an email.
//! ```
//!
//! - Required params: `name:type`
//! - Optional params: `name?:type`
//! - Types: `str`, `int`, `num`, `bool`, `null`, `datetime`, `date`, `time`, `uri`,
//!   enums as `a|b`, arrays as `[T]`, nested objects as `{a:str, b?:int}`
//!
//! ## Call markers
//!
//! ```text
//! <<call TOOL_NAME JSON_ARGUMENTS>>
//! ```
//!
//! Example:
//!
//! ```text
//! <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>
//! ```
//!
//! The closing `>>` is recognized only outside JSON strings, so values may contain `>>`.
//!
//! # Fail-closed
//!
//! Unknown tools, invalid types/enums, missing required fields, malformed JSON, and unsupported
//! schema features (`anyOf`, `$ref`, …) return [`CompactError`] — never a guessed call.
//!
//! # No I/O
//!
//! This crate is pure: no environment variables, no network, no filesystem.

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

pub use decode::{CALL_PREFIX, CALL_SUFFIX, decode_calls};
pub use encode::{CALL_INSTRUCTIONS, decode_tools, encode_tools, render_call, render_calls};
pub use error::{CompactError, Result};
pub use stream::StreamDecoder;
pub use types::{CompactToolEntry, CompactTools, ParamSpec, ToolCall, ToolDef, TypeExpr};
pub use validate::validate_arguments;

/// Split `rendered` into chunks at the same relative boundaries as `template_chunks`.
///
/// Used by the eval harness so private decoder cases can be re-chunked for this grammar
/// without hardcoding case IDs.
pub fn split_like(rendered: &str, template_chunks: &[String]) -> Vec<String> {
    if template_chunks.is_empty() {
        return vec![rendered.to_string()];
    }
    let total: usize = template_chunks.iter().map(|c| c.len()).sum();
    if total == 0 || rendered.is_empty() {
        return vec![rendered.to_string()];
    }
    let mut out = Vec::with_capacity(template_chunks.len());
    let mut offset = 0usize;
    let rendered_len = rendered.len();
    for (i, chunk) in template_chunks.iter().enumerate() {
        if i + 1 == template_chunks.len() {
            out.push(rendered[offset..].to_string());
            break;
        }
        let target = ((chunk.len() as u64) * (rendered_len as u64) / (total as u64)) as usize;
        let mut end = (offset + target).min(rendered_len);
        // Snap to char boundary.
        while end < rendered_len && !rendered.is_char_boundary(end) {
            end += 1;
        }
        if end < offset {
            end = offset;
        }
        out.push(rendered[offset..end].to_string());
        offset = end;
    }
    out
}
