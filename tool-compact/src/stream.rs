//! Incremental stream decoder for compact tool calls split across SSE chunks.

use crate::decode;
use crate::error::CompactError;
use crate::types::{ToolCall, ToolDef};

/// Decoded event emitted by the stream decoder.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Plain text content (not part of a tool call).
    Text(String),
    /// A fully decoded and validated tool call.
    Call(ToolCall),
}

/// Incremental decoder that buffers partial `<<call ...>>` markers across chunks.
///
/// Feed chunks via [`push`] and collect events. Call [`finish`] after the last chunk
/// to flush any remaining text.
pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    buffer: String,
    events: Vec<StreamEvent>,
}

impl<'a> StreamDecoder<'a> {
    pub fn new(tools: &'a [ToolDef]) -> Self {
        Self {
            tools,
            buffer: String::new(),
            events: Vec::new(),
        }
    }

    /// Feed a chunk of text. Returns any events that can be emitted.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>, CompactError> {
        self.buffer.push_str(chunk);
        self.events.clear();
        self.try_extract()?;
        Ok(std::mem::take(&mut self.events))
    }

    /// Flush remaining buffer after the last chunk.
    pub fn finish(mut self) -> Result<Vec<StreamEvent>, CompactError> {
        self.events.clear();

        if self.buffer.contains("<<call ") {
            // There's an unclosed marker — error
            return Err(CompactError::MalformedCall(
                "unclosed <<call marker at end of stream".to_string(),
            ));
        }

        if !self.buffer.is_empty() {
            self.events
                .push(StreamEvent::Text(std::mem::take(&mut self.buffer)));
        }

        Ok(self.events)
    }

    fn try_extract(&mut self) -> Result<(), CompactError> {
        loop {
            // Look for a complete <<call ...>> in the buffer
            let open_pos = match self.buffer.find("<<call ") {
                Some(pos) => pos,
                None => {
                    // No marker start found. But we might have a partial "<<ca" at the end.
                    // Emit text up to the last potential partial marker.
                    let safe_end = self.safe_text_end();
                    if safe_end > 0 {
                        let text: String = self.buffer.drain(..safe_end).collect();
                        if !text.is_empty() {
                            self.events.push(StreamEvent::Text(text));
                        }
                    }
                    return Ok(());
                }
            };

            // Emit any text before the marker
            if open_pos > 0 {
                let text: String = self.buffer.drain(..open_pos).collect();
                self.events.push(StreamEvent::Text(text));
            }

            let after_open = "<<call ".len();
            // Find closing >> (not inside a JSON string)
            match find_closing_marker(&self.buffer[after_open..]) {
                Some(close_offset) => {
                    let close_pos = after_open + close_offset;
                    let inner = self.buffer[after_open..close_pos].to_string();
                    // Remove the entire marker from buffer
                    let _ = self.buffer.drain(..close_pos + ">>".len());

                    // Decode the call
                    let full_marker = format!("<<call {inner}>>");
                    let calls = decode::decode_calls(&full_marker, self.tools)?;
                    for call in calls {
                        self.events.push(StreamEvent::Call(call));
                    }
                }
                None => {
                    // Incomplete marker — wait for more data
                    return Ok(());
                }
            }
        }
    }

    /// Find how much text at the start of the buffer is safe to emit.
    /// We hold back any trailing chars that could be the start of "<<call ".
    fn safe_text_end(&self) -> usize {
        let marker = "<<call ";
        let buf = &self.buffer;

        if buf.is_empty() {
            return 0;
        }

        // Check if any suffix of the buffer is a prefix of the marker
        for i in (1..=marker.len().min(buf.len())).rev() {
            let suffix = &buf[buf.len() - i..];
            if marker.starts_with(suffix) {
                return buf.len() - i;
            }
        }

        buf.len()
    }
}

/// Find the closing `>>` that is not inside a JSON string (same logic as decode.rs).
fn find_closing_marker(text: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escape_next = false;
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        match bytes[i] {
            b'\\' if in_string => {
                escape_next = true;
            }
            b'"' => {
                in_string = !in_string;
            }
            b'>' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'>' => {
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"}
                },
                "required": ["title", "start"]
            })),
        }
    }

    #[test]
    fn stream_single_chunk() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        let events = dec
            .push(r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::Call(c) if c.name == "create_calendar_event"));
    }

    #[test]
    fn stream_split_across_chunks() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);

        // Marker split across 4 chunks (matches the problem statement example)
        let chunks = [
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];

        let mut all_events = Vec::new();
        for chunk in &chunks {
            let events = dec.push(chunk).unwrap();
            all_events.extend(events);
        }
        let final_events = dec.finish().unwrap();
        all_events.extend(final_events);

        let calls: Vec<_> = all_events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Call(_)))
            .collect();
        assert_eq!(calls.len(), 1);
        if let StreamEvent::Call(c) = &calls[0] {
            assert_eq!(c.name, "create_calendar_event");
            let args: serde_json::Value = serde_json::from_str(&c.arguments).unwrap();
            assert_eq!(args["title"], "Retro");
        }
    }

    #[test]
    fn stream_text_before_and_after() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);

        let events = dec
            .push(r#"Sure! <<call create_calendar_event {"title":"Test","start":"2026-10-05T09:00:00+05:30"}>> Done."#)
            .unwrap();
        let final_events = dec.finish().unwrap();

        let mut all: Vec<_> = events;
        all.extend(final_events);

        let texts: Vec<_> = all
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        let calls: Vec<_> = all
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Call(c) => Some(c.name.as_str()),
                _ => None,
            })
            .collect();

        assert!(texts.iter().any(|t| t.contains("Sure!")));
        assert!(texts.iter().any(|t| t.contains("Done.")));
        assert_eq!(calls, ["create_calendar_event"]);
    }

    #[test]
    fn stream_no_calls() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        let events = dec.push("Just a normal response.").unwrap();
        let final_events = dec.finish().unwrap();

        let mut all = events;
        all.extend(final_events);

        assert!(all.iter().all(|e| matches!(e, StreamEvent::Text(_))));
    }

    #[test]
    fn stream_unclosed_marker_error() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        let _ = dec
            .push("<<call create_calendar_event {\"title\":\"Test\"")
            .unwrap();
        let result = dec.finish();
        assert!(result.is_err());
    }
}
