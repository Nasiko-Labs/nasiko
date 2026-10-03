use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolCompactError, ToolDef};
use serde_json::json;

fn create_full_test_tools() -> Vec<ToolDef> {
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
            "system_command",
            Some("Execute a command.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "query": {"type": "string"},
                    "title": {"type": "string"},
                    "desc": {"type": "string"},
                    "timeout": {"type": ["integer", "null"]}
                },
                "additionalProperties": false,
                "required": []
            })),
        ),
        ToolDef::new(
            "get_status",
            Some("Check system health status.".to_string()),
            Some(json!({
                "type": "object",
                "properties": {}
            })),
        ),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Streaming Stress Cases
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_adversarial_1byte_chunk_streaming() {
    let tools = create_full_test_tools();
    let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;

    let mut decoder = StreamDecoder::new(tools);
    let mut collected = Vec::new();

    // Stream 1 byte (1 ASCII char) at a time
    for ch in text.chars() {
        let s = ch.to_string();
        let chunk_res = decoder.push_chunk(&s).expect("push chunk should succeed");
        collected.extend(chunk_res);
    }
    let finished = decoder.finish().expect("finish should succeed");
    collected.extend(finished);

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].function.name, "create_calendar_event");
    let args: serde_json::Value = serde_json::from_str(&collected[0].function.arguments).unwrap();
    assert_eq!(args["title"], "Design review");
    assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
}

#[test]
fn test_adversarial_char_by_char_streaming_with_unicode() {
    let tools = create_full_test_tools();
    let text = r#"<<call system_command {"title":"🚀 Review 💡","desc":"नमस्ते / こんにちは"}>>"#;

    let mut decoder = StreamDecoder::new(tools);
    let mut collected = Vec::new();

    // Stream 1 char at a time across multi-byte UTF-8 boundaries
    for ch in text.chars() {
        let s = ch.to_string();
        let chunk_res = decoder.push_chunk(&s).expect("push chunk should succeed");
        collected.extend(chunk_res);
    }
    let finished = decoder.finish().expect("finish should succeed");
    collected.extend(finished);

    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].function.name, "system_command");
    let args: serde_json::Value = serde_json::from_str(&collected[0].function.arguments).unwrap();
    assert_eq!(args["title"], "🚀 Review 💡");
    assert_eq!(args["desc"], "नमस्ते / こんにちは");
}

#[test]
fn test_adversarial_marker_splits_at_every_boundary() {
    let tools = create_full_test_tools();
    let full_call =
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#;

    // Test splitting at every prefix of "<<call ":
    // boundaries: 1 ("<"), 2 ("<<"), 3 ("<<c"), 4 ("<<ca"), 5 ("<<cal"), 6 ("<<call"), 7 ("<<call ")
    for split_idx in 1..=7 {
        let chunk1 = &full_call[..split_idx];
        let chunk2 = &full_call[split_idx..];

        let mut decoder = StreamDecoder::new(tools.clone());
        let mut calls = Vec::new();
        calls.extend(decoder.push_chunk(chunk1).expect("chunk 1 ok"));
        calls.extend(decoder.push_chunk(chunk2).expect("chunk 2 ok"));
        calls.extend(decoder.finish().expect("finish ok"));

        assert_eq!(
            calls.len(),
            1,
            "Failed when splitting marker at index {split_idx} ('{chunk1}')"
        );
        assert_eq!(calls[0].function.name, "create_calendar_event");
    }
}

