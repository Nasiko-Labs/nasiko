use nasiko_tool_compact::{
    decode_calls, decode_tools, encode_tools, StreamDecoder, ToolCompactError, ToolDef,
};
use serde_json::json;

fn create_sample_tools() -> Vec<ToolDef> {
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
            Some("Send an email to recipients.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"}
                },
                "required": ["to", "subject", "body"]
            })),
        ),
        ToolDef::new(
            "get_status",
            Some("Get system status.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {}
            })),
        ),
    ]
}

#[test]
fn test_single_tool_call_decoding() {
    let tools = create_sample_tools();
    let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;

    let calls = decode_calls(text, &tools).expect("should decode call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].kind, "function");
    assert_eq!(calls[0].function.name, "create_calendar_event");

    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.arguments).expect("valid json");
    assert_eq!(args["title"], "Design review");
    assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
}

#[test]
fn test_multiple_tool_calls_decoding() {
    let tools = create_sample_tools();
    let text = r#"
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>
"#;

    let calls = decode_calls(text, &tools).expect("should decode multiple calls");
    assert_eq!(calls.len(), 2);

    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].function.name, "send_email");
    let args1: serde_json::Value =
        serde_json::from_str(&calls[0].function.arguments).expect("valid json");
    assert_eq!(args1["to"][0], "sam@example.com");
    assert_eq!(args1["subject"], "Build status");

    assert_eq!(calls[1].id, "call_2");
    assert_eq!(calls[1].function.name, "create_calendar_event");
    let args2: serde_json::Value =
        serde_json::from_str(&calls[1].function.arguments).expect("valid json");
    assert_eq!(args2["title"], "Retro");
    assert_eq!(args2["duration_min"], 30);
    assert_eq!(args2["visibility"], "private");
}

#[test]
fn test_mixed_text_and_tool_calls() {
    let tools = create_sample_tools();
    let text = r#"
Certainly! I will send an email right away:
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"All clear"}>>
I have also placed a reminder on your calendar:
<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>
Please let me know if there's anything else you need assistance with!
"#;

    let calls = decode_calls(text, &tools).expect("should decode mixed text and calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].function.name, "create_calendar_event");
}

#[test]
fn test_plain_text_response_no_calls() {
    let tools = create_sample_tools();
    let text = "Hello! I looked at your schedule, and you have no meetings today. Enjoy your day!";

    let calls = decode_calls(text, &tools).expect("plain text should decode without error");
    assert!(calls.is_empty(), "expected no tool calls");
}

#[test]
fn test_stream_chunking_split_markers() {
    let tools = create_sample_tools();
    let chunks = vec![
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];

    let mut decoder = StreamDecoder::new(tools);
    let mut collected = Vec::new();

    let c0 = decoder.push_chunk(chunks[0]).expect("chunk 0 ok");
    assert!(c0.is_empty());

    let c1 = decoder.push_chunk(chunks[1]).expect("chunk 1 ok");
    assert!(c1.is_empty());

    let c2 = decoder.push_chunk(chunks[2]).expect("chunk 2 ok");
    assert!(c2.is_empty());

    let c3 = decoder.push_chunk(chunks[3]).expect("chunk 3 ok");
    assert_eq!(c3.len(), 1);
    collected.extend(c3);

    let finished = decoder.finish().expect("finish ok");
    assert!(finished.is_empty());

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].function.name, "create_calendar_event");
    let args: serde_json::Value =
        serde_json::from_str(&collected[0].function.arguments).expect("valid json");
    assert_eq!(args["title"], "Retro");
    assert_eq!(args["start"], "2026-10-04T10:00:00+05:30");
}

