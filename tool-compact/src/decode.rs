//! Decode compact model output into standard tool calls.
//!
//! The decoder parses `<<call tool_name {json args}>>` markers from model output,
//! validates each call against the original schema, and returns structured `ToolCall`s.
//!
//! # Grammar
//!
//! ```text
//! output      := (text | call_marker)*
//! call_marker := "<<call" WS tool_name WS json_object ">>"
//! tool_name   := [a-zA-Z_][a-zA-Z0-9_]*
//! json_object := valid JSON object (balanced braces; ">>" inside strings is safe
//!                because JSON string escaping handles it)
//! ```

use serde_json::Value;

use crate::error::DecodeError;
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate_call;

/// Marker that opens a tool call.
const CALL_OPEN: &str = "<<call ";
/// Marker that closes a tool call.
const CALL_CLOSE: &str = ">>";

/// Decode all tool calls from a complete model output string.
///
/// Returns an empty `Vec` if no `<<call ...>>` markers are present (plain text answer).
/// Decode all tool calls and extract remaining text from a complete model output string.
///
/// Returns decoded tool calls and any text outside of `<<call ...>>` markers.
/// Returns an error if any marker is malformed or any call fails validation.
pub fn decode_calls_and_text(text: &str, tools: &[ToolDef]) -> Result<(Vec<ToolCall>, String), DecodeError> {
    let mut calls = Vec::new();
    let mut remaining = String::new();
    let mut pos = 0;

    while pos < text.len() {
        if let Some(start) = text[pos..].find(CALL_OPEN) {
            let marker_start = pos + start + CALL_OPEN.len();
            remaining.push_str(&text[pos..pos + start]);
            if let Some(call) = parse_call_at(&text[marker_start..], tools)? {
                calls.push(call.0);
                pos = marker_start + call.1;
            } else {
                remaining.push_str(CALL_OPEN);
                pos = marker_start;
            }
        } else {
            remaining.push_str(&text[pos..]);
            break;
        }
    }

    Ok((calls, remaining.trim().to_string()))
}

/// Decode all tool calls from a complete model output string.
///
/// Returns an empty `Vec` if no `<<call ...>>` markers are present (plain text answer).
/// Returns an error if any marker is malformed or any call fails validation.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    decode_calls_and_text(text, tools).map(|(calls, _)| calls)
}

/// Parse a single call starting right after `<<call `.
/// Returns `(ToolCall, bytes_consumed)` or `None` if the marker is incomplete.
fn parse_call_at(text: &str, tools: &[ToolDef]) -> Result<Option<(ToolCall, usize)>, DecodeError> {
    let text = text.trim_start();

    // Extract tool name (alphanumeric + underscore).
    let name_end = text
        .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .unwrap_or(text.len());

    if name_end == 0 {
        return Err(DecodeError::MalformedCall(
            "expected tool name after <<call".into(),
        ));
    }

    let tool_name = &text[..name_end];
    let rest = text[name_end..].trim_start();

    // Find the tool definition.
    let tool = tools
        .iter()
        .find(|t| t.name == tool_name)
        .ok_or_else(|| DecodeError::UnknownTool(tool_name.to_string()))?;

    // Parse JSON arguments. We need to find the balanced `{...}` block.
    if !rest.starts_with('{') {
        // Check for empty args with immediate close.
        if rest.starts_with(CALL_CLOSE) {
            let consumed = name_end + (text.len() - text[name_end..].len() - rest.len())
                + CALL_CLOSE.len();
            let call = ToolCall {
                name: tool_name.to_string(),
                arguments: Value::Object(serde_json::Map::new()),
            };
            validate_call(tool, &call.arguments)?;
            return Ok(Some((call, consumed)));
        }
        return Err(DecodeError::MalformedCall(format!(
            "expected JSON object after tool name '{tool_name}'"
        )));
    }

    // Find the end of the JSON object by counting braces, respecting strings.
    let json_end = find_json_end(rest)?;
    let json_str = &rest[..json_end];

    // Parse the JSON.
    let arguments: Value = serde_json::from_str(json_str).map_err(|e| DecodeError::InvalidJson {
        tool: tool_name.to_string(),
        reason: e.to_string(),
    })?;

    if !arguments.is_object() {
        return Err(DecodeError::InvalidJson {
            tool: tool_name.to_string(),
            reason: "arguments must be a JSON object".into(),
        });
    }

    // Validate against schema.
    validate_call(tool, &arguments)?;

    // Expect >> after the JSON.
    let after_json = rest[json_end..].trim_start();
    let close_len = if after_json.starts_with(CALL_CLOSE) {
        CALL_CLOSE.len()
    } else {
        0 // Tolerate missing close marker at end of output.
    };

    let total_consumed =
        (text.len() - rest.len()) + json_end + (rest[json_end..].len() - after_json.len()) + close_len;

    Ok(Some((
        ToolCall {
            name: tool_name.to_string(),
            arguments,
        },
        total_consumed,
    )))
}