#[test]
fn test_adversarial_trailing_delimiter_splits() {
    let tools = create_full_test_tools();
    let prefix =
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}"#;

    // Cases for trailing delimiter:
    // 1. split right before ">>"
    // 2. split between ">" and ">"
    // 3. split before "}"
    // 4. split into single-char pieces for "}>>"
    let cases = vec![
        (vec![format!("{prefix}"), ">>".to_string()]),
        (vec![format!("{prefix}>"), ">".to_string()]),
        (vec![
            format!(
                r#"<<call create_calendar_event {{"title":"Retro","start":"2026-10-04T10:00:00+05:30""#
            ),
            r#"}>>"#.to_string(),
        ]),
        (vec![
            format!(
                r#"<<call create_calendar_event {{"title":"Retro","start":"2026-10-04T10:00:00+05:30""#
            ),
            "}".to_string(),
            ">".to_string(),
            ">".to_string(),
        ]),
    ];

    for (case_idx, chunks) in cases.into_iter().enumerate() {
        let mut decoder = StreamDecoder::new(tools.clone());
        let mut calls = Vec::new();
        for chunk in &chunks {
            calls.extend(decoder.push_chunk(chunk).expect("push chunk ok"));
        }
        calls.extend(decoder.finish().expect("finish ok"));

        assert_eq!(
            calls.len(),
            1,
            "Failed trailing delimiter split case {case_idx}: {chunks:?}"
        );
        assert_eq!(calls[0].function.name, "create_calendar_event");
    }
}

#[test]
fn test_adversarial_trailing_delimiter_with_whitespace() {
    let tools = create_full_test_tools();
    let text = r#"<<call get_status {}   >>"#;
    let calls = decode_calls(text, &tools).expect("should decode with whitespace before >>");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "get_status");
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Payload and Syntax Edge Cases
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_adversarial_unicode_and_emojis() {
    let tools = create_full_test_tools();
    let text = r#"<<call system_command {"title":"🚀 Review 💡","desc":"नमस्ते / こんにちは"}>>"#;

    let calls = decode_calls(text, &tools).expect("should decode unicode and emoji arguments");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "system_command");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "🚀 Review 💡");
    assert_eq!(args["desc"], "नमस्ते / こんにちは");
}

#[test]
fn test_adversarial_heavy_backslash_escaping() {
    let tools = create_full_test_tools();
    let text = r#"<<call system_command {"path":"C:\\Program Files\\test\""}>>"#;

    let calls = decode_calls(text, &tools).expect("should decode heavy backslash escaped string");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "system_command");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["path"], "C:\\Program Files\\test\"");
}

#[test]
fn test_adversarial_arrows_inside_arguments() {
    let tools = create_full_test_tools();
    let text = r#"<<call system_command {"query":"if a >> b then c"}>>"#;

    let calls = decode_calls(text, &tools).expect("should decode arrows inside argument string");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "system_command");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["query"], "if a >> b then c");
}

#[test]
fn test_adversarial_missing_required_fields() {
    let tools = create_full_test_tools();
    // Missing 'start' (required)
    let text = r#"<<call create_calendar_event {"title":"Design review"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    match err {
        ToolCompactError::InvalidArguments(msg) => {
            assert!(msg.contains("start"), "message: {msg}");
        }
        other => panic!("expected InvalidArguments, got {other:?}"),
    }
}

#[test]
fn test_adversarial_null_required_field_rejected() {
    let tools = create_full_test_tools();
    // 'start' provided as null
    let text = r#"<<call create_calendar_event {"title":"Design review","start":null}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    match err {
        ToolCompactError::InvalidArguments(msg) => {
            assert!(msg.contains("cannot be null"), "message: {msg}");
        }
        other => panic!("expected InvalidArguments for null required, got {other:?}"),
    }
}

#[test]
fn test_adversarial_extra_fields_additional_properties_false() {
    let tools = create_full_test_tools();
    // system_command has additionalProperties: false
    let text = r#"<<call system_command {"path":"/bin/sh","unexpected_extra_field":"danger"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    match err {
        ToolCompactError::InvalidArguments(msg) => {
            assert!(msg.contains("unexpected_extra_field"), "message: {msg}");
        }
        other => panic!("expected InvalidArguments for unknown property, got {other:?}"),
    }
}

#[test]
fn test_adversarial_invalid_enums() {
    let tools = create_full_test_tools();
    // visibility only allows "public" or "private"
    let text = r#"<<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30","visibility":"unlisted"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    match err {
        ToolCompactError::InvalidArguments(msg) => {
            assert!(msg.contains("visibility"), "message: {msg}");
        }
        other => panic!("expected InvalidArguments for enum violation, got {other:?}"),
    }
}

