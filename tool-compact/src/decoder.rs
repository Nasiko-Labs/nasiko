use crate::types::{FunctionCall, Result, ToolCall, ToolCompactError, ToolDef};
use crate::validator::validate_tool_call;
use serde_json::Map;

/// Incremental DFA stream decoder for chunk-by-chunk tool call extraction.
/// Handles markers split across chunks (e.g. ["<<ca", "ll...", ">>"])
/// and quotes/escapes containing '>>' within string arguments.
#[derive(Debug, Default, Clone)]
pub struct StreamDecoder {
    buffer: String,
    call_counter: usize,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            call_counter: 0,
        }
    }

    /// Feed a new chunk of text into the stream.
    pub fn feed(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    /// Finalizes the stream and parses all extracted calls against the tools schema.
    pub fn finish(mut self, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
        self.extract_and_validate(tools)
    }

    /// Internal extraction using a strict character-level DFA.
    pub fn extract_and_validate(&mut self, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
        let mut calls = Vec::new();
        let bytes = self.buffer.as_bytes();
        let len = bytes.len();
        let mut idx = 0;

        while idx < len {
            // Find start marker "<<call"
            if let Some(pos) = self.find_marker_start(idx) {
                let call_start = pos + 6; // skip "<<call"
                let mut p = call_start;

                // Skip leading whitespace after <<call
                while p < len && (bytes[p] == b' ' || bytes[p] == b'\\t' || bytes[p] == b'\\n' || bytes[p] == b'\\r') {
                    p += 1;
                }

                // Extract tool name
                let name_start = p;
                while p < len && (bytes[p].is_ascii_alphanumeric() || bytes[p] == b'_' || bytes[p] == b'-') {
                    p += 1;
                }
                let tool_name = String::from_utf8_lossy(&bytes[name_start..p]).trim().to_string();

                if tool_name.is_empty() {
                    return Err(ToolCompactError::MalformedSyntax("Missing tool name in <<call>>".into()));
                }

                // Skip whitespace before JSON argument
                while p < len && (bytes[p] == b' ' || bytes[p] == b'\\t' || bytes[p] == b'\\n' || bytes[p] == b'\\r') {
                    p += 1;
                }

                // Parse JSON payload while tracking string states to avoid matching '>>' inside strings!
                let json_start = p;
                let mut in_string = false;
                let mut escaped = false;
                let mut brace_depth = 0;
                let mut call_end: Option<usize> = None;

                while p < len {
                    let b = bytes[p];

                    if in_string {
                        if escaped {
                            escaped = false;
                        } else if b == b'\\\\' {
                            escaped = true;
                        } else if b == b'"' {
                            in_string = false;
                        }
                        p += 1;
                        continue;
                    }

                    // Not inside a string:
                    match b {
                        b'"' => {
                            in_string = true;
                            p += 1;
                        }
                        b'{' => {
                            brace_depth += 1;
                            p += 1;
                        }
                        b'}' => {
                            if brace_depth > 0 {
                                brace_depth -= 1;
                            }
                            p += 1;
                        }
                        b'>' if p + 1 < len && bytes[p + 1] == b'>' && brace_depth == 0 => {
                            // Found valid closing delimiter '>>'
                            call_end = Some(p);
                            p += 2;
                            break;
                        }
                        _ => {
                            p += 1;
                        }
                    }
                }

                if let Some(end_pos) = call_end {
                    let raw_args = String::from_utf8_lossy(&bytes[json_start..end_pos]).trim().to_string();
                    let final_args = if raw_args.is_empty() {
                        "{}".to_string()
                    } else {
                        raw_args
                    };

                    self.call_counter += 1;
                    let call = ToolCall {
                        id: format!("call_{}", self.call_counter),
                        kind: "function".to_string(),
                        function: FunctionCall {
                            name: tool_name,
                            arguments: final_args,
                        },
                        extra: Map::new(),
                    };

                    // Strict fail-closed schema validation!
                    validate_tool_call(&call, tools)?;
                    calls.push(call);
                    idx = p;
                } else {
                    // Incomplete call or unclosed delimiter
                    idx += 1;
                }
            } else {
                break;
            }
        }

        Ok(calls)
    }

    fn find_marker_start(&self, from: usize) -> Option<usize> {
        let slice = &self.buffer[from..];
        slice.find("<<call").map(|offset| from + offset)
    }
}

/// Convenience decoder for complete text.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new();
    decoder.feed(text);
    decoder.finish(tools)
}
