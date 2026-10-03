use nasiko_tool_compact::{FunctionDef, StreamDecoder, ToolCompactError, ToolDef};
use serde_json::{Value, json};

fn sample_tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new(FunctionDef {
            name: "create_calendar_event".into(),
            description: Some("Create a calendar event".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "start": { "type": "string" },
                    "duration_min": { "type": "integer" },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"],
                "additionalProperties": false
            })),
        }),
        ToolDef::new(FunctionDef {
            name: "send_email".into(),
            description: Some("Send an email".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": { "type": "array", "items": { "type": "string" } },
                    "subject": { "type": "string" }
                },
                "required": ["to", "subject"]
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
fn test_stream_official_sample_case() {
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

// 2. Extreme fragmentation: 1 character per chunk!
#[test]
fn test_stream_single_character_chunks() {
    let tools = sample_tools();
    let full_text = r#"Sure! <<call ping {}>> done."#;
    let chunks: Vec<String> = full_text.chars().map(|c| c.to_string()).collect();

    let mut decoder = StreamDecoder::new(tools);
    let mut collected_text = String::new();

    for chunk in chunks {
        let res = decoder.push_chunk(&chunk).unwrap();
        collected_text.push_str(&res.text);
    }
    let finished = decoder.finish().unwrap();
    collected_text.push_str(&finished.remaining_text);

    assert_eq!(finished.calls.len(), 1);
    assert_eq!(finished.calls[0].id, "call_1");
    assert_eq!(finished.calls[0].function.name, "ping");
    assert_eq!(collected_text, "Sure!  done.");
}

// 3. Closing delimiter split across chunks (e.g. ">" then ">")
#[test]
fn test_stream_closing_delimiter_split() {
    let tools = sample_tools();
    let chunks = vec![
        "<<call create_calendar_event {\"title\": \"Design\", \"start\": \"2026-10-05T15:00:00+05:30\"}",
        ">",
        ">",
    ];

    let calls = StreamDecoder::decode_chunks(&tools, chunks).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");
}

// 4. Conversational text streaming before, between, and after tool calls
#[test]
fn test_stream_text_and_tool_interleaving() {
    let tools = sample_tools();
    let mut decoder = StreamDecoder::new(tools);

    // Chunk 1: purely conversational text
    let r1 = decoder.push_chunk("I will help you with that.\n").unwrap();
    assert_eq!(r1.text, "I will help you with that.\n");
    assert!(r1.calls.is_empty());

    // Chunk 2: start of tool call
    let r2 = decoder.push_chunk("<<call ping {}").unwrap();
    assert_eq!(r2.text, "");
    assert!(r2.calls.is_empty());

    // Chunk 3: end of tool call and transition text
    let r3 = decoder.push_chunk(">>\nNow sending email: ").unwrap();
    assert_eq!(r3.text, "\nNow sending email: ");
    assert_eq!(r3.calls.len(), 1);
    assert_eq!(r3.calls[0].function.name, "ping");

    // Chunk 4: second tool call
    let r4 = decoder
        .push_chunk("<<call send_email {\"to\": [\"dev@example.com\"], \"subject\": \"Hi\"}>>")
        .unwrap();
    assert_eq!(r4.text, "");
    assert_eq!(r4.calls.len(), 1);
    assert_eq!(r4.calls[0].function.name, "send_email");

    // Chunk 5: concluding text
    let r5 = decoder.push_chunk(" All done!").unwrap();
    assert_eq!(r5.text, " All done!");
    assert!(r5.calls.is_empty());

    let finished = decoder.finish().unwrap();
    assert_eq!(finished.calls.len(), 2);
    assert_eq!(finished.calls[0].id, "call_1");
    assert_eq!(finished.calls[1].id, "call_2");
}

// 5. False alarm markers ("<" and "<<call_function")
#[test]
fn test_stream_false_alarm_markers() {
    let tools = sample_tools();
    let mut decoder = StreamDecoder::new(tools);

    let r1 = decoder.push_chunk("Check if 1 <").unwrap();
    assert_eq!(r1.text, "Check if 1 ");

    let r2 = decoder.push_chunk(" 2 is true. Also <<callable").unwrap();
    assert_eq!(r2.text, "< 2 is true. Also <<callable");

    let r3 = decoder.push_chunk(" is a keyword.").unwrap();
    assert_eq!(r3.text, " is a keyword.");

    let finished = decoder.finish().unwrap();
    assert!(finished.calls.is_empty());
}

// 6. Escaped delimiters and braces inside JSON strings across chunks
#[test]
fn test_stream_escaped_delimiters_in_string_across_chunks() {
    let tools = sample_tools();
    let chunks = vec![
        "<<call create_calendar_event {\"title\": \"A >",
        "> B meeting with \\\"qu",
        "otes\\\" inside\", \"start\": \"2026-10-05T10:00:00+05:30\"}>>",
    ];

    let calls = StreamDecoder::decode_chunks(&tools, chunks).unwrap();
    assert_eq!(calls.len(), 1);
    let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "A >> B meeting with \"quotes\" inside");
}

// 7. Unclosed marker at EOF fails closed
#[test]
fn test_stream_unclosed_marker_fails_closed() {
    let tools = sample_tools();
    let mut decoder = StreamDecoder::new(tools);

    decoder
        .push_chunk("<<call create_calendar_event {\"title\": \"Meeting\"")
        .unwrap();

    let err = decoder.finish().unwrap_err();
    assert_eq!(err.as_code(), "invalid_arguments");
}

// 8. Unknown tool name fails closed
#[test]
fn test_stream_unknown_tool_fails_closed() {
    let tools = sample_tools();
    let chunks = vec!["<<call drop_database {\"all\": true}>>"];
    let err = StreamDecoder::decode_chunks(&tools, chunks).unwrap_err();
    assert_eq!(err.as_code(), "unknown_tool");

    match err {
        ToolCompactError::UnknownTool(name) => assert_eq!(name, "drop_database"),
        other => panic!("expected UnknownTool, got {:?}", other),
    }
}

// 9. Missing required field fails closed
#[test]
fn test_stream_missing_required_field_fails_closed() {
    let tools = sample_tools();
    // 'start' is missing
    let chunks = vec!["<<call create_calendar_event {\"title\": \"Planning\"}>>"];
    let err = StreamDecoder::decode_chunks(&tools, chunks).unwrap_err();
    assert_eq!(err.as_code(), "invalid_arguments");

    match err {
        ToolCompactError::MissingRequiredField { tool, field } => {
            assert_eq!(tool, "create_calendar_event");
            assert_eq!(field, "start");
        }
        other => panic!("expected MissingRequiredField, got {:?}", other),
    }
}

// 10. Invalid enum value fails closed
#[test]
fn test_stream_invalid_enum_fails_closed() {
    let tools = sample_tools();
    // 'visibility' must be "public" or "private"
    let chunks = vec![
        "<<call create_calendar_event {\"title\": \"Secret\", \"start\": \"2026-10-05T10:00:00+05:30\", ",
        "\"visibility\": \"confidential\"}>>",
    ];
    let err = StreamDecoder::decode_chunks(&tools, chunks).unwrap_err();
    assert_eq!(err.as_code(), "invalid_arguments");

    match err {
        ToolCompactError::InvalidEnumValue { tool, value, .. } => {
            assert_eq!(tool, "create_calendar_event");
            assert_eq!(value, "confidential");
        }
        other => panic!("expected InvalidEnumValue, got {:?}", other),
    }
}

// 11. Malformed JSON syntax inside arguments fails closed
#[test]
fn test_stream_malformed_json_fails_closed() {
    let tools = sample_tools();
    let chunks = vec!["<<call create_calendar_event {\"title\": }>>"];
    let err = StreamDecoder::decode_chunks(&tools, chunks).unwrap_err();
    assert_eq!(err.as_code(), "invalid_arguments");
}

// 12. Pure conversational stream with no tool calls
#[test]
fn test_stream_pure_conversational_text() {
    let tools = sample_tools();
    let chunks = vec!["The weather ", "is sunny ", "today."];

    let mut decoder = StreamDecoder::new(tools);
    let mut text_acc = String::new();
    for c in chunks {
        let r = decoder.push_chunk(c).unwrap();
        text_acc.push_str(&r.text);
    }
    let finished = decoder.finish().unwrap();
    text_acc.push_str(&finished.remaining_text);

    assert_eq!(text_acc, "The weather is sunny today.");
    assert!(finished.calls.is_empty());
}
