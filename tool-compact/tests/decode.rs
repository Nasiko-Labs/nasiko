mod common;

use common::*;
use nasiko_tool_compact::{
    CompactError, StreamDecoder, StreamEvent, ToolDef, decode, decode_calls,
};
use serde_json::{Value, json};

/// Calls as `[{name, arguments}]`, the eval's comparison shape.
fn calls(text: &str, tools: &[ToolDef]) -> Result<Value, String> {
    decode_calls(text, tools)
        .map(|cs| {
            cs.into_iter()
                .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                .collect()
        })
        .map_err(|e| e.kind().to_string())
}

fn stream(chunks: &[&str], tools: &[ToolDef]) -> Result<Value, String> {
    let mut d = StreamDecoder::new(tools).unwrap();
    let mut events = Vec::new();
    for c in chunks {
        events.extend(d.push(c).map_err(|e| e.kind().to_string())?);
    }
    events.extend(d.finish().map_err(|e| e.kind().to_string())?);
    Ok(events
        .into_iter()
        .filter_map(|e| match e {
            StreamEvent::Call(c) => Some(json!({"name": c.name, "arguments": c.arguments})),
            StreamEvent::Text(_) => None,
        })
        .collect())
}

// ─── The public decoder cases (compact-tools-eval@v1-sample) ────────────────

#[test]
fn dc_001_valid_single_call() {
    let out = stream(
        &[
            r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#,
        ],
        &[calendar()],
    );
    assert_eq!(
        out.unwrap(),
        json!([{"name":"create_calendar_event","arguments":{"title":"Design review","start":"2026-10-05T15:00:00+05:30"}}])
    );
}

#[test]
fn dc_002_marker_split_across_stream_chunks() {
    let out = stream(
        &[
            "<<ca",
            r#"ll create_calendar_event {"title":"Ret"#,
            r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#,
            ">",
        ],
        &[calendar()],
    );
    assert_eq!(
        out.unwrap(),
        json!([{"name":"create_calendar_event","arguments":{"title":"Retro","start":"2026-10-04T10:00:00+05:30"}}])
    );
}

