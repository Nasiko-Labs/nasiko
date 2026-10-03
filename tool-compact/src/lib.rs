pub mod decode;
pub mod encode;
pub mod stream;
pub mod types;
pub mod validate;

pub use decode::{DecodeError, decode_calls};
pub use encode::{EncodeError, decode_tools, encode_tools};
pub use stream::StreamDecoder;
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolDef};
pub use validate::ValidationError;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new_function(
                "create_calendar_event",
                Some("Create an event in the user's calendar.".to_string()),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "duration_min": {"type": "integer", "description": "Duration in minutes"},
                        "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            ),
            ToolDef::new_function(
                "send_email",
                Some("Send an email from the user's account.".to_string()),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                        "subject": {"type": "string", "description": "Subject line"},
                        "body": {"type": "string", "description": "Plain-text body"},
                        "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            ),
        ]
    }

    #[test]
    fn test_encode_and_decode_tools() {
        let tools = sample_tools();
        let compact = encode_tools(&tools).expect("encode succeeds");
        assert!(compact.prompt.contains("create_calendar_event"));
        assert!(compact.prompt.contains("send_email"));

        let reconstructed = decode_tools(&compact).expect("decode_tools succeeds");
        assert_eq!(reconstructed.len(), 2);
        assert_eq!(reconstructed[0].function.name, "create_calendar_event");
        assert_eq!(reconstructed[1].function.name, "send_email");

        let p0 = reconstructed[0].function.parameters.as_ref().unwrap();
        let req0 = p0.get("required").unwrap().as_array().unwrap();
        assert!(req0.contains(&json!("title")));
        assert!(req0.contains(&json!("start")));
    }

    #[test]
    fn test_decode_single_call() {
        let tools = sample_tools();
        let text = "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";
        let calls = decode_calls(text, &tools).expect("decode succeeds");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let parsed: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed["title"], "Design review");
    }

    #[test]
    fn test_decode_escaped_delimiters_inside_string() {
        let tools = sample_tools();
        let text = "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>";
        let calls = decode_calls(text, &tools).expect("decode succeeds");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "send_email");
        let parsed: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed["subject"], "a >> b");
    }

    #[test]
    fn test_decode_unknown_tool_fails_closed() {
        let tools = sample_tools();
        let text = "<<call delete_everything {}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "unknown_tool");
    }

    #[test]
    fn test_decode_missing_required_and_bad_enum_fails_closed() {
        let tools = sample_tools();
        let text = "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_stream_decoder_split_markers() {
        let tools = sample_tools();
        let mut decoder = StreamDecoder::new(&tools);
        let chunks = vec![
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];

        for chunk in chunks {
            decoder.feed(chunk).expect("feed ok");
        }

        let calls = decoder.finish().expect("finish ok");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let parsed: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed["title"], "Retro");
    }
}
