//! Stateful streaming decoder for tool calls arriving over chunked SSE streams.

use crate::calls::validate_tool_arguments;
use crate::slice::{safe_slice, safe_slice_from, safe_slice_to};
use crate::types::{CompactError, ToolCall, ToolDef};

/// Events yielded by StreamDecoder as chunks arrive.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Non-call assistant text to stream to client.
    Text(String),
    /// A fully parsed and validated tool call.
    Call(ToolCall),
}

/// Incremental streaming decoder.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    calls: Vec<ToolCall>,
    error: Option<CompactError>,
}

impl StreamDecoder {
    /// Create a new StreamDecoder initialized with tool definitions.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buffer: String::new(),
            calls: Vec::new(),
            error: None,
        }
    }

    /// Push a chunk of streamed text and return any ready events.
    pub fn push(&mut self, chunk: &str) -> Vec<StreamEvent> {
        if self.error.is_some() {
            return Vec::new();
        }
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        loop {
            if self.error.is_some() {
                break;
            }
            // Find `<<call` followed by whitespace
            if let Some((call_idx, marker_len)) = find_call_marker(&self.buffer) {
                // Text before call
                if call_idx > 0 {
                    events.push(StreamEvent::Text(
                        safe_slice_to(&self.buffer, call_idx).to_string(),
                    ));
                    self.buffer.drain(..call_idx);
                }

                // Now buffer starts with `<<call` + whitespace
                let call_text = safe_slice_from(&self.buffer, marker_len);
                let mut char_indices = call_text.char_indices().peekable();

                // Skip any extra whitespace
                while let Some(&(_, c)) = char_indices.peek() {
                    if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
                        char_indices.next();
                    } else {
                        break;
                    }
                }

                // Read tool name
                let name_start = char_indices
                    .peek()
                    .map(|&(i, _)| i)
                    .unwrap_or(call_text.len());
                while let Some(&(_, c)) = char_indices.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        char_indices.next();
                    } else {
                        break;
                    }
                }
                let name_end = char_indices
                    .peek()
                    .map(|&(i, _)| i)
                    .unwrap_or(call_text.len());
                let tool_name = safe_slice(call_text, name_start, name_end).to_string();

                if tool_name.is_empty() {
                    // Incomplete name, wait for more chunks
                    break;
                }

                // Skip whitespace to JSON start
                while let Some(&(_, c)) = char_indices.peek() {
                    if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
                        char_indices.next();
                    } else {
                        break;
                    }
                }

                let Some(&(_, c)) = char_indices.peek() else {
                    // Need more chunks for JSON start
                    break;
                };

                if c != '{' {
                    // Not a valid call start; advance by marker_len to avoid infinite loop
                    events.push(StreamEvent::Text(
                        safe_slice_to(&self.buffer, marker_len).to_string(),
                    ));
                    self.buffer.drain(..marker_len);
                    continue;
                }

                // Parse JSON object tracking brace depth and string escapes
                let json_start_rel = char_indices.peek().map(|&(i, _)| i).unwrap_or(0);
                let json_slice = safe_slice_from(call_text, json_start_rel);
                let mut brace_depth = 0;
                let mut in_str = false;
                let mut in_escape = false;
                let mut json_end_rel = None;

                for (idx, ch) in json_slice.char_indices() {
                    if in_escape {
                        in_escape = false;
                        continue;
                    }
                    if ch == '\\' && in_str {
                        in_escape = true;
                        continue;
                    }
                    if ch == '"' {
                        in_str = !in_str;
                        continue;
                    }
                    if !in_str {
                        if ch == '{' {
                            brace_depth += 1;
                        } else if ch == '}' {
                            brace_depth -= 1;
                            if brace_depth == 0 {
                                json_end_rel = Some(idx + ch.len_utf8());
                                break;
                            }
                        }
                    }
                }

                let Some(json_len) = json_end_rel else {
                    // JSON incomplete, wait for more chunks
                    break;
                };

                let json_str = safe_slice_to(json_slice, json_len);
                let after_json = safe_slice_from(json_slice, json_len);

                // Skip whitespace after JSON before `>>`
                let mut after_indices = after_json.char_indices().peekable();
                while let Some(&(_, c)) = after_indices.peek() {
                    if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
                        after_indices.next();
                    } else {
                        break;
                    }
                }

                let after_ws_idx = after_indices
                    .peek()
                    .map(|&(i, _)| i)
                    .unwrap_or(after_json.len());
                let maybe_closing = safe_slice_from(after_json, after_ws_idx);

                if maybe_closing.len() < 2 {
                    // Could be `<` or empty, wait for `>>`
                    break;
                }

                if !maybe_closing.starts_with(">>") {
                    // Malformed closure
                    self.error = Some(CompactError::InvalidArguments {
                        tool: tool_name.clone(),
                        reason: format!("Expected '>>' after call for tool '{tool_name}'"),
                    });
                    let total_consumed = marker_len + json_start_rel + json_len + after_ws_idx;
                    self.buffer.drain(..total_consumed);
                    continue;
                }

                // Full call found!
                let total_call_len = marker_len + json_start_rel + json_len + after_ws_idx + 2;

                // Validate tool and args
                match self.validate_call(&tool_name, json_str) {
                    Ok(tool_call) => {
                        self.calls.push(tool_call.clone());
                        events.push(StreamEvent::Call(tool_call));
                    }
                    Err(err) => {
                        self.error = Some(err);
                    }
                }

                self.buffer.drain(..total_call_len);
            } else {
                // No `<<call` with whitespace found in buffer.
                let prefixes = ["<<call", "<<cal", "<<ca", "<<c", "<<", "<"];
                let mut matched_prefix_len = 0;
                for p in prefixes {
                    if self.buffer.ends_with(p) {
                        matched_prefix_len = p.len();
                        break;
                    }
                }

                let emit_len = self.buffer.len() - matched_prefix_len;
                if emit_len > 0 {
                    events.push(StreamEvent::Text(
                        safe_slice_to(&self.buffer, emit_len).to_string(),
                    ));
                    self.buffer.drain(..emit_len);
                }
                break;
            }
        }

        events
    }

    /// Take any error that occurred during decoding.
    pub fn take_error(&mut self) -> Option<CompactError> {
        self.error.take()
    }

    /// Check if an error occurred during decoding.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Flush any held-back plain text at the end of the stream.
    pub fn flush(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if !self.has_truncated_call() && !self.buffer.is_empty() {
            let drained = std::mem::take(&mut self.buffer);
            events.push(StreamEvent::Text(drained));
        }
        events
    }

    /// Finish decoding and return all assembled calls, or error.
    pub fn finish(&mut self) -> Result<Vec<ToolCall>, CompactError> {
        if let Some(err) = self.error.take() {
            return Err(err);
        }

        // If buffer still contains unclosed call syntax, fail closed with InvalidArguments
        if self.has_truncated_call() {
            return Err(CompactError::InvalidArguments {
                tool: "".to_string(),
                reason: "Truncated tool call syntax at end of stream".to_string(),
            });
        }

        Ok(std::mem::take(&mut self.calls))
    }

    fn has_truncated_call(&self) -> bool {
        let bytes = self.buffer.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i + 6 <= len {
            if &bytes[i..i + 6] == b"<<call" {
                if i + 6 == len {
                    return true;
                }
                let b = bytes[i + 6];
                if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                    return true;
                }
            }
            i += 1;
        }
        false
    }

    fn validate_call(&self, tool_name: &str, args_json: &str) -> Result<ToolCall, CompactError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| CompactError::UnknownTool(tool_name.to_string()))?;

        let args_val: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| CompactError::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("JSON error: {e}"),
            })?;

        validate_tool_arguments(tool, &args_val)?;

        Ok(ToolCall {
            name: tool_name.to_string(),
            arguments: args_val,
        })
    }
}

fn find_call_marker(buf: &str) -> Option<(usize, usize)> {
    let bytes = buf.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 6 <= len {
        if &bytes[i..i + 6] == b"<<call" && i + 6 < len {
            let b = bytes[i + 6];
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                let mut ws_end = i + 7;
                while ws_end < len
                    && (bytes[ws_end] == b' '
                        || bytes[ws_end] == b'\t'
                        || bytes[ws_end] == b'\r'
                        || bytes[ws_end] == b'\n')
                {
                    ws_end += 1;
                }
                return Some((i, ws_end - i));
            }
        }
        i += 1;
    }
    None
}