#[test]
fn test_stream_chunking_fine_grain_splits() {
    let tools = create_sample_tools();
    let text = r#"Notice: <<call get_status {}>> Done!"#;

    // Split text into tiny 2-character chunks
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = Vec::new();

    for chunk in text.as_bytes().chunks(2) {
        let chunk_str = std::str::from_utf8(chunk).unwrap();
        calls.extend(decoder.push_chunk(chunk_str).expect("chunk push ok"));
    }
    calls.extend(decoder.finish().expect("finish ok"));

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "get_status");
}

#[test]
fn test_escaped_characters_and_arrows_inside_json() {
    let tools = create_sample_tools();
    let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"Use >> and \"quotes\" safely","body":"Text with >> inside \"quoted >> segment\" and backslash \\"}>>"#;

    let calls = decode_calls(text, &tools).expect("should handle >> and escapes inside string");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "send_email");

    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.arguments).expect("valid json");
    assert_eq!(args["subject"], "Use >> and \"quotes\" safely");
    assert_eq!(
        args["body"],
        "Text with >> inside \"quoted >> segment\" and backslash \\"
    );
}

#[test]
fn test_negative_unknown_tool() {
    let tools = create_sample_tools();
    let text = r#"<<call unknown_tool {"param":"val"}>>"#;

    let result = decode_calls(text, &tools);
    match result {
        Err(ToolCompactError::UnknownTool(name)) => {
            assert_eq!(name, "unknown_tool");
        }
        other => panic!("expected ToolCompactError::UnknownTool, got {other:?}"),
    }
}

#[test]
fn test_negative_missing_required_field() {
    let tools = create_sample_tools();
    // Missing 'start' parameter (only 'title' provided)
    let text = r#"<<call create_calendar_event {"title":"Design review"}>>"#;

    let result = decode_calls(text, &tools);
    match result {
        Err(ToolCompactError::InvalidArguments(msg)) => {
            assert!(
                msg.contains("start"),
                "expected message mentioning missing field 'start', got: {msg}"
            );
        }
        other => panic!("expected ToolCompactError::InvalidArguments, got {other:?}"),
    }
}

#[test]
fn test_negative_enum_violation() {
    let tools = create_sample_tools();
    // visibility must be "public" or "private", "secret" is invalid
    let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;

    let result = decode_calls(text, &tools);
    match result {
        Err(ToolCompactError::InvalidArguments(msg)) => {
            assert!(
                msg.contains("visibility"),
                "expected message mentioning field 'visibility', got: {msg}"
            );
        }
        other => panic!("expected ToolCompactError::InvalidArguments, got {other:?}"),
    }
}

#[test]
fn test_negative_type_mismatch() {
    let tools = create_sample_tools();
    // duration_min expects integer, got string "thirty"
    let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","duration_min":"thirty"}>>"#;

    let result = decode_calls(text, &tools);
    match result {
        Err(ToolCompactError::InvalidArguments(msg)) => {
            assert!(
                msg.contains("duration_min"),
                "expected message mentioning 'duration_min', got: {msg}"
            );
        }
        other => panic!("expected ToolCompactError::InvalidArguments, got {other:?}"),
    }
}

