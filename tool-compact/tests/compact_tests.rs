use nasiko_tool_compact::{
    decode_calls, decode_tools, encode_tools, FunctionDef, StreamDecoder, ToolCompactError, ToolDef,
};
use serde_json::{json, Map};

fn calendar_tool() -> ToolDef {
    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        },
        extra: Map::new(),
    }
}

#[test]
fn test_encode_and_roundtrip_schema() {
    let tools = vec![calendar_tool()];
    let compact = encode_tools(&tools).expect("encode should succeed");

    assert!(compact.compact_definitions.contains("create_calendar_event("));
    assert!(compact.compact_definitions.contains("visibility?:public|private"));
    assert!(compact.compact_definitions.contains("duration_min?:int"));

    // Verify schema information survived using decode_tools
    let decoded_tools = decode_tools(&compact).expect("decode_tools should succeed");
    assert_eq!(decoded_tools.len(), 1);
    assert_eq!(decoded_tools[0].function.name, "create_calendar_event");
}

#[test]
fn test_decode_single_call() {
    let tools = vec![calendar_tool()];
    let output = r#"Sure! <<call create_calendar_event {"title":"Team Sync","start":"2026-10-05T10:00:00Z"}>> Done!"#;
    let calls = decode_calls(output, &tools).expect("should decode call");

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");
    assert!(calls[0].function.arguments.contains("Team Sync"));
}

#[test]
fn test_split_marker_across_chunks() {
    let tools = vec![calendar_tool()];
    let mut decoder = StreamDecoder::new();

    let chunks = vec![
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];

    for chunk in chunks {
        decoder.feed(chunk);
    }

    let calls = decoder.finish(&tools).expect("should handle split markers");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");
}

#[test]
fn test_escaped_marker_inside_string_argument() {
    let tools = vec![calendar_tool()];
    let output = r#"<<call create_calendar_event {"title":"Design review >> Final pass","start":"2026-10-05T15:00:00Z"}>>"#;
    let calls = decode_calls(output, &tools).expect("should handle >> inside strings");

    assert_eq!(calls.len(), 1);
    assert!(calls[0].function.arguments.contains("Design review >> Final pass"));
}

#[test]
fn test_unknown_tool_fails_closed() {
    let tools = vec![calendar_tool()];
    let output = r#"<<call delete_database {"confirm":true}>>"#;
    let err = decode_calls(output, &tools).unwrap_err();

    match err {
        ToolCompactError::UnknownTool(name) => assert_eq!(name, "delete_database"),
        other => panic!("Expected UnknownTool, got {:?}", other),
    }
}

#[test]
fn test_missing_required_argument_fails_closed() {
    let tools = vec![calendar_tool()];
    // Missing 'start'
    let output = r#"<<call create_calendar_event {"title":"Standup"}>>"#;
    let err = decode_calls(output, &tools).unwrap_err();

    match err {
        ToolCompactError::InvalidArguments(tool, msg) => {
            assert_eq!(tool, "create_calendar_event");
            assert!(msg.contains("Missing required argument 'start'"));
        }
        other => panic!("Expected InvalidArguments, got {:?}", other),
    }
}

#[test]
fn test_enum_violation_fails_closed() {
    let tools = vec![calendar_tool()];
    let output = r#"<<call create_calendar_event {"title":"Party","start":"2026-10-05T20:00:00Z","visibility":"secret"}>>"#;
    let err = decode_calls(output, &tools).unwrap_err();

    match err {
        ToolCompactError::InvalidArguments(tool, msg) => {
            assert_eq!(tool, "create_calendar_event");
            assert!(msg.contains("Must be one of"));
        }
        other => panic!("Expected InvalidArguments, got {:?}", other),
    }
}

#[test]
fn test_multiple_sequential_calls() {
    let tools = vec![calendar_tool()];
    let output = r#"I'll create both:
<<call create_calendar_event {"title":"Event 1","start":"2026-10-05T10:00:00Z"}>>
And the second one:
<<call create_calendar_event {"title":"Event 2","start":"2026-10-05T14:00:00Z"}>>"#;

    let calls = decode_calls(output, &tools).expect("should decode multiple calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[1].id, "call_2");
}

#[test]
fn test_plain_text_with_no_calls() {
    let tools = vec![calendar_tool()];
    let output = "The weather today is sunny and 24 degrees Celsius.";
    let calls = decode_calls(output, &tools).expect("should handle text with no calls");
    assert!(calls.is_empty());
}
