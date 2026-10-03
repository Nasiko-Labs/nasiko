use serde_json::{Value, json};

use super::*;

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

fn tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

fn args(call: &ToolCall) -> Value {
    call.arguments_value().unwrap()
}

// ── encoding ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn renders_the_documented_signature() {
    let compact = encode_tools(&[calendar()]).unwrap();
    assert_eq!(
        compact.definitions,
        "create_calendar_event(title:str \"Event title\", start:datetime \"Start time, ISO 8601\", \
         attendees?:[str] \"Attendee emails\", duration_min?:int \"Duration in minutes\", \
         visibility?:public|private) - Create an event in the user's calendar."
    );
    assert!(compact.prompt().contains("<<call name {"));
}

#[test]
fn encoding_is_deterministic() {
    assert_eq!(
        encode_tools(&tools()).unwrap(),
        encode_tools(&tools()).unwrap()
    );
}

#[test]
fn decode_tools_recovers_the_original_schemas() {
    let original = tools();
    let back = decode_tools(&encode_tools(&original).unwrap()).unwrap();
    assert_eq!(back, original);
}

#[test]
fn nested_objects_arrays_defaults_and_int_enums_round_trip() {
    let tool = ToolDef {
        name: "book_trip".into(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "legs": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "from": {"type": "string", "description": "IATA code"},
                        "to": {"type": "string"},
                        "class": {"type": "string", "enum": ["economy", "business", "str"]}
                    },
                    "required": ["from", "to"]
                }},
                "seats": {"type": "integer", "enum": [1, 2, 4], "default": 1},
                "refundable": {"type": "boolean", "default": false},
                "budget": {"type": "number"},
                "mode": {"type": "string", "enum": ["only one"]},
                "notes": {"type": "string", "description": "Say \"hi\", then (stop)"}
            },
            "required": ["legs"]
        })),
    };
    let compact = encode_tools(std::slice::from_ref(&tool)).unwrap();
    assert!(
        compact.definitions.contains("economy|business|\"str\""),
        "reserved word must be quoted: {}",
        compact.definitions
    );
    assert!(compact.definitions.contains("mode?:\"only one\""));
    assert!(compact.definitions.contains("seats?:1|2|4=1"));
    assert_eq!(decode_tools(&compact).unwrap(), vec![tool]);
}

#[test]
fn no_argument_tools_are_supported() {
    for params in [
        None,
        Some(json!({})),
        Some(json!({"type": "object", "properties": {}})),
    ] {
        let tool = ToolDef {
            name: "ping".into(),
            description: None,
            parameters: params,
        };
        let compact = encode_tools(std::slice::from_ref(&tool)).unwrap();
        assert_eq!(compact.definitions, "ping()");
        let calls = decode_calls("<<call ping {}>>", &[tool]).unwrap();
        assert_eq!(calls[0].arguments, "{}");
    }
}

#[test]
fn unsupported_schema_features_bypass() {
    let cases = [
        json!({"type": "object", "properties": {"a": {"$ref": "#/defs/x"}}}),
        json!({"type": "object", "properties": {"a": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}}),
        json!({"type": "object", "properties": {"a": {"type": ["string", "null"]}}}),
        json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "^x$"}}}),
        json!({"type": "object", "properties": {"a": {"type": "integer", "minimum": 1}}}),
        json!({"type": "object", "properties": {"a": {"type": "string", "format": "ipv4"}}}),
        json!({"type": "object", "properties": {"a": {"type": "object"}}}),
        json!({"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": true}),
        json!({"type": "object", "properties": {"a": {"enum": ["x", 1]}}}),
        json!({"type": "object", "properties": {"bad name": {"type": "string"}}}),
        json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["b"]}),
        json!({"type": "array", "items": {"type": "string"}}),
    ];
    for params in cases {
        let tool = ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(params.clone()),
        };
        let err = encode_tools(&[tool]).unwrap_err();
        assert_eq!(err.code(), "unsupported_schema", "{params} should bypass");
    }
}

#[test]
fn duplicate_tool_names_bypass() {
    let err = encode_tools(&[calendar(), calendar()]).unwrap_err();
    assert_eq!(err.code(), "unsupported_schema");
}

#[test]
fn annotations_are_dropped_not_rejected() {
    let tool = ToolDef {
        name: "t".into(),
        description: None,
        parameters: Some(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "Args", "type": "object", "additionalProperties": false,
            "properties": {"a": {"title": "A", "type": "string"}}
        })),
    };
    assert_eq!(encode_tools(&[tool]).unwrap().definitions, "t(a?:str)");
}

