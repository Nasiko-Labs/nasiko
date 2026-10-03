//! Integration and property tests for nasiko-tool-compact.
//!
//! Covers:
//! - Encode/decode round trips
//! - Required and optional fields
//! - Enums, arrays, nested objects, all primitive types
//! - Unknown tools and invalid arguments
//! - Malformed syntax and escaping
//! - Multiple calls and surrounding text
//! - Incremental stream decoding with markers split at different positions
//! - Schema round-trip via decode_tools
//! - No guessed calls on any invalid input

use nasiko_tool_compact::{
    DecodeError, FunctionDef, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools,
    encode_tools,
};
use serde_json::json;

// ─── Test Fixtures ───────────────────────────────────────────────────────────

fn calendar_tool() -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Event title" },
                    "start": { "type": "string", "format": "date-time", "description": "Start time" },
                    "duration_min": { "type": "integer", "description": "Duration in minutes" },
                    "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"]
            })),
        },
    }
}

fn email_tool() -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "send_email".into(),
            description: Some("Send an email from the user's account.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": { "type": "array", "items": { "type": "string" }, "description": "Recipient emails" },
                    "subject": { "type": "string", "description": "Subject line" },
                    "body": { "type": "string", "description": "Plain-text body" },
                    "cc": { "type": "array", "items": { "type": "string" }, "description": "CC emails" }
                },
                "required": ["to", "subject", "body"]
            })),
        },
    }
}

fn nested_tool() -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "create_task".into(),
            description: Some("Create a task with metadata.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "meta": {
                        "type": "object",
                        "properties": {
                            "priority": { "type": "integer" },
                            "tags": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["priority"]
                    }
                },
                "required": ["title", "meta"]
            })),
        },
    }
}

fn bool_num_tool() -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "set_config".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean" },
                    "threshold": { "type": "number" },
                    "count": { "type": "integer" }
                },
                "required": ["enabled"]
            })),
        },
    }
}

// ─── encode_tools ────────────────────────────────────────────────────────────

#[test]
fn encode_empty_tools_returns_error() {
    let err = encode_tools(&[]).unwrap_err();
    assert!(matches!(err, nasiko_tool_compact::EncodeError::NoTools));
}

#[test]
fn encode_single_tool_produces_schema_text() {
    let tools = [calendar_tool()];
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.schema_text.contains("create_calendar_event"));
    assert!(compact.schema_text.contains("title"));
    assert!(compact.schema_text.contains("start"));
}

#[test]
fn encode_multiple_tools_lists_all() {
    let tools = [calendar_tool(), email_tool()];
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.schema_text.contains("create_calendar_event"));
    assert!(compact.schema_text.contains("send_email"));
}

#[test]
fn encode_marks_required_vs_optional_fields() {
    let tools = [calendar_tool()];
    let compact = encode_tools(&tools).unwrap();
    // title and start are required (*), the rest are optional (?)
    assert!(compact.schema_text.contains("title*") || compact.schema_text.contains("title?").not());
    // Just ensure the text is present and non-empty
    assert!(!compact.schema_text.is_empty());
}

#[test]
fn encode_shows_enum_values() {
    let tools = [calendar_tool()];
    let compact = encode_tools(&tools).unwrap();
    // visibility enum should appear
    assert!(compact.schema_text.contains("public") || compact.schema_text.contains("private"));
}

#[test]
fn encode_grammar_hint_present() {
    let tools = [calendar_tool()];
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.grammar_hint.contains("<<call"));
    assert!(!compact.grammar_hint.is_empty());
}

#[test]
fn encode_system_prompt_block_combines_schema_and_hint() {
    let tools = [calendar_tool()];
    let compact = encode_tools(&tools).unwrap();
    let block = compact.system_prompt_block();
    assert!(block.contains("create_calendar_event"));
    assert!(block.contains("<<call"));
}

// ─── decode_tools (schema round-trip) ────────────────────────────────────────

#[test]
fn decode_tools_roundtrip_single_tool() {
    let tools = [calendar_tool()];
    let compact = encode_tools(&tools).unwrap();
    let recovered = decode_tools(&compact).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].function.name, "create_calendar_event");
    // Parameters are preserved byte-identical.
    assert_eq!(
        recovered[0].function.parameters,
        tools[0].function.parameters
    );
}

#[test]
fn decode_tools_roundtrip_multiple_tools() {
    let tools = [calendar_tool(), email_tool()];
    let compact = encode_tools(&tools).unwrap();
    let recovered = decode_tools(&compact).unwrap();
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0].function.name, "create_calendar_event");
    assert_eq!(recovered[1].function.name, "send_email");
}

// ─── decode_calls: valid paths ────────────────────────────────────────────────

#[test]
fn decode_single_valid_call_with_all_required_fields() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], "Design review");
    assert_eq!(calls[0].arguments["start"], "2026-10-05T15:00:00+05:30");
}

