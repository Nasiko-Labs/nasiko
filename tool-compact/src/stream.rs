//! Incremental streaming decoder for compact tool schemas.
//!
//! Handles model output chunks split arbitrarily across token boundaries,
//! preserves conversational prose outside markers, and produces OpenAI-compliant
//! `ToolCall` objects once complete markers close.

use serde_json::Value;

use crate::decoder::{CALL_PREFIX, CALL_SUFFIX, extract_balanced_json};
use crate::error::ToolCompactError;
use crate::types::{ToolCall, ToolDef};
use crate::validator::validate_call_arguments;

/// Result produced by feeding a chunk into [`StreamDecoder::push_chunk`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamChunkResult {
    /// Conversational model text ready to be streamed to the client.
    pub text: String,
    /// Tool calls that completed and passed schema validation in this chunk.
    pub calls: Vec<ToolCall>,
}

/// The final outcome of a completed stream returned by [`StreamDecoder::finish`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamFinished {
    /// Any remaining trailing text after the last tool call.
    pub remaining_text: String,
    /// All tool calls decoded across the entire stream.
    pub calls: Vec<ToolCall>,
}

/// Internal state of the streaming decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Outside any tool call. Buffering tentative prefixes like `"<"` or `"<<ca"`.
    Text,
    /// Inside a tool call marker `<<call ...`.
    InCall,
}

/// Incremental streaming decoder for compact tool schemas.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    /// Tool definitions used for schema validation.
    tools: Vec<ToolDef>,
    /// Accumulator buffer for chunked tokens.
    buffer: String,
    /// Current state machine state.
    state: State,
    /// All completed tool calls collected across the stream.
    completed_calls: Vec<ToolCall>,
    /// Sequential ID counter (`call_1`, `call_2`, ...).
    call_counter: usize,
}