// ── decoding ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn decodes_a_single_call() {
    let calls = decode_calls(
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(args(&calls[0])["attendees"], json!(["riya@example.com"]));
}

#[test]
fn decodes_multiple_calls_with_surrounding_text() {
    let out = decode(
        "Sure, doing both.\n<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build\",\"body\":\"Green.\"}>>\n\
         <<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"duration_min\":30,\"visibility\":\"private\"}>>\nDone.",
        &tools(),
    )
    .unwrap();
    let names: Vec<_> = out.calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["send_email", "create_calendar_event"]);
    assert_eq!(
        out.content().as_deref(),
        Some("Sure, doing both.\n\n\nDone.")
    );
}

#[test]
fn plain_answer_has_no_calls() {
    let out = decode("I can't check the weather, sorry.", &tools()).unwrap();
    assert!(out.calls.is_empty());
    assert_eq!(
        out.content().as_deref(),
        Some("I can't check the weather, sorry.")
    );
}

#[test]
fn calls_inside_a_code_fence_decode_and_the_empty_fence_is_dropped() {
    let out = decode(
        "```\n<<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\"}>>\n```",
        &tools(),
    )
    .unwrap();
    assert_eq!(out.calls.len(), 1);
    assert_eq!(out.content(), None);
}

#[test]
fn whitespace_and_pretty_printed_json_are_accepted_and_kept_verbatim() {
    let raw = "{\n  \"to\": [\"a@b.c\"],\n  \"subject\": \"s\",\n  \"body\": \"b\"\n}";
    let calls = decode_calls(&format!("<<call  send_email\n{raw}\n>>"), &tools()).unwrap();
    assert_eq!(calls[0].arguments, raw);
}

#[test]
fn close_marker_inside_a_string_is_not_the_end() {
    let calls = decode_calls(
        r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x}>> {<<call y {}>>"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(args(&calls[0])["subject"], "a >> b");
    assert_eq!(args(&calls[0])["body"], "x}>> {<<call y {}>>");
}

#[test]
fn escaped_quotes_inside_strings_are_handled() {
    let calls = decode_calls(
        r#"<<call send_email {"to":["a@b.c"],"subject":"say \"hi\" >>","body":"back\\slash"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(args(&calls[0])["subject"], "say \"hi\" >>");
    assert_eq!(args(&calls[0])["body"], "back\\slash");
}

#[test]
fn similar_prose_is_not_a_marker() {
    let out = decode("x << y, <<caller>> and <<call", &tools());
    // `<<call` at the very end with nothing after it is an unfinished opener, so it is text.
    assert_eq!(out.unwrap().text, "x << y, <<caller>> and <<call");
}

// ── fail-closed ──────────────────────────────────────────────────────────────────────────────

fn code_of(text: &str) -> &'static str {
    decode_calls(text, &tools()).unwrap_err().code()
}

#[test]
fn unknown_tool_is_an_error() {
    assert_eq!(code_of("<<call delete_everything {}>>"), "unknown_tool");
}

#[test]
fn missing_required_field_is_an_error() {
    assert_eq!(
        code_of(r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#),
        "invalid_arguments"
    );
}

#[test]
fn enum_violation_is_an_error() {
    assert_eq!(
        code_of(
            r#"<<call create_calendar_event {"title":"x","start":"s","visibility":"secret"}>>"#
        ),
        "invalid_arguments"
    );
}

#[test]
fn wrong_types_unknown_args_and_nulls_are_errors() {
    for bad in [
        r#"{"title":"x","start":"s","duration_min":"30"}"#,
        r#"{"title":"x","start":"s","duration_min":30.5}"#,
        r#"{"title":"x","start":"s","attendees":"a@b.c"}"#,
        r#"{"title":"x","start":"s","attendees":[1]}"#,
        r#"{"title":"x","start":"s","location":"HQ"}"#,
        r#"{"title":"x","start":"s","visibility":null}"#,
        r#"{"title":"x","start":"s",}"#,
    ] {
        assert_eq!(
            code_of(&format!("<<call create_calendar_event {bad}>>")),
            "invalid_arguments",
            "{bad}"
        );
    }
}

#[test]
fn integral_floats_are_integers() {
    decode_calls(
        r#"<<call create_calendar_event {"title":"x","start":"s","duration_min":30.0}>>"#,
        &tools(),
    )
    .unwrap();
}

#[test]
fn malformed_markers_are_errors() {
    for bad in [
        "<<call {}>>",
        "<<call send_email>>",
        "<<call send_email ()>>",
        "<<call send_email [1]>>",
        r#"<<call send_email {"to":["a"],"subject":"s","body":"b"} >"#,
        r#"<<call send_email {"to":["a"],"subject":"s","body":"b"}"#,
        r#"<<call send_email {"to":["a"],"subject":"s","body":"b"}x>>"#,
        r#"<<call send_email {"to":["a"],"subject":"s","bo"#,
    ] {
        assert_eq!(code_of(bad), "malformed_call", "{bad}");
    }
}

#[test]
fn one_bad_call_fails_the_whole_reply() {
    let text =
        "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\",\"body\":\"b\"}>>\n<<call nope {}>>";
    assert_eq!(code_of(text), "unknown_tool");
}

#[test]
fn decoder_refuses_tool_sets_it_could_not_have_encoded() {
    let tool = ToolDef {
        name: "t".into(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {"a": {"oneOf": []}}})),
    };
    assert_eq!(
        StreamDecoder::new(&[tool]).unwrap_err().code(),
        "unsupported_schema"
    );
}

