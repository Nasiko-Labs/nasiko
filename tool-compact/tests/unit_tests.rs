use nasiko_tool_compact::*;
use serde_json::json;

fn calendar_and_email_tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new(
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
        ToolDef::new(
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
fn test_dc_001_valid_single_call() {
    let tools = calendar_and_email_tools();
    let text = "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";

    let calls = decode_calls(text, &tools).expect("should decode valid call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "Design review");
    assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
}

#[test]
fn test_dc_002_marker_split_across_stream_chunks() {
    let tools = calendar_and_email_tools();
    let chunks = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];

    let mut decoder = StreamDecoder::new(tools);
    for chunk in chunks {
        decoder.feed(chunk).expect("chunk should feed cleanly");
    }

    let calls = decoder.finish().expect("should decode split marker stream");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "Retro");
    assert_eq!(args["start"], "2026-10-04T10:00:00+05:30");
}

#[test]
fn test_dc_003_delimiter_inside_string_argument() {
    let tools = calendar_and_email_tools();
    let text = "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>";

    let calls = decode_calls(text, &tools).expect("should not be fooled by '>>' inside quotes");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "send_email");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["subject"], "a >> b");
    assert_eq!(args["body"], "x");
}

#[test]
fn test_dc_004_unknown_tool_fails_closed() {
    let tools = calendar_and_email_tools();
    let text = "<<call delete_everything {}>>";

    let err = decode_calls(text, &tools).expect_err("unknown tool must fail");
    assert_eq!(err.error_code(), "unknown_tool");
    match err {
        DecodeError::UnknownTool(name) => assert_eq!(name, "delete_everything"),
        other => panic!("expected UnknownTool, got {other:?}"),
    }
}

#[test]
fn test_dc_005_missing_required_and_invalid_enum_fails_closed() {
    let tools = calendar_and_email_tools();
    let text = "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>";

    let err = decode_calls(text, &tools).expect_err("missing title & bad enum must fail");
    assert_eq!(err.error_code(), "invalid_arguments");
}

#[test]
fn test_multiple_calls_and_conversational_text() {
    let tools = calendar_and_email_tools();
    let text = "\
I will help you with both tasks! First, sending the email:
<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build status\",\"body\":\"The build is green.\"}>>
And scheduling the event:
<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"duration_min\":30,\"visibility\":\"private\"}>>
All done!";

    let calls = decode_calls(text, &tools).expect("should parse multiple calls with surrounding text");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].function.name, "create_calendar_event");
}

#[test]
fn test_no_calls_returns_empty_vec() {
    let tools = calendar_and_email_tools();
    let text = "What's the weather today in Hyderabad? It's sunny and 28°C.";

    let calls = decode_calls(text, &tools).expect("plain text should succeed with 0 calls");
    assert!(calls.is_empty());
}

#[test]
fn test_encode_and_decode_tools_schema_roundtrip() {
    let tools = calendar_and_email_tools();
    let compact = encode_tools(&tools).expect("tools should encode");

    assert!(compact.definitions.contains("create_calendar_event("));
    assert!(compact.definitions.contains("send_email("));
    assert!(compact.definitions.contains("visibility?:public|private"));
    assert!(compact.definitions.contains("duration_min?:int"));

    // decode_tools must restore schemas exactly
    let recovered = decode_tools(&compact).expect("decode_tools should recover definitions");
    assert_eq!(recovered.len(), tools.len());
    assert_eq!(recovered[0].name(), tools[0].name());
    assert_eq!(recovered[1].name(), tools[1].name());
}

#[test]
fn test_byte_by_byte_stream_fuzzing() {
    let tools = calendar_and_email_tools();
    let full_text = "<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";

    // Split at every single character
    let mut decoder = StreamDecoder::new(tools);
    for ch in full_text.chars() {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        decoder.feed(s).unwrap();
    }

    let calls = decoder.finish().expect("byte-by-byte feed must decode properly");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");
}