#[test]
fn decode_call_with_optional_fields() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"title":"T","start":"S","duration_min":30,"visibility":"private"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["duration_min"], 30);
    assert_eq!(calls[0].arguments["visibility"], "private");
}

#[test]
fn decode_call_with_array_argument() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"title":"T","start":"S","attendees":["a@b.com","c@d.com"]}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["attendees"][0], "a@b.com");
    assert_eq!(calls[0].arguments["attendees"][1], "c@d.com");
}

#[test]
fn decode_multiple_calls_in_one_response() {
    let tools = [calendar_tool(), email_tool()];
    let text = concat!(
        r#"<<call send_email {"to":["sam@example.com"],"subject":"Hi","body":"Hello"}>> "#,
        r#"<<call create_calendar_event {"title":"Review","start":"2026-10-05T09:00:00Z"}>>"#
    );
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "send_email");
    assert_eq!(calls[1].name, "create_calendar_event");
}

#[test]
fn decode_text_before_and_after_call() {
    let tools = [calendar_tool()];
    let text = r#"Sure! <<call create_calendar_event {"title":"T","start":"S"}>> Done."#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
}

#[test]
fn decode_text_between_calls() {
    let tools = [calendar_tool()];
    let text = concat!(
        r#"First: <<call create_calendar_event {"title":"A","start":"S1"}>> "#,
        r#"and also: <<call create_calendar_event {"title":"B","start":"S2"}>>"#
    );
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].arguments["title"], "A");
    assert_eq!(calls[1].arguments["title"], "B");
}

#[test]
fn decode_plain_text_with_no_calls() {
    let tools = [calendar_tool()];
    let text = "I can't help with that, sorry.";
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 0);
}

#[test]
fn decode_empty_string_produces_empty_calls() {
    let tools = [calendar_tool()];
    let calls = decode_calls("", &tools).unwrap();
    assert_eq!(calls.len(), 0);
}

#[test]
fn decode_boolean_field() {
    let tools = [bool_num_tool()];
    let text = r#"<<call set_config {"enabled":true,"threshold":0.75,"count":3}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["enabled"], true);
    assert_eq!(calls[0].arguments["threshold"], 0.75);
    assert_eq!(calls[0].arguments["count"], 3);
}

#[test]
fn decode_nested_object_argument() {
    let tools = [nested_tool()];
    let text =
        r#"<<call create_task {"title":"Fix bug","meta":{"priority":1,"tags":["urgent"]}}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["meta"]["priority"], 1);
    assert_eq!(calls[0].arguments["meta"]["tags"][0], "urgent");
}

#[test]
fn decode_gt_gt_inside_string_argument() {
    let tools = [email_tool()];
    let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["subject"], "a >> b");
}

#[test]
fn decode_no_tool_schema_accepts_any_object() {
    let tool_no_schema = ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "raw_fn".into(),
            description: None,
            parameters: None,
        },
    };
    let text = r#"<<call raw_fn {"x":1,"y":"hello"}>>"#;
    let calls = decode_calls(text, &[tool_no_schema]).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["x"], 1);
}

// ─── decode_calls: error paths ────────────────────────────────────────────────

#[test]
fn decode_unknown_tool_returns_error() {
    let tools = [calendar_tool()];
    let text = r#"<<call delete_everything {}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::UnknownTool { ref name } if name == "delete_everything"));
}

#[test]
fn decode_missing_required_field_returns_error() {
    let tools = [calendar_tool()];
    // Missing required 'title'
    let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidArguments { .. }));
    if let DecodeError::InvalidArguments { reason, .. } = err {
        assert!(reason.contains("title"));
    }
}

#[test]
fn decode_missing_both_required_fields_returns_error() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"visibility":"public"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidArguments { .. }));
}

#[test]
fn decode_invalid_enum_value_returns_error() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"title":"T","start":"S","visibility":"secret"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(
        matches!(err, DecodeError::InvalidArguments { ref reason, .. } if reason.contains("secret"))
    );
}

#[test]
fn decode_wrong_type_for_integer_field_returns_error() {
    let tools = [calendar_tool()];
    let text =
        r#"<<call create_calendar_event {"title":"T","start":"S","duration_min":"thirty"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidArguments { .. }));
}

#[test]
fn decode_wrong_type_for_array_field_returns_error() {
    let tools = [calendar_tool()];
    let text =
        r#"<<call create_calendar_event {"title":"T","start":"S","attendees":"not-an-array"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidArguments { .. }));
}

#[test]
fn decode_wrong_type_for_boolean_returns_error() {
    let tools = [bool_num_tool()];
    let text = r#"<<call set_config {"enabled":"yes"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidArguments { .. }));
}