/// Find the end of a JSON object by counting balanced braces, respecting string escaping.
fn find_json_end(text: &str) -> Result<usize, DecodeError> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let bytes = text.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if b == b'\\' && in_string {
            escape = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Ok(i + 1);
            }
        }
    }

    Err(DecodeError::MalformedCall(
        "unterminated JSON object in call arguments".into(),
    ))
}

/// Result of a completed stream decode.
#[derive(Debug, Clone)]
pub struct DecoderResult {
    /// Decoded tool calls.
    pub calls: Vec<ToolCall>,
    /// Errors encountered during decoding.
    pub errors: Vec<DecodeError>,
    /// Non-call text fragments from the model output.
    pub text_parts: Vec<String>,
}

/// Events emitted by the `StreamDecoder` as chunks arrive.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A fragment of plain text (not part of a call marker).
    Text(String),
    /// A complete, validated tool call.
    Call(ToolCall),
    /// A decoding error for one call.
    Error(DecodeError),
}

/// Incremental stream decoder that handles `<<call ...>>` markers split across chunks.
///
/// Feed chunks via [`push`](StreamDecoder::push) and collect events. Call
/// [`finish`](StreamDecoder::finish) when the stream ends to flush any buffered state.
///
/// # Example
///
/// ```rust
/// use nasiko_tool_compact::{StreamDecoder, ToolDef};
///
/// let tools = vec![ToolDef {
///     name: "greet".into(),
///     description: None,
///     parameters: Some(serde_json::json!({
///         "type": "object",
///         "properties": {"name": {"type": "string"}},
///         "required": ["name"]
///     })),
/// }];
///
/// let mut decoder = StreamDecoder::new(tools);
/// let events1 = decoder.push("Hello! <<ca");
/// let events2 = decoder.push(r#"ll greet {"name":"Alice"}>>"#);
/// let result = decoder.finish();
/// assert_eq!(result.calls.len(), 1);
/// ```
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    calls: Vec<ToolCall>,
    errors: Vec<DecodeError>,
    text_parts: Vec<String>,
}

impl StreamDecoder {
    /// Create a new stream decoder with the given tool definitions.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buffer: String::new(),
            calls: Vec::new(),
            errors: Vec::new(),
            text_parts: Vec::new(),
        }
    }

    /// Feed a chunk of model output. Returns any events that can be determined from
    /// the accumulated buffer.
    pub fn push(&mut self, chunk: &str) -> Vec<StreamEvent> {
        self.buffer.push_str(chunk);
        self.drain_buffer()
    }

    /// Finish the stream. Any remaining buffered text that isn't a complete call
    /// marker is emitted as plain text. Returns the final result.
    pub fn finish(mut self) -> DecoderResult {
        // If there's a partial `<<call` at the end, it's malformed.
        if self.buffer.contains("<<call") && !self.buffer.contains(CALL_CLOSE) {
            self.errors.push(DecodeError::MalformedCall(
                "unterminated <<call marker at end of stream".into(),
            ));
        } else if !self.buffer.is_empty() {
            // Remaining text is plain output.
            self.text_parts.push(std::mem::take(&mut self.buffer));
        }

        DecoderResult {
            calls: self.calls,
            errors: self.errors,
            text_parts: self.text_parts,
        }
    }

    /// Try to extract complete events from the buffer.
    fn drain_buffer(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        loop {
            // Look for a complete <<call ...>> in the buffer.
            let Some(start) = self.buffer.find(CALL_OPEN) else {
                // No call marker found. Check if we might have a partial marker at the end.
                // If the buffer ends with a prefix of "<<call ", keep it buffered.
                let keep_from = partial_marker_start(&self.buffer);
                if keep_from < self.buffer.len() {
                    let text = self.buffer[..keep_from].to_string();
                    if !text.is_empty() {
                        events.push(StreamEvent::Text(text.clone()));
                        self.text_parts.push(text);
                    }
                    let remaining = self.buffer[keep_from..].to_string();
                    self.buffer = remaining;
                } else if !self.buffer.is_empty() {
                    // Buffer is entirely a potential partial marker — keep buffering.
                }
                break;
            };

            // Emit any text before the marker.
            if start > 0 {
                let text = self.buffer[..start].to_string();
                events.push(StreamEvent::Text(text.clone()));
                self.text_parts.push(text);
            }

            let after_open = start + CALL_OPEN.len();

            // Look for the close marker, respecting JSON string boundaries.
            let rest = &self.buffer[after_open..];
            match find_call_close(rest) {
                Some(close_pos) => {
                    let call_content = &self.buffer[after_open..after_open + close_pos];
                    let consumed_end = after_open + close_pos + CALL_CLOSE.len();

                    // Parse the call.
                    match parse_call_content(call_content, &self.tools) {
                        Ok(call) => {
                            events.push(StreamEvent::Call(call.clone()));
                            self.calls.push(call);
                        }
                        Err(e) => {
                            events.push(StreamEvent::Error(e.clone()));
                            self.errors.push(e);
                        }
                    }

                    self.buffer = self.buffer[consumed_end..].to_string();
                }
                None => {
                    // Close marker not yet received — keep everything from the open
                    // marker onward in the buffer and wait for more data.
                    if start > 0 {
                        self.buffer = self.buffer[start..].to_string();
                    }
                    break;
                }
            }
        }

        events
    }
}

