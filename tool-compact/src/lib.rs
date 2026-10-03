pub mod decoder;
pub mod encoder;
pub mod error;
pub mod stream;
pub mod types;

pub use decoder::{decode_calls, extract_raw_calls, validate_arguments};
pub use encoder::{decode_tools, encode_tool_signature, encode_tools};
pub use error::ToolCompactError;
pub use stream::StreamDecoder;
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolCallDelta, ToolDef};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_calendar_tool() -> ToolDef {
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
        )
    }

    fn sample_email_tool() -> ToolDef {
        ToolDef::new_function(
            "send_email",
            Some("Send an email to specified recipients.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}, "description": "Recipients"},
                    "subject": {"type": "string", "description": "Email subject"},
                    "body": {"type": "string", "description": "Email body"}
                },
                "required": ["to", "subject", "body"]
            })),
        )
    }

    #[test]
    fn test_encode_tools() {
        let tools = vec![sample_calendar_tool(), sample_email_tool()];
        let compact = encode_tools(&tools).expect("encode failed");

        assert!(compact.signatures.contains("create_calendar_event("));
        assert!(compact.signatures.contains("title:str"));
        assert!(compact.signatures.contains("start:datetime"));
        assert!(compact.signatures.contains("duration_min?:int"));
        assert!(compact.signatures.contains("visibility?:public|private"));
        assert!(compact.signatures.contains("send_email("));
        assert!(compact.instructions.contains("<<call"));

        // Roundtrip preservation check
        let recovered = decode_tools(&compact).expect("decode tools failed");
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].function.name, "create_calendar_event");
    }

    #[test]
    fn test_decode_calls_valid() {
        let tools = vec![sample_calendar_tool()];
        let text = "Here is your event: <<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\",\"attendees\":[\"riya@example.com\"]}>> Have a nice day!";

        let calls = decode_calls(text, &tools).expect("decode failed");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");

        let parsed_args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed_args["title"], "Design review");
        assert_eq!(parsed_args["attendees"][0], "riya@example.com");
    }

    #[test]
    fn test_decode_calls_multiple() {
        let tools = vec![sample_calendar_tool(), sample_email_tool()];
        let text = "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build status\",\"body\":\"The build is green.\"}>>\n<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"duration_min\":30,\"visibility\":\"private\"}>>";

        let calls = decode_calls(text, &tools).expect("decode failed");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "send_email");
        assert_eq!(calls[1].function.name, "create_calendar_event");
    }

    #[test]
    fn test_fail_closed_unknown_tool() {
        let tools = vec![sample_calendar_tool()];
        let text = "<<call delete_database {\"confirm\":true}>>";

        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "unknown_tool");
    }

    #[test]
    fn test_fail_closed_missing_required() {
        let tools = vec![sample_calendar_tool()];
        // Missing "start"
        let text = "<<call create_calendar_event {\"title\":\"Meeting\"}>>";

        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_fail_closed_enum_violation() {
        let tools = vec![sample_calendar_tool()];
        // visibility enum violation: "secret"
        let text = "<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-04T10:00:00+05:30\",\"visibility\":\"secret\"}>>";

        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_stream_decoder_chunk_split() {
        let tools = vec![sample_calendar_tool()];
        let mut stream = StreamDecoder::new(tools);

        let chunks = vec![
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];

        for chunk in chunks {
            stream.push_chunk(chunk);
        }

        let calls = stream.finish().expect("stream finish failed");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let parsed_args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed_args["title"], "Retro");
    }

    #[test]
    fn test_plain_text_no_calls() {
        let tools = vec![sample_calendar_tool()];
        let text = "What's the weather today in Delhi?";
        let calls = decode_calls(text, &tools).expect("should succeed");
        assert!(calls.is_empty());
    }

    #[test]
    fn test_escaped_closing_marker() {
        let tools = vec![sample_email_tool()];
        let text = "<<call send_email {\"to\":[\"test@example.com\"],\"subject\":\"Symbols\",\"body\":\"Check this arrow: \\>> looks good\"}>>";
        let calls = decode_calls(text, &tools).expect("decode failed");
        assert_eq!(calls.len(), 1);
        let parsed_args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed_args["body"], "Check this arrow: >> looks good");
    }
}