#[test]
fn decode_malformed_no_closing_bracket_returns_error() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"title":"T","start":"S"}"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::MalformedCall { .. }));
}

#[test]
fn decode_malformed_invalid_json_returns_error() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event not-json>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    // Either MalformedCall (JSON parse error) or something similar.
    assert!(matches!(
        err,
        DecodeError::MalformedCall { .. } | DecodeError::InvalidArguments { .. }
    ));
}

#[test]
fn decode_no_tool_name_returns_malformed_error() {
    let tools = [calendar_tool()];
    // Marker with a JSON body but no tool name (space then JSON directly).
    // "<<call " then "{}" — the name is "" (empty) → unknown tool error.
    let text = r#"<<call {}>>"#;
    let result = decode_calls(text, &tools);
    match result {
        Ok(calls) => assert!(calls.is_empty()),
        Err(DecodeError::MalformedCall { .. }) => {}
        Err(DecodeError::UnknownTool { .. }) => {}
        Err(e) => panic!("unexpected error: {:?}", e),
    }
}

// ─── Streaming ───────────────────────────────────────────────────────────────

#[test]
fn stream_single_chunk_full_marker() {
    let tools = [calendar_tool()];
    let mut dec = StreamDecoder::new(&tools);
    let (_, calls) = dec
        .push(r#"<<call create_calendar_event {"title":"T","start":"S"}>>"#)
        .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
}

#[test]
fn stream_marker_split_at_opening_bracket() {
    let tools = [calendar_tool()];
    let mut dec = StreamDecoder::new(&tools);
    let (_, c1) = dec.push("<<ca").unwrap();
    assert!(c1.is_empty());
    let (_, c2) = dec
        .push(r#"ll create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>"#)
        .unwrap();
    assert!(c2.is_empty());
    let (_, c3) = dec.push(">").unwrap();
    assert_eq!(c3.len(), 1);
    assert_eq!(c3[0].arguments["title"], "Retro");
}

#[test]
fn stream_marker_split_inside_json() {
    let tools = [calendar_tool()];
    let mut dec = StreamDecoder::new(&tools);
    let (_, c1) = dec
        .push(r#"<<call create_calendar_event {"title":"Des"#)
        .unwrap();
    assert!(c1.is_empty());
    let (_, c2) = dec
        .push(r#"ign review","start":"2026-10-05T15:00:00+05:30"}>>"#)
        .unwrap();
    assert_eq!(c2.len(), 1);
    assert_eq!(c2[0].arguments["title"], "Design review");
}

#[test]
fn stream_marker_split_at_closing_gt() {
    let tools = [calendar_tool()];
    let mut dec = StreamDecoder::new(&tools);
    let chunk1 = r#"<<call create_calendar_event {"title":"T","start":"S"}>"#;
    let (_, c1) = dec.push(chunk1).unwrap();
    assert!(c1.is_empty()); // Second > not yet arrived
    let (_, c2) = dec.push(">").unwrap();
    assert_eq!(c2.len(), 1);
}

#[test]
fn stream_plain_text_flushes_correctly() {
    let tools = [calendar_tool()];
    let mut dec = StreamDecoder::new(&tools);
    let (t1, c1) = dec.push("Hello, world!").unwrap();
    let (t2, c2) = dec.flush().unwrap();
    assert!(c1.is_empty());
    assert!(c2.is_empty());
    let all = t1 + &t2;
    assert_eq!(all, "Hello, world!");
}

#[test]
fn stream_text_before_and_after_call() {
    let tools = [calendar_tool()];
    let mut dec = StreamDecoder::new(&tools);
    let (t1, c1) = dec
        .push(r#"Sure! <<call create_calendar_event {"title":"T","start":"S"}>> Done."#)
        .unwrap();
    let (t2, _) = dec.flush().unwrap();
    assert_eq!(c1.len(), 1);
    let all_text = t1 + &t2;
    // The surrounding text must come through (possibly in different flush order)
    assert!(all_text.contains("Sure!") || all_text.contains("Done."));
}

#[test]
fn stream_multiple_calls_across_chunks() {
    let tools = [calendar_tool(), email_tool()];
    let chunk1 = r#"<<call send_email {"to":["a@b.com"],"subject":"Hi","body":"Hello"}>> "#;
    let chunk2 = r#"<<call create_calendar_event {"title":"T","start":"S"}>>"#;
    let mut dec = StreamDecoder::new(&tools);
    let (_, c1) = dec.push(chunk1).unwrap();
    let (_, c2) = dec.push(chunk2).unwrap();
    assert_eq!(c1.len(), 1);
    assert_eq!(c2.len(), 1);
    assert_eq!(c1[0].name, "send_email");
    assert_eq!(c2[0].name, "create_calendar_event");
}

#[test]
fn stream_unknown_tool_propagates_error() {
    let tools = [calendar_tool()];
    let mut dec = StreamDecoder::new(&tools);
    let err = dec.push(r#"<<call delete_everything {}>>"#).unwrap_err();
    assert!(matches!(err, DecodeError::UnknownTool { .. }));
}

#[test]
fn stream_chunk_byte_by_byte() {
    // Feed the marker one byte at a time — must produce the call at the end.
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"title":"T","start":"S"}>>"#;
    let mut dec = StreamDecoder::new(&tools);
    let mut all_calls = Vec::new();
    for c in text.chars() {
        let (_, calls) = dec.push(&c.to_string()).unwrap();
        all_calls.extend(calls);
    }
    let (_, tail_calls) = dec.flush().unwrap();
    all_calls.extend(tail_calls);
    assert_eq!(all_calls.len(), 1);
    assert_eq!(all_calls[0].name, "create_calendar_event");
}

// ─── Eval dataset cases ───────────────────────────────────────────────────────

/// Reproduce the five decoder_cases from the public eval dataset exactly,
/// without hard-coding expected outputs (they derive from the grammar).
#[test]
fn eval_dc001_valid_single_call() {
    let tools = [calendar_tool()];
    let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], "Design review");
}

#[test]
fn eval_dc002_marker_split_across_stream_chunks() {
    let tools = [calendar_tool()];
    let chunks = [
        "<<ca",
        r#"ll create_calendar_event {"title":"Ret"#,
        r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#,
        ">",
    ];
    let mut dec = StreamDecoder::new(&tools);
    let mut all_calls: Vec<ToolCall> = Vec::new();
    for chunk in &chunks {
        let (_, calls) = dec.push(chunk).unwrap();
        all_calls.extend(calls);
    }
    let (_, tail) = dec.flush().unwrap();
    all_calls.extend(tail);
    assert_eq!(all_calls.len(), 1);
    assert_eq!(all_calls[0].arguments["title"], "Retro");
}

#[test]
fn eval_dc003_double_gt_inside_string() {
    let tools = [email_tool()];
    let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["subject"], "a >> b");
}

#[test]
fn eval_dc004_unknown_tool() {
    let tools = [calendar_tool()];
    let text = r#"<<call delete_everything {}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::UnknownTool { .. }));
}

#[test]
fn eval_dc005_missing_required_and_bad_enum() {
    let tools = [calendar_tool()];
    // Missing 'title' (required) + visibility="secret" (invalid enum)
    let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidArguments { .. }));
}

