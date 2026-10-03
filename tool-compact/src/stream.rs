//! Real-Time Incremental Streaming State Machine Decoder.

use crate::decode::decode_calls;
use crate::types::{DecodeError, FunctionCallDelta, ToolCall, ToolCallDelta, ToolDef};

/// Incremental streaming decoder for real-time chunk ingestion.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    current_index: i64,
    last_emitted_len: usize,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buffer: String::new(),
            current_index: 0,
            last_emitted_len: 0,
        }
    }

    /// Feeds an incoming token fragment/chunk and yields any incremental `ToolCallDelta`s.
    pub fn push_chunk(&mut self, chunk: &str) -> Vec<ToolCallDelta> {
        self.buffer.push_str(chunk);
        let mut deltas = Vec::new();

        // Check if a call marker has begun
        if let Some(pos) = self.buffer.find("<<call") {
            let after_marker = &self.buffer[pos + 6..];
            let trimmed = after_marker.trim_start();

            if let Some(json_start) = trimmed.find('{') {
                let name = trimmed[..json_start].trim();
                let after_json_start = &trimmed[json_start..];

                // If arguments are accumulating, emit delta
                if after_json_start.len() > self.last_emitted_len {
                    let new_content = &after_json_start[self.last_emitted_len..];
                    // Clean up any trailing '>>' if partially arrived
                    let clean_chunk = new_content.trim_end_matches('>').trim_end_matches('>');
                    if !clean_chunk.is_empty() {
                        deltas.push(ToolCallDelta {
                            index: self.current_index,
                            id: Some(format!("call_{}", self.current_index + 1)),
                            kind: Some("function".to_string()),
                            function: Some(FunctionCallDelta {
                                name: Some(name.to_string()),
                                arguments: Some(clean_chunk.to_string()),
                            }),
                        });
                    }
                    self.last_emitted_len = after_json_start.len();
                }
            }
        }

        deltas
    }

    /// Finalizes decoding across all streamed chunks and validates the full tool calls.
    pub fn finish(self) -> Result<Vec<ToolCall>, DecodeError> {
        decode_calls(&self.buffer, &self.tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDef;
    use serde_json::json;

    fn test_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(FunctionDef {
                name: "create_calendar_event".to_string(),
                description: Some("Create calendar event".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string"},
                        "duration_min": {"type": "integer"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            }),
            ToolDef::new(FunctionDef {
                name: "send_email".to_string(),
                description: Some("Send email".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            }),
        ]
    }

    #[test]
    fn test_dc001_valid_single_call() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        let chunks = vec![r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#];
        for chunk in chunks {
            decoder.push_chunk(chunk);
        }
        let calls = decoder.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
    }

    #[test]
    fn test_dc002_marker_split_across_stream_chunks() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        let chunks = vec![
            "<<ca",
            r#"ll create_calendar_event {"title":"Ret"#,
            r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#,
            ">",
        ];
        for chunk in chunks {
            decoder.push_chunk(chunk);
        }
        let calls = decoder.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let parsed: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed["title"], "Retro");
    }

    #[test]
    fn test_dc003_embedded_angle_brackets_in_string() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        let chunks = vec![r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#];
        for chunk in chunks {
            decoder.push_chunk(chunk);
        }
        let calls = decoder.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "send_email");
        let parsed: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed["subject"], "a >> b");
    }

    #[test]
    fn test_dc004_unknown_tool_fails() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        let chunks = vec![r#"<<call delete_everything {}>>"#];
        for chunk in chunks {
            decoder.push_chunk(chunk);
        }
        let err = decoder.finish().unwrap_err();
        assert_eq!(err.eval_error_kind(), "unknown_tool");
    }

    #[test]
    fn test_dc005_missing_required_and_bad_enum_fails() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        let chunks = vec![r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#];
        for chunk in chunks {
            decoder.push_chunk(chunk);
        }
        let err = decoder.finish().unwrap_err();
        assert_eq!(err.eval_error_kind(), "invalid_arguments");
    }
}