/// Find the close marker `>>` that matches the open marker, respecting JSON strings.
fn find_call_close(text: &str) -> Option<usize> {
    // We need to find >> that is not inside a JSON string.
    let mut in_string = false;
    let mut escape = false;
    let mut brace_depth = 0i32;
    let bytes = text.as_bytes();

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        if escape {
            escape = false;
            i += 1;
            continue;
        }
        if b == b'\\' && in_string {
            escape = true;
            i += 1;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        if in_string {
            i += 1;
            continue;
        }

        if b == b'{' {
            brace_depth += 1;
        } else if b == b'}' {
            brace_depth -= 1;
        }

        // Only match >> outside of JSON (brace_depth == 0) or after JSON is complete.
        if b == b'>' && i + 1 < bytes.len() && bytes[i + 1] == b'>' && brace_depth == 0 {
            return Some(i);
        }

        i += 1;
    }

    None
}

/// Parse the content between `<<call ` and `>>`.
fn parse_call_content(content: &str, tools: &[ToolDef]) -> Result<ToolCall, DecodeError> {
    let content = content.trim();

    // Split into tool name and JSON args.
    let name_end = content
        .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .unwrap_or(content.len());

    if name_end == 0 {
        return Err(DecodeError::MalformedCall(
            "missing tool name in call".into(),
        ));
    }

    let tool_name = &content[..name_end];

    // Find the tool.
    let tool = tools
        .iter()
        .find(|t| t.name == tool_name)
        .ok_or_else(|| DecodeError::UnknownTool(tool_name.to_string()))?;

    let args_str = content[name_end..].trim();

    let arguments = if args_str.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(args_str).map_err(|e| DecodeError::InvalidJson {
            tool: tool_name.to_string(),
            reason: e.to_string(),
        })?
    };

    if !arguments.is_object() {
        return Err(DecodeError::InvalidJson {
            tool: tool_name.to_string(),
            reason: "arguments must be a JSON object".into(),
        });
    }

    validate_call(tool, &arguments)?;

    Ok(ToolCall {
        name: tool_name.to_string(),
        arguments,
    })
}