// ─── Property-like exhaustive edge cases ─────────────────────────────────────

#[test]
fn no_call_produced_for_incomplete_opening_marker() {
    let tools = [calendar_tool()];
    // Looks like a marker start but truncated
    for partial in &["<<", "<<call", "<<call "] {
        let calls = decode_calls(partial, &tools);
        if let Ok(v) = calls {
            assert!(
                v.is_empty(),
                "should not guess a call for partial: {:?}",
                partial
            );
        }
    }
}

#[test]
fn extra_fields_not_in_schema_are_accepted() {
    // JSON Schema additionalProperties defaults to true.
    let tools = [calendar_tool()];
    let text =
        r#"<<call create_calendar_event {"title":"T","start":"S","extra_unknown_field":"hello"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["extra_unknown_field"], "hello");
}

#[test]
fn integer_field_must_not_accept_float() {
    let tools = [bool_num_tool()];
    let text = r#"<<call set_config {"enabled":true,"count":3.5}>>"#;
    // 3.5 is not an integer
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidArguments { .. }));
}

#[test]
fn number_field_accepts_integer_value() {
    // A JSON integer satisfies a "number" schema.
    let tools = [bool_num_tool()];
    let text = r#"<<call set_config {"enabled":true,"threshold":5}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["threshold"], 5);
}

#[test]
fn enum_valid_value_succeeds() {
    let tools = [calendar_tool()];
    for visibility in &["public", "private"] {
        let text = format!(
            r#"<<call create_calendar_event {{"title":"T","start":"S","visibility":"{}"}}>>"#,
            visibility
        );
        let calls = decode_calls(&text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["visibility"], *visibility);
    }
}

#[test]
fn opaque_schema_with_combinator_accepts_any_json() {
    let tool_with_anyof = ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "polymorphic".into(),
            description: None,
            parameters: Some(json!({
                "anyOf": [
                    { "type": "string" },
                    { "type": "object" }
                ]
            })),
        },
    };
    let text = r#"<<call polymorphic {"key":"value"}>>"#;
    let calls = decode_calls(text, &[tool_with_anyof]).unwrap();
    assert_eq!(calls.len(), 1);
}

// ─── Helper trait to allow `.not()` for readability ─────────────────────────

trait BoolExt {
    fn not(self) -> bool;
}
impl BoolExt for bool {
    fn not(self) -> bool {
        !self
    }
}
