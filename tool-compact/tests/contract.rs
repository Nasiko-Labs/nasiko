//! The public contract: encoding, schema recovery, decoding, streaming and failure policy.

use nasiko_tool_compact::{
    Error, StreamDecoder, StreamEvent, ToolCall, ToolDef, decode, decode_calls, decode_tools,
    encode_tools, render_calls,
};
use serde_json::{Value, json};

fn calendar() -> ToolDef {
    ToolDef {
        name: "create_calendar_event".into(),
        description: Some("Create an event in the user's calendar.".into()),
        parameters: Some(json!({
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
    }
}

fn email() -> ToolDef {
    ToolDef {
        name: "send_email".into(),
        description: Some("Send an email from the user's account.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                "subject": {"type": "string", "description": "Subject line"},
                "body": {"type": "string", "description": "Plain-text body"},
                "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
            },
            "required": ["to", "subject", "body"]
        })),
    }
}

fn nested() -> ToolDef {
    ToolDef {
        name: "create_order".into(),
        description: Some("Line one.\nLine two.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "items": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "sku": {"type": "string"},
                        "qty": {"type": "integer", "description": "How many"},
                        "tags": {"type": "array", "items": {"type": "string", "enum": ["gift", "fragile"]}}
                    },
                    "required": ["sku", "qty"]
                }},
                "priority": {"type": "integer", "enum": [1, 2, 3]},
                "mode": {"type": "string", "enum": ["str"]},
                "notify": {"type": "boolean"},
                "weight": {"type": "number"},
                "meta": {"type": "object"},
                "ship_to": {"type": "object", "properties": {
                    "city": {"type": "string"}, "zip": {"type": "string"}
                }, "required": ["city"]},
                "due": {"type": "string", "format": "date"},
                "odd key": {"type": "string"}
            },
            "required": ["items"]
        })),
    }
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        name: name.into(),
        arguments: args.to_string(),
    }
}

fn tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

#[test]
fn encodes_the_brief_example() {
    let c = encode_tools(&[calendar()]).unwrap();
    assert_eq!(
        c.definitions,
        "create_calendar_event(title:str 'Event title', start:datetime 'Start time, ISO 8601', attendees?:[str] 'Attendee emails', duration_min?:int 'Duration in minutes', visibility?:public|private) - Create an event in the user's calendar."
    );
    assert!(c.prompt().contains("<<call NAME {JSON args}>>"));
}

#[test]
fn decode_tools_recovers_the_schema_exactly() {
    for t in [calendar(), email(), nested()] {
        let back = decode_tools(&encode_tools(std::slice::from_ref(&t)).unwrap()).unwrap();
        assert_eq!(back, vec![t]);
    }
}

