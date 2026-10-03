use crate::decode::{DecodeError, find_balanced_json_and_closing};
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate_arguments;

/// Incremental streaming decoder that parses compact tool calls across chunk boundaries.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
    calls: Vec<ToolCall>,
    call_counter: usize,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            buffer: String::new(),
            tools: tools.to_vec(),
            calls: Vec::new(),
            call_counter: 1,
        }
    }

    /// Feed an incremental text chunk into the decoder.
    /// Returns newly completed tool calls if any completed in this chunk.
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<ToolCall>, DecodeError> {
        self.buffer.push_str(chunk);
        let mut new_calls = Vec::new();

        while let Some(start_idx) = self.buffer.find("<<call") {
            let after_call = start_idx + "<<call".len();
            if after_call >= self.buffer.len() {
                // Buffer ends right at "<<call", wait for next chunk
                break;
            }

            let rest = &self.buffer[after_call..];
            let trimmed = rest.trim_start();
            if trimmed.len() == rest.len() {
                // No whitespace after "<<call" yet
                break;
            }

            // Look for tool name termination (whitespace or '{')
            let name_end = match trimmed.find(|c: char| c.is_whitespace() || c == '{') {
                Some(idx) => idx,
                None => {
                    // Tool name might not be complete yet
                    break;
                }
            };

            let tool_name = trimmed[..name_end].trim();
            if tool_name.is_empty() {
                break;
            }

            // Find JSON start '{'
            let json_start_rel = match trimmed[name_end..].find('{') {
                Some(idx) => name_end + idx,
                None => {
                    // JSON arguments haven't started yet
                    break;
                }
            };

            let json_start_abs = after_call + (rest.len() - trimmed.len()) + json_start_rel;

            // Check if balanced JSON and closing >> are available
            match find_balanced_json_and_closing(&self.buffer[json_start_abs..]) {
                Some((j_end, m_end)) => {
                    let json_end_abs = json_start_abs + j_end;
                    let marker_end_abs = json_start_abs + m_end;

                    // Verify tool existence
                    let tool_def = match self.tools.iter().find(|t| t.function.name == tool_name) {
                        Some(td) => td,
                        None => {
                            return Err(DecodeError::UnknownTool(tool_name.to_string()));
                        }
                    };

                    let json_str = &self.buffer[json_start_abs..json_end_abs];
                    let parsed_val: serde_json::Value = serde_json::from_str(json_str)
                        .map_err(|e| DecodeError::InvalidArguments(format!("JSON parse error: {}", e)))?;

                    validate_arguments(&parsed_val, tool_def.function.parameters.as_ref())?;

                    let call = ToolCall::new(
                        format!("call_{}", self.call_counter),
                        tool_name,
                        json_str,
                    );
                    self.call_counter += 1;
                    new_calls.push(call.clone());
                    self.calls.push(call);

                    // Drain buffer past the end of this call
                    self.buffer.drain(..marker_end_abs);
                }
                None => {
                    // Call not yet complete, wait for more chunks
                    break;
                }
            }
        }

        Ok(new_calls)
    }

    /// Complete the stream and return all accumulated calls.
    pub fn finish(self) -> Result<Vec<ToolCall>, DecodeError> {
        if let Some(start_idx) = self.buffer.find("<<call") {
            let rest = self.buffer[start_idx..].trim();
            if !rest.is_empty() {
                return Err(DecodeError::MalformedCall(
                    "incomplete call marker at end of stream".to_string(),
                ));
            }
        }
        Ok(self.calls)
    }

    /// Returns references to all decoded calls so far.
    pub fn calls(&self) -> &[ToolCall] {
        &self.calls
    }
}