#[test]
fn dc_003_close_marker_inside_a_string_argument() {
    let out = stream(
        &[r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#],
        &[email()],
    );
    assert_eq!(out.unwrap()[0]["arguments"]["subject"], "a >> b");
}

#[test]
fn dc_004_unknown_tool() {
    let out = stream(&["<<call delete_everything {}>>"], &[calendar()]);
    assert_eq!(out.unwrap_err(), "unknown_tool");
}

#[test]
fn dc_005_missing_required_field_and_bad_enum() {
    let out = stream(
        &[
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
        ],
        &[calendar()],
    );
    assert_eq!(out.unwrap_err(), "invalid_arguments");
}

// ─── Output shapes ──────────────────────────────────────────────────────────

#[test]
fn plain_answer_has_no_calls_and_keeps_its_text() {
    let d = decode("I can't check the weather, sorry.", &[calendar()]).unwrap();
    assert!(d.calls.is_empty());
    assert_eq!(d.text, "I can't check the weather, sorry.");
}

#[test]
fn text_before_between_and_after_calls_is_kept_in_order() {
    let text = "Sure.\n<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build\",\"body\":\"Green.\"}>>\nand\n<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"duration_min\":30,\"visibility\":\"private\"}>> Done.";
    let d = decode(text, &sample_tools()).unwrap();
    assert_eq!(d.calls.len(), 2);
    assert_eq!(d.calls[0].name, "send_email");
    assert_eq!(d.calls[1].name, "create_calendar_event");
    assert_eq!(d.text, "Sure.\n\nand\n Done.");
}

#[test]
fn whitespace_and_newlines_inside_a_call_are_tolerated() {
    let text = "<<call   create_calendar_event\n{\n  \"title\": \"A\",\n  \"start\": \"2026-10-05T15:00:00Z\"\n}\n>>";
    assert_eq!(
        calls(text, &[calendar()])
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn call_without_arguments_object_means_empty_arguments() {
    let ping = ToolDef {
        name: "ping".into(),
        description: None,
        parameters: None,
    };
    assert_eq!(
        calls("<<call ping>>", std::slice::from_ref(&ping)).unwrap(),
        json!([{"name":"ping","arguments":{}}])
    );
    // A tool declared without parameters takes no arguments.
    assert_eq!(
        calls(r#"<<call ping {"x":1}>>"#, &[ping]).unwrap_err(),
        "invalid_arguments"
    );
}

#[test]
fn arguments_are_returned_byte_for_byte_as_the_model_wrote_them() {
    let text = r#"<<call send_email {"to":["a@b.c"],"subject":"Ünïcode ✓ \"quoted\" }{ <<call x>>","body":"line1\nline2"}>>"#;
    let out = calls(text, &[email()]).unwrap();
    assert_eq!(
        out[0]["arguments"]["subject"],
        "Ünïcode ✓ \"quoted\" }{ <<call x>>"
    );
    assert_eq!(out[0]["arguments"]["body"], "line1\nline2");
}

// ─── Fail closed ────────────────────────────────────────────────────────────

fn kind(text: &str, tools: &[ToolDef]) -> String {
    calls(text, tools).unwrap_err()
}

#[test]
fn malformed_calls_are_errors_not_text() {
    let tools = [calendar()];
    for text in [
        "<<callcreate_calendar_event {}>>",
        "<<call {\"title\":\"x\"}>>",
        "<<call create_calendar_event [1]>>",
        "<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-05T15:00:00Z\"} >",
        "<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-05T15:00:00Z\"}",
        "<<call create_calendar_event {\"title\":\"A\"",
        "<<call create_calendar_event",
        "<<call",
    ] {
        assert_eq!(kind(text, &tools), "malformed_call", "{text}");
    }
}

#[test]
fn invalid_json_arguments_are_invalid_arguments() {
    assert_eq!(
        kind(
            r#"<<call create_calendar_event {"title":"A",}>>"#,
            &[calendar()]
        ),
        "invalid_arguments"
    );
    assert_eq!(
        kind(
            r#"<<call create_calendar_event {title:"A"}>>"#,
            &[calendar()]
        ),
        "invalid_arguments"
    );
}

#[test]
fn schema_violations_are_rejected_not_repaired() {
    let tools = [calendar()];
    let base = r#""title":"A","start":"2026-10-05T15:00:00+05:30""#;
    for extra in [
        r#","duration_min":"30""#,
        r#","duration_min":30.5"#,
        r#","attendees":"riya@example.com""#,
        r#","attendees":[1]"#,
        r#","visibility":"Private""#,
        r#","visibility":null"#,
        r#","duration_min":null"#,
    ] {
        let text = format!("<<call create_calendar_event {{{base}{extra}}}>>");
        assert_eq!(kind(&text, &tools), "invalid_arguments", "{extra}");
    }
    for start in [
        "2026-10-05",
        "Monday 3pm",
        "2026-10-05T15:00:00",
        "2026-02-30T10:00:00Z",
    ] {
        let text = format!(r#"<<call create_calendar_event {{"title":"A","start":"{start}"}}>>"#);
        assert_eq!(kind(&text, &tools), "invalid_arguments", "{start}");
    }
    let text = r#"<<call create_calendar_event {"title":7,"start":"2026-10-05T15:00:00Z"}>>"#;
    assert_eq!(kind(text, &tools), "invalid_arguments");
}

#[test]
fn integral_floats_are_integers_per_json_schema() {
    let text = r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z","duration_min":30.0}>>"#;
    let out = calls(text, &[calendar()]).unwrap();
    // Accepted, and passed through untouched (not rewritten to 30).
    assert_eq!(out[0]["arguments"]["duration_min"], json!(30.0));
}

#[test]
fn unknown_fields_follow_additional_properties() {
    // Open object (JSON Schema default): extra fields pass through.
    let open = r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z","room":"4B"}>>"#;
    assert!(calls(open, &[calendar()]).is_ok());
    // Closed object: rejected.
    let closed = r#"<<call place_order {"customer":{"id":"123e4567-e89b-12d3-a456-426614174000"},"items":[{"sku":"A1","color":"red"}]}>>"#;
    assert_eq!(kind(closed, &[order()]), "invalid_arguments");
}

#[test]
fn nested_objects_arrays_ranges_and_nullable_are_validated() {
    let ok = r#"<<call place_order {"customer":{"id":"123e4567-e89b-12d3-a456-426614174000","tier":"it's complicated"},"items":[{"sku":"A1","qty":3},{"sku":"B2"}],"note":null,"priority":2,"deliver_on":"2026-10-09","discount":0.25,"meta":{"any":"thing"}}>>"#;
    assert!(calls(ok, &[order()]).is_ok(), "{:?}", calls(ok, &[order()]));
    let id = r#""customer":{"id":"123e4567-e89b-12d3-a456-426614174000"}"#;
    for bad in [
        r#""customer":{"tier":"pro"},"items":[{"sku":"A"}]"#.to_string(),
        r#""customer":{"id":"not-a-uuid"},"items":[{"sku":"A"}]"#.to_string(),
        format!(r#"{id},"items":[{{"qty":1}}]"#),
        format!(r#"{id},"items":[{{"sku":"A","qty":0}}]"#),
        format!(r#"{id},"items":[{{"sku":"A","qty":100}}]"#),
        format!(r#"{id},"items":[{{"sku":"A"}}],"priority":4"#),
        format!(r#"{id},"items":[{{"sku":"A"}}],"discount":0.9"#),
        format!(r#"{id},"items":[{{"sku":"A"}}],"deliver_on":"2026-10-32""#),
        format!(r#"{id},"items":[{{"sku":"A"}}],"meta":[]"#),
        format!(r#"{id},"items":[{{"sku":"A"}}],"express":"yes""#),
        format!(r#"{id},"items":[{{"sku":"A"}}],"gift":true"#),
    ] {
        let text = format!("<<call place_order {{{bad}}}>>");
        assert_eq!(kind(&text, &[order()]), "invalid_arguments", "{bad}");
    }
}

#[test]
fn one_bad_call_fails_the_whole_output() {
    let text = "<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-05T15:00:00Z\"}>> <<call nope {}>>";
    assert_eq!(kind(text, &[calendar()]), "unknown_tool");
}

#[test]
fn errors_name_the_tool_and_path() {
    let text = r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z","attendees":["a",2]}>>"#;
    match decode_calls(text, &[calendar()]).unwrap_err() {
        CompactError::InvalidArguments { tool, path, .. } => {
            assert_eq!(tool, "create_calendar_event");
            assert_eq!(path, "$.attendees[1]");
        }
        other => panic!("{other:?}"),
    }
}

// ─── Streaming ──────────────────────────────────────────────────────────────

#[test]
fn text_is_released_early_but_a_partial_marker_is_held_back() {
    let mut d = StreamDecoder::new(&[calendar()]).unwrap();
    assert_eq!(
        d.push("Booking it now <").unwrap(),
        vec![StreamEvent::Text("Booking it now ".into())]
    );
    assert_eq!(d.push("<ca").unwrap(), vec![]);
    let done = d
        .push(r#"ll create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z"}>> ok"#)
        .unwrap();
    assert!(matches!(&done[0], StreamEvent::Call(c) if c.name == "create_calendar_event"));
    assert_eq!(done[1], StreamEvent::Text(" ok".into()));
    assert_eq!(d.finish().unwrap(), vec![]);
}

#[test]
fn a_lone_angle_bracket_is_text_once_the_stream_ends() {
    let mut d = StreamDecoder::new(&[calendar()]).unwrap();
    assert_eq!(
        d.push("x <<").unwrap(),
        vec![StreamEvent::Text("x ".into())]
    );
    assert_eq!(d.finish().unwrap(), vec![StreamEvent::Text("<<".into())]);
}

#[test]
fn unknown_tool_fails_as_soon_as_its_name_is_complete() {
    let mut d = StreamDecoder::new(&[calendar()]).unwrap();
    assert!(d.push("<<call delete_every").unwrap().is_empty());
    assert_eq!(d.push("thing {").unwrap_err().kind(), "unknown_tool");
    // The decoder stays failed.
    assert_eq!(d.push("}>>").unwrap_err().kind(), "unknown_tool");
}

#[test]
fn byte_by_byte_streaming_matches_whole_text() {
    let text = "Ok <<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b }\",\"body\":\"x\"}>> bye";
    let chars: Vec<String> = text.chars().map(String::from).collect();
    let chunks: Vec<&str> = chars.iter().map(String::as_str).collect();
    assert_eq!(stream(&chunks, &[email()]), calls(text, &[email()]));
}

// ─── Nullable enums, Pydantic `Optional`, duplicate keys, native calls ─────

fn one_field(schema: Value) -> ToolDef {
    tool(
        "f",
        "d",
        json!({"type": "object", "properties": {"v": schema}}),
    )
}

#[test]
fn a_nullable_type_does_not_add_null_to_an_enum() {
    // JSON Schema checks `type` and `enum` independently: null is not a member, so it fails.
    let t = one_field(json!({"type": ["string", "null"], "enum": ["public", "private"]}));
    assert_eq!(
        kind(r#"<<call f {"v":null}>>"#, std::slice::from_ref(&t)),
        "invalid_arguments"
    );
    assert!(calls(r#"<<call f {"v":"public"}>>"#, std::slice::from_ref(&t)).is_ok());
    let text = nasiko_tool_compact::encode_tools(&[t]).unwrap();
    assert_eq!(text.definitions(), "f(v?:public|private) - d");
    // With null listed as a member it is allowed, and shown.
    let t = one_field(json!({"type": ["string", "null"], "enum": ["public", null]}));
    assert!(calls(r#"<<call f {"v":null}>>"#, std::slice::from_ref(&t)).is_ok());
    let text = nasiko_tool_compact::encode_tools(&[t]).unwrap();
    assert_eq!(text.definitions(), "f(v?:public|null) - d");
}

#[test]
fn pydantic_optional_any_of_is_compacted_as_nullable() {
    let t = one_field(json!({
        "anyOf": [{"type": "integer", "minimum": 1}, {"type": "null"}],
        "default": null, "title": "V", "description": "Page size"
    }));
    let text = nasiko_tool_compact::encode_tools(std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        text.definitions(),
        "f(v?:int(1..)|null=null 'Page size') - d"
    );
    assert!(calls(r#"<<call f {"v":null}>>"#, std::slice::from_ref(&t)).is_ok());
    assert!(calls(r#"<<call f {"v":3}>>"#, std::slice::from_ref(&t)).is_ok());
    assert_eq!(
        kind(r#"<<call f {"v":0}>>"#, std::slice::from_ref(&t)),
        "invalid_arguments"
    );

    // Optional[Literal[...]]: null joins the enum.
    let t = one_field(json!({"anyOf": [{"enum": ["a", "b"], "type": "string"}, {"type": "null"}]}));
    assert_eq!(
        nasiko_tool_compact::encode_tools(std::slice::from_ref(&t))
            .unwrap()
            .definitions(),
        "f(v?:a|b|null) - d"
    );
    assert!(calls(r#"<<call f {"v":null}>>"#, &[t]).is_ok());
}

#[test]
fn other_any_of_shapes_still_bypass() {
    for schema in [
        json!({"anyOf": [{"type": "string"}, {"type": "integer"}]}),
        json!({"anyOf": [{"type": "null"}, {"type": "null"}]}),
        json!({"anyOf": [{"type": "string"}, {"type": "null"}], "minLength": 1}),
        json!({"anyOf": [{"type": ["string", "null"]}, {"type": "null"}]}),
        json!({"anyOf": [{"type": "string", "description": "a"}, {"type": "null"}], "description": "b"}),
    ] {
        let err = nasiko_tool_compact::encode_tools(&[one_field(schema.clone())]).unwrap_err();
        assert_eq!(err.kind(), "unsupported_schema", "{schema}");
    }
}

#[test]
fn repeated_keys_are_rejected_not_collapsed() {
    let text = r#"<<call send_email {"to":["a@example.com"],"subject":"x","body":"y","to":["b@example.com"]}>>"#;
    assert_eq!(kind(text, &[email()]), "invalid_arguments");
}

#[test]
fn validate_call_applies_the_same_rules_to_native_calls() {
    use nasiko_tool_compact::validate_call;
    let tools = [calendar()];
    let ok = validate_call(
        "create_calendar_event",
        r#"{"title":"A","start":"2026-10-05T15:00:00Z"}"#,
        &tools,
    )
    .unwrap();
    assert_eq!(
        ok.arguments_json(),
        r#"{"start":"2026-10-05T15:00:00Z","title":"A"}"#
    );
    for (name, args, want) in [
        ("nope", "{}", "unknown_tool"),
        (
            "create_calendar_event",
            r#"{"title":"A"}"#,
            "invalid_arguments",
        ),
        ("create_calendar_event", "not json", "invalid_arguments"),
        (
            "create_calendar_event",
            r#"{"title":"A","title":"B","start":"2026-10-05T15:00:00Z"}"#,
            "invalid_arguments",
        ),
    ] {
        assert_eq!(
            validate_call(name, args, &tools).unwrap_err().kind(),
            want,
            "{args}"
        );
    }
}