/// Find the start position of a potential partial `<<call ` marker at the end of text.
/// Returns `text.len()` if no partial marker is found.
fn partial_marker_start(text: &str) -> usize {
    let marker = CALL_OPEN.as_bytes();
    let text_bytes = text.as_bytes();

    // Check if the text ends with any prefix of "<<call " (length 1..marker.len()-1).
    for prefix_len in (1..marker.len()).rev() {
        if text_bytes.len() >= prefix_len
            && &text_bytes[text_bytes.len() - prefix_len..] == &marker[..prefix_len]
        {
            return text_bytes.len() - prefix_len;
        }
    }

    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
                        "duration_min": {"type": "integer"},
                        "attendees": {"type": "array", "items": {"type": "string"}},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
                name: "send_email".into(),
                description: Some("Send an email".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"},
                        "cc": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        ]
    }

    // ─── Batch decode tests ─────────────────────────────────────────────

    #[test]
    fn decode_single_call() {
        let tools = test_tools();
        let text = r#"<<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Review");
    }

    #[test]
    fn decode_multiple_calls() {
        let tools = test_tools();
        let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"Build","body":"Green."}>> <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }

    #[test]
    fn decode_no_calls_plain_text() {
        let tools = test_tools();
        let text = "I can't help with that, sorry!";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn decode_text_mixed_with_calls() {
        let tools = test_tools();
        let text = r#"Sure! I'll create that event for you. <<call create_calendar_event {"title":"Meeting","start":"2026-10-05T10:00:00+05:30"}>> Done!"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
    }

    #[test]
    fn decode_unknown_tool_fails() {
        let tools = test_tools();
        let text = r#"<<call nonexistent_tool {"x": 1}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::UnknownTool(ref name) if name == "nonexistent_tool"));
    }

    #[test]
    fn decode_missing_required_field_fails() {
        let tools = test_tools();
        let text = r#"<<call create_calendar_event {"title":"Review"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::MissingRequired { .. }));
    }

    #[test]
    fn decode_invalid_enum_fails() {
        let tools = test_tools();
        let text = r#"<<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidArgument { .. }));
    }

    #[test]
    fn decode_handles_chevrons_inside_json_strings() {
        let tools = vec![ToolDef {
            name: "echo".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": { "text": {"type": "string"} },
                "required": ["text"]
            })),
        }];
        let text = r#"<<call echo {"text":"use >> operator"}>>"#;
        // The >> inside the JSON string is handled by JSON string parsing.
        // The close marker is the >> after the JSON object closes.
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["text"], "use >> operator");
    }

    // ─── Stream decoder tests ───────────────────────────────────────────

    #[test]
    fn stream_single_chunk() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        decoder.push(r#"<<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30"}>>"#);
        let result = decoder.finish();
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.calls[0].name, "create_calendar_event");
        assert!(result.errors.is_empty());
    }

    #[test]
    fn stream_marker_split_across_chunks() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        decoder.push("<<ca");
        decoder.push(r#"ll create_calendar_event {"title":"Ret"#);
        decoder.push(r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#);
        decoder.push(">");
        let result = decoder.finish();
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.calls[0].name, "create_calendar_event");
        assert_eq!(result.calls[0].arguments["title"], "Retro");
    }

    #[test]
    fn stream_text_before_and_after_call() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        decoder.push("Sure! ");
        decoder.push(r#"<<call create_calendar_event {"title":"X","start":"2026-10-05T10:00:00+05:30"}>>"#);
        decoder.push(" Done!");
        let result = decoder.finish();
        assert_eq!(result.calls.len(), 1);
        assert!(result.text_parts.iter().any(|t| t.contains("Sure!")));
    }

    #[test]
    fn stream_no_calls() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        decoder.push("Just a plain text response.");
        let result = decoder.finish();
        assert!(result.calls.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn stream_multiple_calls_across_chunks() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        decoder.push(r#"<<call send_email {"to":["a@b.com"],"subject":"Hi","body":"Hello"}>>"#);
        decoder.push(r#"<<call create_calendar_event {"title":"M","start":"2026-10-05T10:00:00+05:30"}>>"#);
        let result = decoder.finish();
        assert_eq!(result.calls.len(), 2);
    }

    #[test]
    fn stream_unknown_tool_produces_error() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        decoder.push(r#"<<call fake_tool {"x":1}>>"#);
        let result = decoder.finish();
        assert!(result.calls.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(matches!(result.errors[0], DecodeError::UnknownTool(_)));
    }

    // ─── Partial marker detection ───────────────────────────────────────

    #[test]
    fn partial_marker_detection() {
        assert_eq!(partial_marker_start("hello"), 5); // no partial
        assert_eq!(partial_marker_start("hello<"), 5); // "<" is prefix of "<<call "
        assert_eq!(partial_marker_start("hello<<"), 5);
        assert_eq!(partial_marker_start("hello<<c"), 5);
        assert_eq!(partial_marker_start("hello<<ca"), 5);
        assert_eq!(partial_marker_start("hello<<cal"), 5);
        assert_eq!(partial_marker_start("hello<<call"), 5);
    }
}
