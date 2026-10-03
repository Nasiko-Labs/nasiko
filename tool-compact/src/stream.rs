use crate::{
    schema::ToolCall,
    CompactError,
};
use serde_json::Value;

const START: &str = "<<call ";
const END: &str = ">>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Searching,
    InCall,
}

pub struct StreamDecoder {
    buffer: String,
    state: State,
    emitted_calls: usize,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            state: State::Searching,
            emitted_calls: 0,
        }
    }

    /// Feed one chunk of model output.
    ///
    /// Incomplete calls are retained internally until enough data
    /// arrives to complete them.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        self.buffer.push_str(chunk);

        let mut calls = Vec::new();

        loop {
            match self.state {
                State::Searching => {
                    let Some(start) = self.buffer.find(START) else {
                        // Keep enough trailing data to detect a marker
                        // split across the next chunk.
                        let keep = START.len().saturating_sub(1);

                        if self.buffer.len() > keep {
                            let drain =
                                self.buffer.len() - keep;

                            self.buffer.drain(..drain);
                        }

                        break;
                    };

                    if start > 0 {
                        self.buffer.drain(..start);
                    }

                    self.state = State::InCall;
                }

                State::InCall => {
                    let body_start = START.len();

                    let Some(end) =
                        find_call_end(&self.buffer[body_start..])
                    else {
                        // Call is incomplete. Keep everything.
                        break;
                    };

                    let body_end = body_start + end;

                    let body = self.buffer[body_start..body_end]
                        .trim();

                    let call = parse_call_body(body)?;

                    calls.push(call);

                    self.buffer.drain(..body_end + END.len());

                    self.state = State::Searching;
                }
            }
        }

        self.emitted_calls += calls.len();

        Ok(calls)
    }

    /// Finish the stream.
    ///
    /// An incomplete call is considered malformed and fails closed.
    pub fn finish(&self) -> Result<(), CompactError> {
        if self.state == State::InCall {
            return Err(CompactError::MalformedCall(
                "stream ended before call was completed".into(),
            ));
        }

        Ok(())
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.state = State::Searching;
        self.emitted_calls = 0;
    }
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn find_call_end(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();

    let mut in_string = false;
    let mut escaped = false;

    let mut i = 0;

    while i + 1 < bytes.len() {
        let ch = bytes[i];

        if escaped {
            escaped = false;
            i += 1;
            continue;
        }

        if ch == b'\\' && in_string {
            escaped = true;
            i += 1;
            continue;
        }

        if ch == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }

        if !in_string
            && bytes[i] == b'>'
            && bytes[i + 1] == b'>'
        {
            return Some(i);
        }

        i += 1;
    }

    None
}

fn parse_call_body(body: &str) -> Result<ToolCall, CompactError> {
    let split = body.find(' ').ok_or_else(|| {
        CompactError::MalformedCall(
            "expected '<tool_name> <json_arguments>'".into(),
        )
    })?;

    let name = body[..split].trim();

    if name.is_empty() {
        return Err(CompactError::MalformedCall(
            "missing tool name".into(),
        ));
    }

    let arguments = body[split..].trim();

    if arguments.is_empty() {
        return Err(CompactError::MalformedCall(
            "missing arguments".into(),
        ));
    }

    let arguments: Value =
        serde_json::from_str(arguments).map_err(|e| {
            CompactError::MalformedCall(format!(
                "invalid JSON arguments: {e}"
            ))
        })?;

    Ok(ToolCall {
        name: name.to_string(),
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_marker_split_across_chunks() {
        let mut decoder = StreamDecoder::new();

        assert!(
            decoder.push("<<cal").unwrap().is_empty()
        );

        let calls = decoder
            .push(r#"l send_email {"to":["a@example.com"]}>>"#)
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
    }

    #[test]
    fn handles_marker_split_at_every_boundary() {
        let chunks = [
            "<",
            "<",
            "call",
            " ",
            "send_email",
            " ",
            r#"{"to":["a@example.com"]}"#,
            ">",
            ">",
        ];

        let mut decoder = StreamDecoder::new();
        let mut calls = Vec::new();

        for chunk in chunks {
            calls.extend(decoder.push(chunk).unwrap());
        }

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
    }

    #[test]
    fn handles_multiple_streamed_calls() {
        let mut decoder = StreamDecoder::new();

        let calls1 = decoder
            .push(
                r#"<<call send_email {"to":["a@example.com"]}>>"#,
            )
            .unwrap();

        let calls2 = decoder
            .push(
                r#"<<call create_calendar_event {"title":"Review"}>>"#,
            )
            .unwrap();

        assert_eq!(calls1.len(), 1);
        assert_eq!(calls2.len(), 1);

        assert_eq!(calls1[0].name, "send_email");
        assert_eq!(calls2[0].name, "create_calendar_event");
    }

    #[test]
    fn handles_text_before_and_after_call() {
        let mut decoder = StreamDecoder::new();

        let calls = decoder
            .push(
                r#"I will do it. <<call send_email {"to":["a@example.com"]}>> Done."#,
            )
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
    }

    #[test]
    fn does_not_break_on_end_marker_inside_string() {
        let mut decoder = StreamDecoder::new();

        let calls = decoder
            .push(
                r#"<<call send_email {"body":"hello >> world"}>>"#,
            )
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].arguments["body"],
            "hello >> world"
        );
    }

    #[test]
    fn handles_end_marker_split_across_chunks() {
        let mut decoder = StreamDecoder::new();

        assert!(
            decoder
                .push(r#"<<call send_email {"body":"hello"}>"#)
                .unwrap()
                .is_empty()
        );

        let calls = decoder.push(">").unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
    }

    #[test]
    fn plain_stream_has_no_calls() {
        let mut decoder = StreamDecoder::new();

        assert!(
            decoder
                .push("Hello, I don't need a tool.")
                .unwrap()
                .is_empty()
        );

        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn incomplete_stream_fails_closed() {
        let mut decoder = StreamDecoder::new();

        decoder
            .push(
                r#"<<call send_email {"to":["a@example.com"]}"#,
            )
            .unwrap();

        assert!(decoder.finish().is_err());
    }

    #[test]
    fn reset_allows_decoder_reuse() {
        let mut decoder = StreamDecoder::new();

        let calls = decoder
            .push(
                r#"<<call send_email {"to":["a@example.com"]}>>"#,
            )
            .unwrap();

        assert_eq!(calls.len(), 1);

        decoder.reset();

        let calls = decoder
            .push(
                r#"<<call send_email {"to":["b@example.com"]}>>"#,
            )
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].arguments["to"][0],
            "b@example.com"
        );
    }
}