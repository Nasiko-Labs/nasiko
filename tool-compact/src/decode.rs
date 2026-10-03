//! Batch decoder: one chunk through the streaming scanner, so the two paths cannot diverge.

use crate::stream::StreamDecoder;
use crate::types::{DecodeError, ToolCall, ToolDef};

/// Decode every `<<call ...>>` in `text`, validating against `tools`.
///
/// All or nothing: any invalid, unknown or truncated call makes the whole decode an error.
/// Text with no call is `Ok(vec![])`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = decoder.push(text)?;
    calls.extend(decoder.finish()?);
    Ok(calls)
}
