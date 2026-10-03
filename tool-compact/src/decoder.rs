use crate::error::DecodeError;
use crate::types::{FunctionCallDelta, ToolCall, ToolCallDelta, ToolDef};
use crate::validator::validate_call;
use serde_json::Value;

/// Decodes model output text into standard OpenAI-compatible tool calls.
///
/// Implements fail-closed decoding:
/// - Plain answers with no calls return `Ok(vec![])`.
/// - Unknown tools return `Err(DecodeError::UnknownTool)`.
/// - Missing required fields, invalid enums, or bad JSON return `Err(DecodeError::InvalidArguments)`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut decoder = StreamDecoder::new(tools.to_vec());
    decoder.feed(text)?;
    decoder.finish()
}

/// An incremental streaming decoder that handles split markers across chunks.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    call_index: i64,
    completed_calls: Vec<ToolCall>,
}

impl StreamDecoder {
    /// Creates a new `StreamDecoder` configured with the expected tool schemas.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buffer: String::new(),
            call_index: 0,
            completed_calls: Vec::new(),
        }
    }

    /// Feed a text chunk from the model stream.
    ///
    /// Returns any newly completed tool call deltas, or a `DecodeError`
    /// if a syntax or schema validation violation is detected.
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<ToolCallDelta>, DecodeError> {
        self.buffer.push_str(chunk);
        let mut deltas = Vec::new();

        loop {
            // Search for start marker `<<call`
            let Some(start_idx) = self.buffer.find("<<call") else {
                // Keep trailing prefix if it might be a split `<<call`
                self.trim_non_marker_prefix();
                break;
            };

            // Discard any conversational text preceding the marker
            if start_idx > 0 {
                self.buffer.drain(..start_idx);
            }

            let after_marker = &self.buffer[6..];
            if after_marker.is_empty() {
                // Waiting for next chunk to determine if whitespace follows
                break;
            }

            // `<<call` must be followed by whitespace
            let first_char = after_marker.chars().next().unwrap();
            if !first_char.is_whitespace() {
                // Not a valid call marker: drain past this occurrence and continue
                self.buffer.drain(..6);
                continue;
            }

            // Find tool name: skip leading whitespace
            let trimmed = after_marker.trim_start();
            let leading_ws = after_marker.len() - trimmed.len();

            let Some(name_end) = trimmed.find(|c: char| c.is_whitespace() || c == '{') else {
                // Tool name still streaming
                break;
            };

            let tool_name = trimmed[..name_end].trim();
            if tool_name.is_empty() {
                return Err(DecodeError::MalformedCall(
                    "empty tool name in call marker".into(),
                ));
            }

            let after_name = &trimmed[name_end..];
            let after_name_trimmed = after_name.trim_start();
            let after_name_ws = after_name.len() - after_name_trimmed.len();

            if after_name_trimmed.is_empty() {
                // Waiting for arguments block
                break;
            }

            if !after_name_trimmed.starts_with('{') {
                return Err(DecodeError::MalformedCall(format!(
                    "expected '{{' after tool name '{tool_name}'"
                )));
            }

            // Find matching closing '}' for the JSON object
            let json_start_offset = 6 + leading_ws + name_end + after_name_ws;
            let json_slice = &self.buffer[json_start_offset..];

            let Some(json_end_rel) = find_matching_brace(json_slice) else {
                // JSON object still streaming
                break;
            };

            let after_json_offset = json_start_offset + json_end_rel + 1;
            let after_json = &self.buffer[after_json_offset..];
            let after_json_trimmed = after_json.trim_start();
            let end_ws = after_json.len() - after_json_trimmed.len();

            if after_json_trimmed.is_empty() || after_json_trimmed == ">" {
                // End marker `>>` still streaming
                break;
            }

            if !after_json_trimmed.starts_with(">>") {
                return Err(DecodeError::MalformedCall(
                    "expected '>>' closing delimiter after JSON arguments".into(),
                ));
            }

            let total_call_len = after_json_offset + end_ws + 2;
            let arguments_str = json_slice[..=json_end_rel].trim().to_string();
            let tool_name_owned = tool_name.to_string();

            // Validate arguments JSON syntax
            let args_val: Value = serde_json::from_str(&arguments_str).map_err(|e| {
                DecodeError::InvalidArguments(format!("malformed JSON arguments: {e}"))
            })?;

            // Validate against the original schemas
            validate_call(&tool_name_owned, &args_val, &self.tools)?;

            // Record completed call
            self.call_index += 1;
            let call_id = format!("call_{}", self.call_index);
            let tool_call = ToolCall::new(&call_id, &tool_name_owned, &arguments_str);
            self.completed_calls.push(tool_call);

            // Yield streaming delta for router integration
            deltas.push(ToolCallDelta {
                index: self.call_index - 1,
                id: Some(call_id),
                kind: Some("function".to_string()),
                function: Some(FunctionCallDelta {
                    name: Some(tool_name_owned),
                    arguments: Some(arguments_str),
                }),
            });

            // Drain this call from the buffer and continue scanning
            self.buffer.drain(..total_call_len);
        }

        Ok(deltas)
    }

    /// Finishes decoding at the end of the stream and returns all completed calls.
    pub fn finish(self) -> Result<Vec<ToolCall>, DecodeError> {
        let trimmed = self.buffer.trim();
        // Check if there is an unclosed tool call at EOF
        if trimmed.starts_with("<<call") {
            return Err(DecodeError::MalformedCall(
                "unclosed tool call at end of stream".into(),
            ));
        }

        Ok(self.completed_calls)
    }

    /// Access all completed calls decoded so far.
    pub fn calls(&self) -> &[ToolCall] {
        &self.completed_calls
    }

    fn trim_non_marker_prefix(&mut self) {
        // Keep at most 5 bytes at the end if they match a prefix of "<<call"
        let prefixes = ["<<cal", "<<ca", "<<c", "<<", "<"];
        for prefix in prefixes {
            if self.buffer.ends_with(prefix) {
                let keep_len = prefix.len();
                let drain_len = self.buffer.len() - keep_len;
                self.buffer.drain(..drain_len);
                return;
            }
        }
        self.buffer.clear();
    }
}

/// Finds the index of the matching closing brace '}' for the opening '{' at index 0.
/// Properly accounts for quotes and backslash escape sequences in strings so that
/// `>>` or `{` / `}` inside strings are never mistaken for markers.
fn find_matching_brace(s: &str) -> Option<usize> {
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        if in_string {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }

    None
}
