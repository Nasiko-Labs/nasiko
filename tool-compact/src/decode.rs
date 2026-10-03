use crate::error::DecodeError;
use crate::stream::StreamDecoder;
use crate::types::{ToolCall, ToolDef};

/// Decode all tool calls from a complete response text and validate them against
/// the supplied tool definitions.
///
/// Returns:
/// - `Ok(Vec<ToolCall>)` with all decoded calls in order.
/// - `Err(DecodeError::UnknownTool)` if any call names an unknown tool.
/// - `Err(DecodeError::InvalidArguments)` if arguments fail schema validation.
/// - `Err(DecodeError)` on syntax or JSON errors.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut decoder = StreamDecoder::new(tools.to_vec());
    decoder.push_chunk(text);
    decoder.finish()
}

/// Helper function to strip `<<call ...>>` markers from model text response,
/// returning the surrounding human-facing assistant message text.
pub fn strip_call_markers(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;

    while let Some(call_start) = text[cursor..].find("<<call ") {
        let abs_start = cursor + call_start;
        result.push_str(&text[cursor..abs_start]);

        // Find closing >>
        let after_start = abs_start + 7;
        let mut in_string = false;
        let mut escaped = false;
        let mut end_found = None;

        for (idx, ch) in text[after_start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
            } else if ch == '"' {
                in_string = true;
            } else if ch == '>' && text[after_start + idx..].starts_with(">>") {
                end_found = Some(after_start + idx + 2);
                break;
            }
        }

        if let Some(abs_end) = end_found {
            cursor = abs_end;
        } else {
            // Unclosed marker, retain remainder
            result.push_str(&text[abs_start..]);
            cursor = text.len();
            break;
        }
    }

    if cursor < text.len() {
        result.push_str(&text[cursor..]);
    }

    result.trim().to_string()
}
