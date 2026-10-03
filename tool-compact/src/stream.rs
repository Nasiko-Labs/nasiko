use crate::decode::decode_calls;
use crate::error::Error;
use crate::model::{CompactTools, ToolCall};

#[derive(Debug, Clone)]
pub struct StreamDecoder {
    buffer: String,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn push(
        &mut self,
        chunk: &str,
        tools: &CompactTools,
    ) -> Result<Vec<ToolCall>, Error> {
        self.buffer.push_str(chunk);

        let mut calls = Vec::new();

        loop {
            let Some(start) = self.buffer.find("<<call ") else {
                self.keep_possible_marker_prefix();
                break;
            };

            if start > 0 {
                self.buffer.drain(..start);
            }

            let Some(end_relative) = find_call_end(&self.buffer[7..]) else {
                break;
            };

            let end_relative = end_relative + 7;

            let end = end_relative + 2;
            let complete_call = self.buffer[..end].to_string();

            let decoded = decode_calls(&complete_call, tools)?;
            calls.extend(decoded);

            self.buffer.drain(..end);
        }

        Ok(calls)
    }

    pub fn finish(&mut self, _tools: &CompactTools) -> Result<Vec<ToolCall>, Error> {
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return Ok(Vec::new());
        }

        let remaining = std::mem::take(&mut self.buffer);

        if remaining.contains("<<call ") {
            return Err(Error::InvalidFormat(
                "stream ended before a complete tool call".to_string(),
            ));
        }

        Ok(Vec::new())
    }

    pub fn buffered_text(&self) -> &str {
        &self.buffer
    }

    fn keep_possible_marker_prefix(&mut self) {
        const MARKER: &str = "<<call ";

        let max_prefix = MARKER.len().saturating_sub(1);

        for len in (1..=max_prefix).rev() {
            if self.buffer.ends_with(&MARKER[..len]) {
                let keep_from = self.buffer.len() - len;
                self.buffer.drain(..keep_from);
                return;
            }
        }

        self.buffer.clear();
    }
}


fn find_call_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut in_string = false;
    let mut escaped = false;

    let mut i = 0;

    while i + 1 < bytes.len() {
        let byte = bytes[i];

        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'>' && bytes[i + 1] == b'>' {
            return Some(i);
        }

        i += 1;
    }

    None
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode_tools;
    use crate::model::ToolDef;
    use serde_json::json;

    fn tools() -> CompactTools {
        encode_tools(&[ToolDef {
            name: "get_weather".to_string(),
            description: Some("Get weather".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string"
                    }
                },
                "required": ["city"]
            })),
        }])
        .unwrap()
    }

    #[test]
    fn handles_marker_split_across_chunks() {
        let compact = tools();
        let mut decoder = StreamDecoder::new();

        assert!(decoder.push("<<ca", &compact).unwrap().is_empty());
        assert!(decoder
            .push("ll get_weather ", &compact)
            .unwrap()
            .is_empty());

        assert!(decoder
            .push(r#"{"city":"Hyderabad"}"#, &compact)
            .unwrap()
            .is_empty());

        let calls = decoder.push(">>", &compact).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments, r#"{"city":"Hyderabad"}"#);
    }

    #[test]
    fn handles_complete_call_in_one_chunk() {
        let compact = tools();
        let mut decoder = StreamDecoder::new();

        let calls = decoder
            .push(
                r#"before <<call get_weather {"city":"Delhi"}>> after"#,
                &compact,
            )
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
    }

    #[test]
    fn handles_multiple_calls() {
        let compact = tools();
        let mut decoder = StreamDecoder::new();

        let calls = decoder
            .push(
                r#"<<call get_weather {"city":"Hyderabad"}>><<call get_weather {"city":"Delhi"}>>"#,
                &compact,
            )
            .unwrap();

        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn rejects_incomplete_call_on_finish() {
        let compact = tools();
        let mut decoder = StreamDecoder::new();

        decoder.push("<<call get_weather {", &compact).unwrap();

        let result = decoder.finish(&compact);

        assert!(matches!(result, Err(Error::InvalidFormat(_))));
    }
}
