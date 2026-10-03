//! Whole-text decoding, as a thin wrapper over [`StreamDecoder`].

use crate::error::Result;
use crate::stream::StreamDecoder;
use crate::types::{Decoded, Event, ToolCall, ToolDef};

/// Decode complete model output into its text and its calls.
///
/// All or nothing: one bad call fails the whole output, so a caller never acts on half of what
/// the model asked for.
pub fn decode(text: &str, tools: &[ToolDef]) -> Result<Decoded> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut events = decoder.push(text)?;
    events.extend(decoder.finish()?);

    let mut decoded = Decoded::default();
    for event in events {
        match event {
            Event::Text(text) => decoded.text.push_str(&text),
            Event::Call(call) => decoded.calls.push(call),
        }
    }
    Ok(decoded)
}

/// The calls in complete model output, in order. A plain answer gives an empty list.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    decode(text, tools).map(|decoded| decoded.calls)
}