impl StreamDecoder {
    /// Create a new `StreamDecoder` configured with the available tools.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buffer: String::new(),
            state: State::Text,
            completed_calls: Vec::new(),
            call_counter: 1,
        }
    }

    /// Access all tool calls completed so far across the stream.
    pub fn calls(&self) -> &[ToolCall] {
        &self.completed_calls
    }

    /// Feed an incremental chunk of LLM output into the decoder.
    ///
    /// Returns any text ready for the user and any tool calls that closed in this chunk.
    pub fn push_chunk(&mut self, chunk: &str) -> Result<StreamChunkResult, ToolCompactError> {
        let mut out = StreamChunkResult::default();
        self.buffer.push_str(chunk);

        loop {
            match self.state {
                State::Text => {
                    // Check if CALL_PREFIX ("<<call") exists in the buffer
                    if let Some(pos) = self.buffer.find(CALL_PREFIX) {
                        let after_prefix = pos + CALL_PREFIX.len();
                        let rem = &self.buffer[after_prefix..];

                        // If nothing follows `<<call` yet in the buffer, wait for more chunks
                        if rem.is_empty() {
                            // Emit everything strictly before `pos` as plain text
                            if pos > 0 {
                                out.text.push_str(&self.buffer[..pos]);
                                self.buffer = self.buffer[pos..].to_string();
                            }
                            break;
                        }

                        let first_char = rem.chars().next().unwrap_or(' ');
                        if !first_char.is_whitespace() {
                            // False alarm (e.g. "<<callback"). Emit text up to after_prefix and continue scanning
                            out.text.push_str(&self.buffer[..after_prefix]);
                            self.buffer = self.buffer[after_prefix..].to_string();
                            continue;
                        }

                        // Confirmed marker! Emit text preceding the marker
                        if pos > 0 {
                            out.text.push_str(&self.buffer[..pos]);
                            self.buffer = self.buffer[pos..].to_string();
                        }

                        self.state = State::InCall;
                        // Continue loop in InCall state
                    } else {
                        // `<<call` is not in buffer. Check if the tail is a partial prefix of `<<call`
                        let hold_len = longest_marker_prefix_suffix(&self.buffer);
                        let emit_len = self.buffer.len() - hold_len;
                        if emit_len > 0 {
                            out.text.push_str(&self.buffer[..emit_len]);
                            self.buffer = self.buffer[emit_len..].to_string();
                        }
                        break;
                    }
                }
                State::InCall => {
                    // Buffer starts with `<<call ...`. Attempt to parse the completed call.
                    match self.try_parse_call()? {
                        Some((call, consumed_bytes)) => {
                            self.completed_calls.push(call.clone());
                            out.calls.push(call);
                            self.buffer = self.buffer[consumed_bytes..].to_string();
                            self.state = State::Text;
                            // Continue loop in Text state to process any trailing text in buffer
                        }
                        None => {
                            // Marker is still incomplete, wait for subsequent chunks
                            break;
                        }
                    }
                }
            }
        }

        Ok(out)
    }

    /// Complete the stream and check that no tool calls were left half-open.
    ///
    /// Fails closed if the stream ended inside an unclosed marker.
    pub fn finish(mut self) -> Result<StreamFinished, ToolCompactError> {
        match self.state {
            State::InCall => Err(ToolCompactError::MalformedSyntax(
                "unterminated tool call marker at end of stream".to_string(),
            )),
            State::Text => {
                if self.buffer.starts_with(CALL_PREFIX) {
                    Err(ToolCompactError::MalformedSyntax(
                        "incomplete '<<call' marker at end of stream".to_string(),
                    ))
                } else {
                    let remaining_text = std::mem::take(&mut self.buffer);
                    Ok(StreamFinished {
                        remaining_text,
                        calls: self.completed_calls,
                    })
                }
            }
        }
    }

    /// Convenience helper for decoding a full sequence of chunks into a vector of tool calls.
    pub fn decode_chunks<I, S>(tools: &[ToolDef], chunks: I) -> Result<Vec<ToolCall>, ToolCompactError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut decoder = StreamDecoder::new(tools.to_vec());
        for chunk in chunks {
            decoder.push_chunk(chunk.as_ref())?;
        }
        let finished = decoder.finish()?;
        Ok(finished.calls)
    }

    /// Try to parse a complete tool call from `self.buffer` starting with `<<call`.
    ///
    /// Returns:
    /// - `Ok(Some((call, consumed_bytes)))` if a complete, valid call was parsed.
    /// - `Ok(None)` if more chunks are needed.
    /// - `Err(e)` if syntax or schema validation failed.
    fn try_parse_call(&mut self) -> Result<Option<(ToolCall, usize)>, ToolCompactError> {
        let after_prefix = CALL_PREFIX.len();
        let rem = &self.buffer[after_prefix..];
        let trimmed_rem = rem.trim_start();

        if trimmed_rem.is_empty() {
            // Waiting for tool name
            return Ok(None);
        }

        // 1. Locate end of tool name (whitespace or '{')
        let name_end_rel = match trimmed_rem.find(|c: char| c.is_whitespace() || c == '{') {
            Some(idx) => idx,
            None => return Ok(None), // Tool name still streaming
        };

        let tool_name = trimmed_rem[..name_end_rel].trim();
        if tool_name.is_empty() {
            return Err(ToolCompactError::MalformedSyntax(
                "missing tool name in '<<call' marker".to_string(),
            ));
        }

        // 2. Verify tool exists (fail closed on unknown tool)
        let tool = self
            .tools
            .iter()
            .find(|t| t.function.name == tool_name)
            .ok_or_else(|| ToolCompactError::UnknownTool(tool_name.to_string()))?
            .clone();

        // 3. Locate '{'
        let after_name = &trimmed_rem[name_end_rel..];
        let open_brace_rel = match after_name.find('{') {
            Some(idx) => idx,
            None => {
                if after_name.trim().is_empty() {
                    return Ok(None); // Waiting for '{' in next chunk
                }
                return Err(ToolCompactError::MalformedSyntax(format!(
                    "expected '{{' for arguments of tool '{tool_name}'"
                )));
            }
        };

        let ws_offset = rem.len() - trimmed_rem.len();
        let json_start_offset = after_prefix + ws_offset + name_end_rel + open_brace_rel;
        let json_slice = &self.buffer[json_start_offset..];

        // 4. Extract balanced JSON
        let (json_str, json_end) = match extract_balanced_json(json_slice) {
            Ok(res) => res,
            Err(ToolCompactError::MalformedSyntax(msg))
                if msg.contains("unclosed JSON object") =>
            {
                // Arguments are still streaming
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        let abs_json_end = json_start_offset + json_end;
        let after_json = &self.buffer[abs_json_end..];
        let trimmed_after = after_json.trim_start();

        if trimmed_after.is_empty() {
            // Waiting for '>>' in next chunk
            return Ok(None);
        }

        if !trimmed_after.starts_with(CALL_SUFFIX) {
            if CALL_SUFFIX.starts_with(trimmed_after) {
                // e.g. trimmed_after is ">", waiting for the second '>'
                return Ok(None);
            }
            return Err(ToolCompactError::MalformedSyntax(format!(
                "expected '>>' to close tool call for '{tool_name}'"
            )));
        }

        let suffix_offset = after_json.len() - trimmed_after.len() + CALL_SUFFIX.len();
        let consumed_bytes = abs_json_end + suffix_offset;

        // 5. Parse and validate arguments
        let args_val: Value = serde_json::from_str(json_str).map_err(|e| {
            ToolCompactError::InvalidArguments {
                tool: tool_name.to_string(),
                details: format!("malformed JSON arguments: {e}"),
            }
        })?;

        validate_call_arguments(&tool, &args_val)?;

        // 6. Construct ToolCall
        let call_id = format!("call_{}", self.call_counter);
        self.call_counter += 1;

        let arguments_str = serde_json::to_string(&args_val).unwrap_or_else(|_| "{}".to_string());
        let call = ToolCall::new(call_id, tool_name, arguments_str);

        Ok(Some((call, consumed_bytes)))
    }
}

/// Returns the length of the longest suffix of `s` that is a non-empty prefix of `CALL_PREFIX`.
fn longest_marker_prefix_suffix(s: &str) -> usize {
    let max_len = CALL_PREFIX.len().min(s.len());
    for len in (1..=max_len).rev() {
        if s.ends_with(&CALL_PREFIX[..len]) {
            return len;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create a calendar event".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string" }
                    },
                    "required": ["title", "start"]
                })),
            }),
            ToolDef::new(FunctionDef {
                name: "ping".into(),
                description: Some("Ping check".into()),
                parameters: None,
            }),
        ]
    }

    // 1. Official hackathon split chunk test case
    #[test]
    fn test_stream_official_split_marker_chunks() {
        let tools = sample_tools();
        let chunks = vec![
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];

        let calls = StreamDecoder::decode_chunks(&tools, chunks).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "create_calendar_event");

        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], "Retro");
        assert_eq!(args["start"], "2026-10-04T10:00:00+05:30");
    }

    // 2. Prose streaming before and after calls
    #[test]
    fn test_stream_prose_preservation() {
        let tools = sample_tools();
        let mut decoder = StreamDecoder::new(tools);

        let r1 = decoder.push_chunk("Hello user! ").unwrap();
        assert_eq!(r1.text, "Hello user! ");
        assert!(r1.calls.is_empty());

        let r2 = decoder.push_chunk("<<call ping {}>>").unwrap();
        assert_eq!(r2.text, "");
        assert_eq!(r2.calls.len(), 1);
        assert_eq!(r2.calls[0].function.name, "ping");

        let r3 = decoder.push_chunk(" Done with ping!").unwrap();
        assert_eq!(r3.text, " Done with ping!");
        assert!(r3.calls.is_empty());

        let finished = decoder.finish().unwrap();
        assert_eq!(finished.calls.len(), 1);
        assert_eq!(finished.remaining_text, "");
    }

    // 3. Partial marker false alarm ("2 < 3")
    #[test]
    fn test_stream_false_alarm_prefix() {
        let tools = sample_tools();
        let mut decoder = StreamDecoder::new(tools);

        let r1 = decoder.push_chunk("We know that 2 <").unwrap();
        // '<' is tentatively buffered because it matches prefix of '<<call'
        assert_eq!(r1.text, "We know that 2 ");

        let r2 = decoder.push_chunk(" 3 is true.").unwrap();
        assert_eq!(r2.text, "< 3 is true.");

        let finished = decoder.finish().unwrap();
        assert!(finished.calls.is_empty());
    }

    // 4. Multiple calls in stream
    #[test]
    fn test_stream_multiple_calls() {
        let tools = sample_tools();
        let chunks = vec![
            "I will ping: <<call ping {}>> and then schedule: ",
            "<<call create_calendar_event {\"title\": \"1-on-1\", ",
            "\"start\": \"2026-10-05T09:00:00+05:30\"}>> all done!",
        ];

        let calls = StreamDecoder::decode_chunks(&tools, chunks).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "ping");
        assert_eq!(calls[1].id, "call_2");
        assert_eq!(calls[1].function.name, "create_calendar_event");
    }

    // 5. Unclosed marker at EOF fails closed
    #[test]
    fn test_stream_unclosed_marker_fails_closed() {
        let tools = sample_tools();
        let mut decoder = StreamDecoder::new(tools);

        decoder
            .push_chunk("<<call create_calendar_event {\"title\": \"Retro\"")
            .unwrap();

        let err = decoder.finish().unwrap_err();
        assert_eq!(err.as_code(), "invalid_arguments");
    }

    // 6. Unknown tool fails closed
    #[test]
    fn test_stream_unknown_tool_fails_closed() {
        let tools = sample_tools();
        let chunks = vec!["<<call nonexistent_tool {}>>"];
        let err = StreamDecoder::decode_chunks(&tools, chunks).unwrap_err();
        assert_eq!(err.as_code(), "unknown_tool");
    }
}