#[test]
fn test_negative_unterminated_stream_call() {
    let tools = create_sample_tools();
    let mut decoder = StreamDecoder::new(tools);
    decoder
        .push_chunk(r#"<<call create_calendar_event {"title":"Cut off""#)
        .expect("chunk push ok");

    let finish_result = decoder.finish();
    match finish_result {
        Err(ToolCompactError::ParseError(msg)) => {
            assert!(
                msg.contains("Unterminated"),
                "expected unterminated error, got: {msg}"
            );
        }
        other => panic!("expected ToolCompactError::ParseError, got {other:?}"),
    }
}

#[test]
fn test_roundtrip_schema_fidelity() {
    let original_tools = vec![
        ToolDef::new(
            "create_calendar_event",
            Some("Create an event in the user's calendar.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        ),
        ToolDef::new(
            "send_email",
            Some("Send an email to recipients.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"}
                },
                "required": ["to", "subject", "body"]
            })),
        ),
        ToolDef::new(
            "get_status",
            Some("Get system status.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {}
            })),
        ),
    ];

    let compact = encode_tools(&original_tools).expect("encode succeeds");
    let decoded_tools = decode_tools(&compact).expect("decode succeeds");

    assert_eq!(decoded_tools.len(), original_tools.len());

    for (orig, dec) in original_tools.iter().zip(decoded_tools.iter()) {
        assert_eq!(orig.kind, dec.kind);
        assert_eq!(orig.function.name, dec.function.name);
        assert_eq!(orig.function.description, dec.function.description);

        let orig_params = orig.function.parameters.as_ref().unwrap();
        let dec_params = dec.function.parameters.as_ref().unwrap();

        // Compare parameter type
        assert_eq!(orig_params["type"], dec_params["type"]);

        // Compare required lists (set-wise)
        let orig_req: Vec<String> = orig_params
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let dec_req: Vec<String> = dec_params
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut orig_req_sorted = orig_req;
        orig_req_sorted.sort();
        let mut dec_req_sorted = dec_req;
        dec_req_sorted.sort();
        assert_eq!(orig_req_sorted, dec_req_sorted);

        // Compare properties
        let orig_props = orig_params["properties"].as_object().unwrap();
        let dec_props = dec_params["properties"].as_object().unwrap();

        assert_eq!(orig_props.len(), dec_props.len());
        for (prop_name, orig_schema) in orig_props {
            let dec_schema = dec_props
                .get(prop_name)
                .unwrap_or_else(|| panic!("missing property '{prop_name}' in decoded"));

            if let Some(t) = orig_schema.get("type") {
                assert_eq!(t, &dec_schema["type"], "type mismatch on {prop_name}");
            }
            if let Some(f) = orig_schema.get("format") {
                assert_eq!(f, &dec_schema["format"], "format mismatch on {prop_name}");
            }
            if let Some(e) = orig_schema.get("enum") {
                assert_eq!(e, &dec_schema["enum"], "enum mismatch on {prop_name}");
            }
            if let Some(items) = orig_schema.get("items") {
                assert_eq!(
                    items["type"], dec_schema["items"]["type"],
                    "items type mismatch on {prop_name}"
                );
            }
        }
    }
}

#[test]
fn test_strip_calls_basic_and_mixed() {
    use nasiko_tool_compact::strip_calls;

    let only_call = "<<call create_calendar_event {\"title\": \"Meeting\"}>>";
    assert_eq!(strip_calls(only_call).trim(), "");

    let mixed = "I scheduled your meeting!\n<<call create_calendar_event {\"title\": \"Meeting\"}>>\nLet me know if you need anything else.";
    assert_eq!(
        strip_calls(mixed).trim(),
        "I scheduled your meeting!\n\nLet me know if you need anything else."
    );

    let arrows_inside = "Result:\n<<call test {\"body\": \"a >> b\"}>>\nDone.";
    assert_eq!(strip_calls(arrows_inside).trim(), "Result:\n\nDone.");
}

#[test]
fn test_nullable_required_field_allowed() {
    use nasiko_tool_compact::{decode_calls, FunctionDef, ToolDef};

    let tools = vec![ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: "update_status".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "nullable": true
                    }
                },
                "required": ["reason"]
            })),
        },
    }];

    let text = "<<call update_status {\"reason\": null}>>";
    let calls = decode_calls(text, &tools).expect("nullable required field should be accepted");
    assert_eq!(calls.len(), 1);
}

#[test]
fn test_stream_decoder_max_buffer_guard() {
    use nasiko_tool_compact::{StreamDecoder, ToolCompactError};

    let mut decoder = StreamDecoder::new(vec![]);
    // Push chunks that exceed 256KB
    let huge_chunk = "a".repeat(300 * 1024);
    let result = decoder.push_chunk(&huge_chunk);
    assert!(matches!(result, Err(ToolCompactError::ParseError(_))));
}