#[test]
fn test_adversarial_multiple_consecutive_tool_calls_no_text() {
    let tools = create_full_test_tools();
    let text = r#"<<call send_email {"to":["a@example.com"],"subject":"Hello","body":"World"}>> <<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30"}>>"#;

    let calls = decode_calls(text, &tools).expect("should decode multiple consecutive calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].function.name, "create_calendar_event");
}

#[test]
fn test_adversarial_multiple_consecutive_tool_calls_directly_adjacent() {
    let tools = create_full_test_tools();
    // Directly adjacent without spaces
    let text = r#"<<call send_email {"to":["a@example.com"],"subject":"Hello","body":"World"}><<call create_calendar_event {"title":"Review","start":"2026-10-05T15:00:00+05:30"}>>"#.replace("><<", ">><<");

    let calls = decode_calls(&text, &tools).expect("should decode directly adjacent calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].function.name, "create_calendar_event");
}

#[test]
fn test_adversarial_multiple_tool_calls_with_conversational_text_in_between() {
    let tools = create_full_test_tools();
    let text = r#"
First, I will notify the team:
<<call send_email {"to":["team@example.com"],"subject":"Sprint Sync","body":"Starting in 5 min"}>>
Next, I am placing it on your calendar:
<<call create_calendar_event {"title":"Sprint Sync","start":"2026-10-05T15:00:00+05:30"}>>
Both actions have been scheduled!
"#;

    let calls =
        decode_calls(text, &tools).expect("should decode calls separated by conversational text");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].function.name, "create_calendar_event");
}

#[test]
fn test_adversarial_plain_chat_text_no_tool_calls() {
    let tools = create_full_test_tools();
    let text = "Hello! Today is a sunny day in Bengaluru. The temperature is 28°C with clear skies. What else can I help you with?";

    let calls =
        decode_calls(text, &tools).expect("plain text should decode to empty vector without error");
    assert!(calls.is_empty());
}

#[test]
fn test_adversarial_plain_chat_with_math_and_bitshift_symbols() {
    let tools = create_full_test_tools();
    let text = "Remember that 5 < 10, and in C 1 << 4 equals 16. Also x >> 2 shifts right.";

    let calls =
        decode_calls(text, &tools).expect("plain text with <, <<, >> should not trigger calls");
    assert!(calls.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Schema Bypass Edge Cases (Polymorphic & Recursive)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_adversarial_unsupported_schema_constructs_bypass() {
    // oneOf
    let tool_oneof = ToolDef::new(
        "polymorphic_tool_1",
        Some("A tool using oneOf".to_string()),
        Some(json!({
            "type": "object",
            "oneOf": [{"type": "string"}, {"type": "integer"}]
        })),
    );
    assert!(matches!(
        encode_tools(&[tool_oneof]),
        Err(ToolCompactError::UnsupportedSchema(_))
    ));

    // anyOf
    let tool_anyof = ToolDef::new(
        "polymorphic_tool_2",
        Some("A tool using anyOf".to_string()),
        Some(json!({
            "type": "object",
            "properties": {
                "field": {
                    "anyOf": [{"type": "string"}, {"type": "number"}]
                }
            }
        })),
    );
    assert!(matches!(
        encode_tools(&[tool_anyof]),
        Err(ToolCompactError::UnsupportedSchema(_))
    ));

    // allOf
    let tool_allof = ToolDef::new(
        "composite_tool",
        Some("A tool using allOf".to_string()),
        Some(json!({
            "allOf": [{"type": "object"}]
        })),
    );
    assert!(matches!(
        encode_tools(&[tool_allof]),
        Err(ToolCompactError::UnsupportedSchema(_))
    ));

    // $ref
    let tool_ref = ToolDef::new(
        "recursive_tool",
        Some("A tool using $ref".to_string()),
        Some(json!({
            "type": "object",
            "properties": {
                "node": {
                    "$ref": "#/definitions/Node"
                }
            }
        })),
    );
    assert!(matches!(
        encode_tools(&[tool_ref]),
        Err(ToolCompactError::UnsupportedSchema(_))
    ));
}