#[test]
fn unsupported_features_bypass() {
    let with = |props: Value| ToolDef {
        name: "t".into(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": props})),
    };
    for props in [
        json!({"a": {"$ref": "#/defs/x"}}),
        json!({"a": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}),
        json!({"a": {"type": ["string", "null"]}}),
        json!({"a": {"type": "string", "pattern": "^x"}}),
        json!({"a": {"type": "integer", "minimum": 0}}),
        json!({"a": {"type": "string", "format": "ipv4"}}),
        json!({"a": {"type": "array"}}),
        json!({"a": {"type": "string", "default": "x"}}),
        json!({"a": {"type": "string", "enum": ["x", 1]}}),
        json!({"a": {"type": "object", "properties": {}, "additionalProperties": false}}),
    ] {
        let err = encode_tools(&[with(props.clone())]).unwrap_err();
        assert_eq!(err.kind(), "unsupported_schema", "{props}");
    }
    let dup = encode_tools(&[calendar(), calendar()]).unwrap_err();
    assert_eq!(dup.kind(), "unsupported_schema");
}

#[test]
fn no_parameters_is_an_empty_signature() {
    let t = ToolDef {
        name: "ping".into(),
        description: None,
        parameters: None,
    };
    let c = encode_tools(std::slice::from_ref(&t)).unwrap();
    assert_eq!(c.definitions, "ping()");
    assert_eq!(
        decode_calls("<<call ping>>", std::slice::from_ref(&t)).unwrap()[0].arguments,
        "{}"
    );
    assert_eq!(decode_calls("<<call ping {}>>", &[t]).unwrap().len(), 1);
}

#[test]
fn decodes_multiple_calls_with_surrounding_text() {
    let out = decode(
        "Sure.\n<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build\",\"body\":\"Green.\"}>>\n\
         <<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"duration_min\":30}>>\nDone.",
        &tools(),
    )
    .unwrap();
    assert_eq!(out.calls.len(), 2);
    assert_eq!(out.calls[0].name, "send_email");
    assert_eq!(out.calls[1].arguments_value()["duration_min"], 30);
    assert_eq!(out.text, "Sure.\n\n\nDone.");
}

#[test]
fn plain_answer_has_no_calls() {
    let out = decode("I can't check the weather. a << b >> c", &tools()).unwrap();
    assert!(out.calls.is_empty());
    assert_eq!(out.text, "I can't check the weather. a << b >> c");
}

#[test]
fn arguments_are_byte_faithful() {
    let raw = r#"{"start":"2026-10-04T10:00:00+05:30", "title":"Retro"}"#;
    let calls = decode_calls(&format!("<<call create_calendar_event {raw}>>"), &tools()).unwrap();
    assert_eq!(calls[0].arguments, raw);
}

#[test]
fn escaping_inside_strings() {
    for subject in [
        "a >> b",
        "}>>",
        "<<call send_email {}>>",
        "quote \" and \\ backslash",
        "emoji 🎉 and ünïcode",
    ] {
        let args = json!({"to": ["sam@example.com"], "subject": subject, "body": "x"});
        let calls =
            decode_calls(&render_calls(&[call("send_email", args.clone())]), &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments_value(), args, "{subject}");
    }
}

#[test]
fn fail_closed() {
    let kind = |text: &str| decode(text, &tools()).unwrap_err().kind();
    assert_eq!(kind("<<call delete_everything {}>>"), "unknown_tool");
    // missing required
    assert_eq!(
        kind(r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#),
        "invalid_arguments"
    );
    // enum violation
    assert_eq!(
        kind(
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#
        ),
        "invalid_arguments"
    );
    // wrong type, unknown field, null for optional, bad format, duplicate key, not JSON
    for bad in [
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":"30"}"#,
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","location":"Room 1"}"#,
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":null}"#,
        r#"{"title":"x","start":"Monday 3pm"}"#,
        r#"{"title":"x","title":"y","start":"2026-10-05T15:00:00+05:30"}"#,
        r#"{title:"x"}"#,
    ] {
        assert_eq!(
            kind(&format!("<<call create_calendar_event {bad}>>")),
            "invalid_arguments",
            "{bad}"
        );
    }
    // malformed framing
    assert_eq!(kind("<<call {\"a\":1}>>"), "malformed_call");
    assert_eq!(kind("<<call send_email \"to\">>"), "malformed_call");
    assert_eq!(
        kind(r#"<<call send_email {"to":["a@b.co"],"subject":"s","body":"b"} >"#),
        "malformed_call"
    );
    assert_eq!(
        kind(r#"<<call send_email {"to":["a@b.co"],"subject":"s","body":"b"} junk>>"#),
        "malformed_call"
    );
    // a valid call followed by an invalid one fails the whole response
    assert_eq!(
        kind(r#"<<call send_email {"to":["a@b.co"],"subject":"s","body":"b"}>> <<call nope {}>>"#),
        "unknown_tool"
    );
}

#[test]
fn stream_split_at_every_byte_matches_whole_decode() {
    let text = "Ok <<call create_calendar_event {\"title\":\"Ret>>ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>> and <<call send_email {\"to\":[\"a@b.co\"],\"subject\":\"🎉\",\"body\":\"}\"}>> bye <";
    let whole = decode(text, &tools()).unwrap();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for &(cut, _) in &chars {
        let mut d = StreamDecoder::new(&tools()).unwrap();
        let mut events = d.push(&text[..cut]).unwrap();
        events.extend(d.push(&text[cut..]).unwrap());
        let streamed = d.finish().unwrap();
        assert_eq!(streamed, whole, "cut at {cut}");
        let calls: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Call(_)))
            .collect();
        assert_eq!(calls.len(), 2);
    }
    // one char per chunk
    let mut d = StreamDecoder::new(&tools()).unwrap();
    for (_, c) in chars {
        d.push(c.encode_utf8(&mut [0; 4])).unwrap();
    }
    assert_eq!(d.finish().unwrap(), whole);
}

#[test]
fn stream_holds_back_only_possible_markers() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert_eq!(
        d.push("Hello <<ca").unwrap(),
        vec![StreamEvent::Text("Hello ".into())]
    );
    assert_eq!(
        d.push("t>> ").unwrap(),
        vec![StreamEvent::Text("<<cat>> ".into())]
    );
}

#[test]
fn stream_fails_early_and_stays_failed() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert_eq!(
        d.push("<<call delete_everything ").unwrap_err(),
        Error::UnknownTool("delete_everything".into())
    );
    assert!(d.push("{}>>").is_err());
    assert!(d.finish().is_err());

    let mut d = StreamDecoder::new(&tools()).unwrap();
    d.push(r#"<<call send_email {"to":["#).unwrap();
    assert_eq!(d.finish().unwrap_err().kind(), "malformed_call");
}

#[test]
fn brief_decoder_case_dc_002() {
    let mut d = StreamDecoder::new(&[calendar()]).unwrap();
    for chunk in [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ] {
        d.push(chunk).unwrap();
    }
    let calls = d.finish().unwrap().calls;
    assert_eq!(
        calls[0].arguments_value(),
        json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30"})
    );
}
