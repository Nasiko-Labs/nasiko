use crate::error::ToolCompactError;
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate_tool_call;

/// Incremental streaming decoder that extracts and validates tool calls across arbitrary chunk splits.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    call_count: usize,
}

pub const MAX_STREAM_BUFFER_BYTES: usize = 256 * 1024;

impl StreamDecoder {
    /// Create a new stream decoder with the provided tool definitions.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buffer: String::new(),
            call_count: 0,
        }
    }

    /// Push an incremental chunk of text from the model and return any newly completed tool calls.
    pub fn push_chunk(&mut self, chunk: &str) -> Result<Vec<ToolCall>, ToolCompactError> {
        if self.buffer.len() + chunk.len() > MAX_STREAM_BUFFER_BYTES {
            return Err(ToolCompactError::ParseError(
                "Tool call stream buffer exceeded maximum allowed size (256 KB)".to_string(),
            ));
        }
        self.buffer.push_str(chunk);
        self.scan_buffer()
    }

    /// Finalize the stream and return any remaining tool calls.
    ///
    /// Fails closed if the stream terminates with an unclosed tool call invocation.
    pub fn finish(mut self) -> Result<Vec<ToolCall>, ToolCompactError> {
        let calls = self.scan_buffer()?;

        if self.buffer.contains("<<call") {
            return Err(ToolCompactError::ParseError(
                "Unterminated tool call: stream finished before call was closed".to_string(),
            ));
        }

        Ok(calls)
    }

    fn scan_buffer(&mut self) -> Result<Vec<ToolCall>, ToolCompactError> {
        let mut emitted = Vec::new();

        loop {
            let start_idx = match self.buffer.find("<<call") {
                Some(idx) => idx,
                None => {
                    // Check if buffer ends with a prefix of "<<call"
                    let prefixes = ["<<cal", "<<ca", "<<c", "<<", "<"];
                    let mut keep_prefix = None;
                    for prefix in prefixes {
                        if self.buffer.ends_with(prefix) {
                            keep_prefix = Some(prefix);
                            break;
                        }
                    }
                    if let Some(p) = keep_prefix {
                        self.buffer = p.to_string();
                    } else {
                        self.buffer.clear();
                    }
                    break;
                }
            };

            let after_marker = &self.buffer[start_idx + "<<call".len()..];
            if after_marker.is_empty() {
                // Buffer cut off right at "<<call", retain from start_idx
                self.buffer = self.buffer[start_idx..].to_string();
                break;
            }

            let Some(first_char) = after_marker.chars().next() else {
                self.buffer = self.buffer[start_idx..].to_string();
                break;
            };
            if !first_char.is_whitespace() {
                // Not a tool call (e.g. "<<callback")
                self.buffer = self.buffer[start_idx + 1..].to_string();
                continue;
            }

            let trimmed = after_marker.trim_start();
            if trimmed.is_empty() {
                // Cut off in whitespace after "<<call "
                self.buffer = self.buffer[start_idx..].to_string();
                break;
            }

            let sep_pos = match trimmed.find(|c: char| c.is_whitespace() || c == '{') {
                Some(pos) => pos,
                None => {
                    // Cut off in the middle of tool name
                    self.buffer = self.buffer[start_idx..].to_string();
                    break;
                }
            };

            let tool_name = &trimmed[..sep_pos];
            let after_name = &trimmed[sep_pos..];
            let args_start = after_name.trim_start();

            if args_start.is_empty() {
                // Cut off before opening '{'
                self.buffer = self.buffer[start_idx..].to_string();
                break;
            }

            if !args_start.starts_with('{') {
                return Err(ToolCompactError::ParseError(format!(
                    "Expected '{{' for arguments of tool '{tool_name}', got '{}'",
                    args_start.chars().take(10).collect::<String>()
                )));
            }

            // Scan JSON arguments tracking quotes, escapes, and brace depth
            let mut in_string = false;
            let mut escape_next = false;
            let mut brace_depth: usize = 0;
            let mut json_end_byte_idx = None;

            for (byte_idx, ch) in args_start.char_indices() {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                if ch == '\\' && in_string {
                    escape_next = true;
                    continue;
                }
                if ch == '"' {
                    in_string = !in_string;
                    continue;
                }
                if !in_string {
                    if ch == '{' {
                        brace_depth += 1;
                    } else if ch == '}' {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            json_end_byte_idx = Some(byte_idx + ch.len_utf8());
                            break;
                        }
                    }
                }
            }

            let end_of_json = match json_end_byte_idx {
                Some(idx) => idx,
                None => {
                    // JSON arguments not yet closed
                    self.buffer = self.buffer[start_idx..].to_string();
                    break;
                }
            };

            let after_json = &args_start[end_of_json..];
            let after_json_trimmed = after_json.trim_start();

            if after_json_trimmed.is_empty() {
                // Cut off right after '}' or in trailing whitespace
                self.buffer = self.buffer[start_idx..].to_string();
                break;
            }

            if after_json_trimmed == ">" {
                // Cut off between '>' and '>'
                self.buffer = self.buffer[start_idx..].to_string();
                break;
            }

            if !after_json_trimmed.starts_with(">>") {
                return Err(ToolCompactError::ParseError(format!(
                    "Expected '>>' after arguments of tool '{tool_name}', got '{}'",
                    after_json_trimmed.chars().take(10).collect::<String>()
                )));
            }

            // Valid invocation found
            let json_args_str = &args_start[..end_of_json];
            validate_tool_call(tool_name, json_args_str, &self.tools)?;

            self.call_count += 1;
            let call = ToolCall::new(
                format!("call_{}", self.call_count),
                tool_name,
                json_args_str,
            );
            emitted.push(call);

            // Compute exact offset after '>>' in self.buffer
            let ws_len = after_json.len() - after_json_trimmed.len();
            let end_in_args_start = end_of_json + ws_len + 2;
            let after_marker_offset = start_idx + "<<call".len();
            let trimmed_offset = after_marker_offset + (after_marker.len() - trimmed.len());
            let after_name_offset = trimmed_offset + sep_pos;
            let args_start_offset = after_name_offset + (after_name.len() - args_start.len());
            let after_call_idx = args_start_offset + end_in_args_start;

            self.buffer = self.buffer[after_call_idx..].to_string();
        }

        Ok(emitted)
    }
}