// ── streaming ────────────────────────────────────────────────────────────────────────────────

fn stream(chunks: &[&str]) -> Result<Decoded> {
    let mut d = StreamDecoder::new(&tools())?;
    let mut events = Vec::new();
    for c in chunks {
        events.extend(d.push(c)?);
    }
    events.extend(d.finish()?);
    let mut out = Decoded::default();
    for e in events {
        match e {
            StreamEvent::Text(t) => out.text.push_str(&t),
            StreamEvent::Call(c) => out.calls.push(c),
        }
    }
    Ok(out)
}

#[test]
fn marker_split_across_chunks() {
    let out = stream(&[
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ])
    .unwrap();
    assert_eq!(out.calls.len(), 1);
    assert_eq!(args(&out.calls[0])["title"], "Retro");
}

#[test]
fn text_is_released_early_and_marker_prefixes_are_held() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert_eq!(
        d.push("Hello <").unwrap(),
        vec![StreamEvent::Text("Hello ".into())]
    );
    assert_eq!(d.push("<ca").unwrap(), vec![]);
    // Turned out not to be a marker: released as text.
    assert_eq!(
        d.push("t").unwrap(),
        vec![StreamEvent::Text("<<cat".into())]
    );
    assert_eq!(
        d.push(" <<call").unwrap(),
        vec![StreamEvent::Text(" ".into())]
    );
    assert_eq!(d.push(" send_email {\"to\":[\"a\"],").unwrap(), vec![]);
    let events = d.push("\"subject\":\"s\",\"body\":\"b\"}>> bye").unwrap();
    assert!(matches!(&events[0], StreamEvent::Call(c) if c.name == "send_email"));
    assert_eq!(events[1], StreamEvent::Text(" bye".into()));
    assert_eq!(d.finish().unwrap(), vec![]);
}

#[test]
fn stream_ending_inside_a_call_is_an_error() {
    let err = stream(&["<<call send_email {\"to\":"]).unwrap_err();
    assert_eq!(err.code(), "malformed_call");
}

#[test]
fn stream_stays_failed_after_an_error() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert!(d.push("<<call nope {}>>").is_err());
    assert_eq!(d.push("more").unwrap_err().code(), "unknown_tool");
    assert_eq!(d.finish().unwrap_err().code(), "unknown_tool");
}

#[test]
fn multibyte_text_survives_any_split() {
    let text = "Héllo → wörld 🎉 <<call send_email {\"to\":[\"a\"],\"subject\":\"日本\",\"body\":\"b\"}>> ✓";
    let whole = decode(text, &tools()).unwrap();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for &(i, _) in &chars {
        let (a, b) = text.split_at(i);
        let split = stream(&[a, b]).unwrap();
        assert_eq!(split, whole, "split at byte {i}");
    }
}

#[test]
fn render_calls_round_trips() {
    let calls = vec![
        call(
            "send_email",
            &json!({"to": ["x@y.z"], "subject": "a >> b", "body": "}{"}),
        ),
        call(
            "create_calendar_event",
            &json!({"title": "T", "start": "s"}),
        ),
    ];
    let text = render_calls(&calls);
    assert_eq!(decode_calls(&text, &tools()).unwrap(), calls);
}

#[test]
fn oversized_calls_are_rejected() {
    let big = "a".repeat(MAX_CALL_BYTES + 10);
    let err = stream(&["<<call send_email {\"to\":[\"", &big]).unwrap_err();
    assert_eq!(err.code(), "malformed_call");
}
