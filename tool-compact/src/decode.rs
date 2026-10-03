//! One-shot decoding: a complete reply fed to [`StreamDecoder`] as a single chunk, so the
//! streaming and non-streaming paths are the same code.

use crate::error::CompactError;
use crate::models::{Decoded, ToolCall, ToolDef};
use crate::stream::StreamDecoder;

/// Decode every call in a complete reply, validated against `tools`.
///
/// Fails closed: an unknown tool, invalid arguments or a malformed marker anywhere in the reply
/// is an error, and no call is returned. A reply with no calls decodes to an empty list.
///
/// ```
/// use nasiko_tool_compact::{decode_calls, ToolDef, CompactError};
/// use serde_json::json;
///
/// let tool = ToolDef::new("set_mode", None, Some(json!({
///     "type": "object",
///     "properties": {"mode": {"type": "string", "enum": ["on", "off"]}},
///     "required": ["mode"]
/// })));
/// let calls = decode_calls(r#"Sure. <<call set_mode {"mode":"on"}>>"#, &[tool.clone()]).unwrap();
/// assert_eq!(calls[0].arguments_json(), r#"{"mode":"on"}"#);
///
/// let err = decode_calls(r#"<<call set_mode {"mode":"auto"}>>"#, &[tool]).unwrap_err();
/// assert_eq!(err.code(), "invalid_arguments");
/// ```
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    decode_reply(text, tools).map(|decoded| decoded.calls)
}

/// Decode a complete reply into the text around the calls and the calls themselves.
pub fn decode_reply(text: &str, tools: &[ToolDef]) -> Result<Decoded, CompactError> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut prose = decoder.push(text)?;
    let rest = decoder.finish()?;
    prose.push_str(&rest.text);
    Ok(Decoded {
        text: prose,
        calls: rest.calls,
    })
}
