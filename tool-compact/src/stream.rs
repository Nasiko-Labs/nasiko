use crate::error::DecodeError;
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate_call;

const CALL_PREFIX: &str = "<<call ";

/// Incremental streaming decoder for model responses containing compact tool calls.
/// Emits complete ToolCalls as soon as parsed across incoming chunk boundaries,
/// while isolating surrounding conversational text.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
    emitted_calls: Vec<ToolCall>,
    call_index: usize,
    latched_error: Option<DecodeError>,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            buffer: String::new(),
            tools,
            emitted_calls: Vec::new(),
            call_index: 0,
            latched_error: None,
        }
    }

    /// Process an incoming chunk of model output. Returns any complete ToolCalls
    /// parsed during this chunk.
    pub fn push_chunk(&mut self, chunk: &str) -> Vec<ToolCall> {
        self.push_chunk_with_text(chunk).1
    }

    /// Process an incoming chunk of model output, returning both:
    /// - Plain text outside of call markers (e.g. conversational text preceding or following calls)
    /// - Any complete ToolCalls parsed during this chunk
    pub fn push_chunk_with_text(&mut self, chunk: &str) -> (Option<String>, Vec<ToolCall>) {
        if self.latched_error.is_some() {
            return (None, Vec::new());
        }

        self.buffer.push_str(chunk);
        let mut newly_parsed = Vec::new();
        let mut plain_text_out = String::new();

        loop {
            // Find start of <<call
            let Some(call_start) = self.buffer.find(CALL_PREFIX) else {
                // If <<call is not found, check if buffer ends with a prefix of "<<call "
                let retain_len = partial_prefix_len(&self.buffer, CALL_PREFIX);
                if retain_len > 0 {
                    let drain_up_to = self.buffer.len() - retain_len;
                    if drain_up_to > 0 {
                        let text_chunk: String = self.buffer.drain(..drain_up_to).collect();
                        plain_text_out.push_str(&text_chunk);
                    }
                } else {
                    plain_text_out.push_str(&self.buffer);
                    self.buffer.clear();
                }
                break;
            };

            // If text precedes <<call, emit it as plain text
            if call_start > 0 {
                let text_chunk: String = self.buffer.drain(..call_start).collect();
                plain_text_out.push_str(&text_chunk);
            }

            // Now buffer starts with "<<call "
            let after_prefix = CALL_PREFIX.len();
            let slice = &self.buffer[after_prefix..];

            // Locate tool name: identifier up to whitespace or '{'
            let mut name_end = None;
            for (idx, ch) in slice.char_indices() {
                if ch.is_whitespace() || ch == '{' {
                    name_end = Some(idx);
                    break;
                }
            }

            let Some(name_end_idx) = name_end else {
                // Tool name might not be complete yet in the chunk
                break;
            };

            let tool_name = slice[..name_end_idx].trim().to_string();
            if tool_name.is_empty() {
                self.latched_error = Some(DecodeError::MalformedSyntax(
                    "empty tool name in call marker".to_string(),
                ));
                break;
            }

            // Locate opening '{'
            let mut json_start = None;
            for (idx, ch) in slice[name_end_idx..].char_indices() {
                if ch == '{' {
                    json_start = Some(after_prefix + name_end_idx + idx);
                    break;
                } else if !ch.is_whitespace() {
                    self.latched_error = Some(DecodeError::MalformedSyntax(format!(
                        "unexpected character '{ch}' before JSON arguments"
                    )));
                    break;
                }
            }

            if self.latched_error.is_some() {
                break;
            }

            let Some(json_start_idx) = json_start else {
                // Opening brace not yet arrived
                break;
            };

            // Scan JSON arguments handling braces, strings, and escaped characters
            let mut brace_depth = 0;
            let mut in_string = false;
            let mut escaped = false;
            let mut json_end_idx = None;

            for (idx, ch) in self.buffer[json_start_idx..].char_indices() {
                let current_idx = json_start_idx + idx;
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                } else {
                    match ch {
                        '"' => in_string = true,
                        '{' => brace_depth += 1,
                        '}' => {
                            brace_depth -= 1;
                            if brace_depth == 0 {
                                json_end_idx = Some(current_idx);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }

            let Some(json_end) = json_end_idx else {
                // JSON object not yet complete
                break;
            };

            // Look for closing ">>"
            let after_json = &self.buffer[json_end + 1..];
            let trimmed_offset = after_json
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(after_json.len());

            let marker_slice = &after_json[trimmed_offset..];
            if marker_slice.starts_with(">>") {
                let call_full_end = json_end + 1 + trimmed_offset + 2;
                let raw_args = self.buffer[json_start_idx..=json_end].to_string();

                // Validate JSON syntax and arguments
                match serde_json::from_str::<serde_json::Value>(&raw_args) {
                    Ok(parsed_args) => match validate_call(&tool_name, &parsed_args, &self.tools) {
                        Ok(()) => {
                            self.call_index += 1;
                            let call = ToolCall::new(
                                format!("call_{}", self.call_index),
                                tool_name,
                                raw_args,
                            );
                            self.emitted_calls.push(call.clone());
                            newly_parsed.push(call);
                            self.buffer.drain(..call_full_end);
                        }
                        Err(err) => {
                            self.latched_error = Some(err);
                            self.buffer.drain(..call_full_end);
                            break;
                        }
                    },
                    Err(json_err) => {
                        self.latched_error = Some(DecodeError::JsonError(json_err.to_string()));
                        self.buffer.drain(..call_full_end);
                        break;
                    }
                }
            } else if marker_slice.is_empty() || marker_slice == ">" {
                // Closing >> might be split across chunks (e.g. ">" then ">")
                break;
            } else {
                self.latched_error = Some(DecodeError::MalformedSyntax(
                    "expected '>>' to close call marker".to_string(),
                ));
                break;
            }
        }

        let text_res = if plain_text_out.is_empty() {
            None
        } else {
            Some(plain_text_out)
        };

        (text_res, newly_parsed)
    }

    /// Finish decoding at the end of the stream. Returns all emitted calls or any
    /// validation/parsing error encountered.
    pub fn finish(mut self) -> Result<Vec<ToolCall>, DecodeError> {
        if let Some(err) = self.latched_error {
            return Err(err);
        }

        // Check if unclosed <<call exists in remaining buffer
        if let Some(call_start) = self.buffer.find(CALL_PREFIX) {
            let rem = &self.buffer[call_start..];
            if !rem.trim().is_empty() {
                return Err(DecodeError::UnexpectedEof);
            }
        }

        Ok(self.emitted_calls)
    }

    /// Finish decoding returning any remaining plain text and all emitted calls.
    pub fn finish_with_text(mut self) -> Result<(Option<String>, Vec<ToolCall>), DecodeError> {
        if let Some(err) = self.latched_error {
            return Err(err);
        }

        if let Some(call_start) = self.buffer.find(CALL_PREFIX) {
            let rem = &self.buffer[call_start..];
            if !rem.trim().is_empty() {
                return Err(DecodeError::UnexpectedEof);
            }
        }

        let rem_text = if !self.buffer.is_empty() {
            Some(self.buffer)
        } else {
            None
        };

        Ok((rem_text, self.emitted_calls))
    }

    /// Access currently emitted calls without consuming the decoder.
    pub fn emitted_calls(&self) -> &[ToolCall] {
        &self.emitted_calls
    }
}

/// Computes the longest suffix of `s` that is a prefix of `target`.
fn partial_prefix_len(s: &str, target: &str) -> usize {
    let max_len = s.len().min(target.len() - 1);
    for len in (1..=max_len).rev() {
        if target.starts_with(&s[s.len() - len..]) {
            return len;
        }
    }
    0
}
