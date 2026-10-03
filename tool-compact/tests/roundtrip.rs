use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolDef, decode_calls, decode_tools, encode_tools, render_call,
};
use serde_json::json;

fn calendar() -> ToolDef {
    ToolDef::new(
        "create_calendar_event",
        Some("Create an event in the user's calendar.".into()),
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

fn email() -> ToolDef {
    ToolDef::new(
        "send_email",
        Some("Send an email from the user's account.".into()),
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
    )
}

#[test]
fn signature_shape() {
    let ct = encode_tools(&[calendar()]).unwrap();
    let block = ct.tools_block;
    assert!(block.contains("create_calendar_event("));
    assert!(block.contains("title:str"));
    assert!(block.contains("start:datetime"));
    assert!(block.contains("duration_min?:int"));
    assert!(block.contains("attendees?:[str]"));
    assert!(block.contains("visibility?:public|private"));
}

#[test]
fn schema_survives_roundtrip() {
    let tools = vec![calendar(), email()];
    let ct = encode_tools(&tools).unwrap();
    let back = decode_tools(&ct).unwrap();
    assert_eq!(back.len(), tools.len());
    for original in &tools {
        let rebuilt = back.iter().find(|t| t.name == original.name).expect("tool present");
        assert_eq!(rebuilt.description, original.description);
        let a = rebuilt.parameters.as_ref().unwrap();
        let b = original.parameters.as_ref().unwrap();
        assert_eq!(a["type"], b["type"]);
        assert_eq!(a["properties"], b["properties"]);
        let mut ra: Vec<_> =
            a["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let mut rb: Vec<_> =
            b["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        ra.sort_unstable();
        rb.sort_unstable();
        assert_eq!(ra, rb);
    }
}

#[test]
fn call_roundtrip() {
    let tools = vec![calendar()];
    let args = json!({"title": "Design review", "start": "2026-10-05T15:00:00+05:30"});
    let text = render_call("create_calendar_event", &args);
    let calls = decode_calls(&text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(parsed, args);
}

#[test]
fn multiple_calls_with_surrounding_text() {
    let tools = vec![calendar(), email()];
    let text = format!(
        "Sure, doing both.\n{}\nand then\n{}\nDone.",
        render_call("send_email", &json!({"to": ["sam@example.com"], "subject": "Build status", "body": "The build is green."})),
        render_call("create_calendar_event", &json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30", "duration_min": 30, "visibility": "private"})),
    );
    let calls = decode_calls(&text, &tools).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "send_email");
    assert_eq!(calls[1].name, "create_calendar_event");
}

#[test]
fn plain_answer_yields_no_calls() {
    let tools = vec![calendar()];
    assert!(decode_calls("I don't have weather access.", &tools).unwrap().is_empty());
    // A stray `<<` in prose must not be mistaken for a call.
    assert!(decode_calls("use a << b shift", &tools).unwrap().is_empty());
}

#[test]
fn close_marker_inside_string_argument() {
    let tools = vec![email()];
    let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(parsed["subject"], "a >> b");
}

#[test]
fn escaped_quotes_and_braces_in_strings() {
    let tools = vec![email()];
    let text = r#"<<call send_email {"to":["a@b.c"],"subject":"he said \"hi\"","body":"{not json} >> still"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(parsed["subject"], "he said \"hi\"");
    assert_eq!(parsed["body"], "{not json} >> still");
}

#[test]
fn unknown_tool_fails_closed() {
    let tools = vec![calendar()];
    let err = decode_calls("<<call delete_everything {}>>", &tools).unwrap_err();
    assert_eq!(err.code(), "unknown_tool");
    assert!(matches!(err, CompactError::UnknownTool { .. }));
}

#[test]
fn missing_required_and_bad_enum_fail_closed() {
    let tools = vec![calendar()];
    let text = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
}

#[test]
fn type_mismatch_and_unknown_field_fail_closed() {
    let tools = vec![calendar()];
    let bad_type = r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":"thirty"}>>"#;
    assert_eq!(decode_calls(bad_type, &tools).unwrap_err().code(), "invalid_arguments");

    let unknown = r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","colour":"red"}>>"#;
    assert_eq!(decode_calls(unknown, &tools).unwrap_err().code(), "invalid_arguments");

    let prose_date = r#"<<call create_calendar_event {"title":"x","start":"tomorrow 10am"}>>"#;
    assert_eq!(decode_calls(prose_date, &tools).unwrap_err().code(), "invalid_arguments");
}

#[test]
fn stream_marker_split_across_chunks() {
    let chunks = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let mut sd = StreamDecoder::new(vec![calendar()]);
    for c in chunks {
        sd.push(c).unwrap();
    }
    let (calls, _text) = sd.finish();
    assert_eq!(calls.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(parsed["title"], "Retro");
}

#[test]
fn stream_one_character_at_a_time() {
    let tools = vec![email()];
    let text = r#"ok: <<call send_email {"to":["a@b.c"],"subject":"a >> b","body":"x"}>> done"#;
    let mut sd = StreamDecoder::new(tools);
    for c in text.chars() {
        sd.push(&c.to_string()).unwrap();
    }
    let (calls, plain) = sd.finish();
    assert_eq!(calls.len(), 1);
    assert!(plain.contains("ok:"));
    assert!(plain.contains("done"));
}

#[test]
fn stream_truncated_call_is_not_guessed() {
    let mut sd = StreamDecoder::new(vec![calendar()]);
    sd.push("<<call create_calendar_event {\"title\":\"Ret").unwrap();
    let (calls, _) = sd.finish();
    assert!(calls.is_empty());
}

#[test]
fn unsupported_schema_is_rejected_for_bypass() {
    let tool = ToolDef::new(
        "weird",
        None,
        Some(json!({"type": "object", "properties": {"x": {"oneOf": [{"type": "string"}]}}})),
    );
    let err = encode_tools(&[tool]).unwrap_err();
    assert!(matches!(err, CompactError::Unsupported { .. }));
}
