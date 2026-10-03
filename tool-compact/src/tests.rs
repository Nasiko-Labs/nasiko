//! Unit and property tests for the compact tool encoding.

use serde_json::json;

use crate::{StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools};

fn calendar_tool() -> ToolDef {
    ToolDef::new(
        "create_calendar_event",
        Some("Create an event in the user's calendar.".to_string()),
        Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "start": {"type": "string", "format": "date-time"},
                "end": {"type": "string", "format": "date-time"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "duration_min": {"type": "integer"},
                "visibility": {"type": "string", "enum": ["default", "private"]}
            },
            "required": ["title", "start"]
        })),
    )
}

fn email_tool() -> ToolDef {
    ToolDef::new(
        "send_email",
        Some("Send an email.".to_string()),
        Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string"},
                "body": {"type": "string"}
            },
            "required": ["to", "subject", "body"]
        })),
    )
}

fn nested_tool() -> ToolDef {
    ToolDef::new(
        "deploy",
        Some("Deploy it.".to_string()),
        Some(json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "object",
                    "properties": {
                        "region": {"type": "string"},
                        "zones": {"type": "array", "items": {"type": "string"}},
                        "opts": {
                            "type": "object",
                            "properties": {
                                "retries": {"type": "integer"},
                                "dry_run": {"type": "boolean"}
                            },
                            "required": ["retries"]
                        }
                    },
                    "required": ["region"]
                },
                "when": {"type": "string", "format": "date"}
            },
            "required": ["target"]
        })),
    )
}

// ─── encode ─────────────────────────────────────────────────────────────────

#[test]
fn encode_sample_tools_exact_signatures() {
    let out = encode_tools(&[calendar_tool(), email_tool()]).unwrap();
    // properties render alphabetically (serde_json Map ordering)
    assert_eq!(
        out.rendered,
        "create_calendar_event(attendees?:[str], duration_min?:int, end?:datetime, start:datetime, title:str, visibility?:default|private) - Create an event in the user's calendar.\n\
         send_email(body:str, subject:str, to:[str]) - Send an email."
    );
    assert_eq!(out.compacted, vec![true, true]);
    assert!(out.instructions.contains("<<call tool_name"));
    assert!(
        out.instructions
            .contains("create_calendar_event(attendees?:[str]")
    );
}

#[test]
fn encode_nested_objects_and_formats() {
    let out = encode_tools(&[nested_tool()]).unwrap();
    assert_eq!(
        out.rendered,
        "deploy(target:{opts?:{dry_run?:bool, retries:int}, region:str, zones?:[str]}, when?:date) - Deploy it."
    );
}

#[test]
fn encode_no_parameters_and_no_description() {
    let out = encode_tools(&[ToolDef::new("ping", None, None)]).unwrap();
    assert_eq!(out.rendered, "ping()");
    assert_eq!(out.compacted, vec![true]);
}

#[test]
fn encode_bypasses_unsupported_schema() {
    let tool = ToolDef::new(
        "fancy",
        Some("Does fancy things.".to_string()),
        Some(json!({
            "type": "object",
            "properties": {"x": {"oneOf": [{"type": "string"}, {"type": "integer"}]}}
        })),
    );
    let out = encode_tools(&[tool]).unwrap();
    assert_eq!(out.compacted, vec![false]);
    assert_eq!(out.rendered, "fancy(?) - Does fancy things.");
}

#[test]
fn encode_rejects_bad_tool_name() {
    let tool = ToolDef::new("not a name", None, None);
    assert!(encode_tools(&[tool]).is_err());
}

#[test]
fn encode_collapses_multiline_description() {
    let tool = ToolDef::new(
        "t",
        Some("line one\nline two   line three".to_string()),
        None,
    );
    let out = encode_tools(&[tool]).unwrap();
    assert_eq!(out.rendered, "t() - line one line two line three");
}

// ─── decode ─────────────────────────────────────────────────────────────────

fn tools() -> Vec<ToolDef> {
    vec![calendar_tool(), email_tool()]
}

#[test]
fn decode_valid_single_call() {
    let text = r#"Here you go: <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>> done."#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], json!("Design review"));
}

#[test]
fn decode_multiple_calls() {
    let text = "<<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\"}>>\n<<call create_calendar_event {\"title\":\"t\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "send_email");
    assert_eq!(calls[1].name, "create_calendar_event");
}

#[test]
fn decode_plain_answer_is_empty() {
    assert_eq!(
        decode_calls("Just a normal answer.", &tools()).unwrap(),
        vec![]
    );
    assert_eq!(decode_calls("", &tools()).unwrap(), vec![]);
}

#[test]
fn decode_gt_inside_string() {
    let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls[0].arguments["subject"], json!("a >> b"));
}

#[test]
fn decode_escaped_quotes_and_braces_in_strings() {
    let text =
        r#"<<call send_email {"to":["a@b.c"],"subject":"say \"hi\" {not json}","body":"b"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(
        calls[0].arguments["subject"],
        json!("say \"hi\" {not json}")
    );
}

#[test]
fn decode_unknown_tool_fails_closed() {
    let err = decode_calls(r#"<<call delete_everything {}>>"#, &tools()).unwrap_err();
    assert_eq!(err.code(), "unknown_tool");
}

#[test]
fn decode_missing_required_field() {
    let err = decode_calls(
        r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
        &tools(),
    )
    .unwrap_err();
    // missing "title" is reported before the bad enum value
    assert_eq!(err.code(), "invalid_arguments");
    assert!(err.to_string().contains("title"), "{err}");
}

#[test]
fn decode_bad_enum_value() {
    let err = decode_calls(
        r#"<<call create_calendar_event {"title":"t","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
        &tools(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
}

#[test]
fn decode_wrong_type() {
    let err = decode_calls(
        r#"<<call create_calendar_event {"title":42,"start":"2026-10-05T15:00:00+05:30"}>>"#,
        &tools(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
}

#[test]
fn decode_non_object_args() {
    let err = decode_calls(r#"<<call send_email [1,2]>>"#, &tools()).unwrap_err();
    assert_eq!(err.code(), "malformed"); // no "{" after the name
}

#[test]
fn decode_unterminated_marker_is_malformed() {
    let err = decode_calls(r#"<<call send_email {"to":["a"]}"#, &tools()).unwrap_err();
    assert_eq!(err.code(), "malformed");
}

#[test]
fn decode_missing_closing_gt_is_malformed() {
    let err = decode_calls(
        r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b"}"#,
        &tools(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "malformed");
}

#[test]
fn decode_allows_extra_arguments() {
    let calls = decode_calls(
        r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b","cc":"x@y.z"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls.len(), 1);
}

#[test]
fn decode_nested_validation() {
    let tools = vec![nested_tool()];
    let ok = r#"<<call deploy {"target":{"region":"hyd","zones":["a"],"opts":{"retries":3}}}>>"#;
    assert_eq!(decode_calls(ok, &tools).unwrap().len(), 1);
    // missing nested required field
    let err = decode_calls(
        r#"<<call deploy {"target":{"region":"hyd","opts":{}}}>>"#,
        &tools,
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    // wrong nested type
    let err = decode_calls(
        r#"<<call deploy {"target":{"region":"hyd","opts":{"retries":"three"}}}>>"#,
        &tools,
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
}

#[test]
fn decode_first_error_wins_fail_closed() {
    let text = "<<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\"}>> <<call nope {}>>";
    let err = decode_calls(text, &tools()).unwrap_err();
    assert_eq!(err.code(), "unknown_tool");
}

// ─── stream ─────────────────────────────────────────────────────────────────

#[test]
fn stream_split_marker() {
    let chunks = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let mut d = StreamDecoder::new(&tools());
    for c in chunks {
        d.push(c);
    }
    let calls = d.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["title"], json!("Retro"));
}

#[test]
fn stream_gt_inside_string_across_chunks() {
    let chunks = [
        "<<call send_email {\"to\":[\"s@e.c\"],\"subject\":\"a >>",
        " b\",\"body\":\"x\"}>>",
    ];
    let mut d = StreamDecoder::new(&tools());
    for c in chunks {
        d.push(c);
    }
    let calls = d.finish().unwrap();
    assert_eq!(calls[0].arguments["subject"], json!("a >> b"));
}

#[test]
fn stream_error_surfaces_at_finish() {
    let mut d = StreamDecoder::new(&tools());
    d.push("hello ");
    d.push("<<call delete_everything {}>>");
    d.push(" trailing");
    let err = d.finish().unwrap_err();
    assert_eq!(err.code(), "unknown_tool");
}

#[test]
fn stream_plain_text_only() {
    let mut d = StreamDecoder::new(&tools());
    d.push("Just ");
    d.push("a normal answer.");
    assert_eq!(d.finish().unwrap(), vec![]);
}

#[test]
fn stream_dangling_marker_at_finish_is_malformed() {
    let mut d = StreamDecoder::new(&tools());
    d.push("<<call send_email {\"to\":[\"a\"");
    let err = d.finish().unwrap_err();
    assert_eq!(err.code(), "malformed");
}

#[test]
fn stream_prose_with_gt_gt_is_ignored() {
    let mut d = StreamDecoder::new(&tools());
    d.push("a >> b is fine");
    assert_eq!(d.finish().unwrap(), vec![]);
}

// ─── decode_tools round-trip ────────────────────────────────────────────────

#[test]
fn decode_tools_round_trip_is_stable() {
    let tools = vec![calendar_tool(), email_tool(), nested_tool()];
    let compact = encode_tools(&tools).unwrap();
    let back = decode_tools(&compact).unwrap();
    assert_eq!(back.len(), 3);
    for (orig, rt) in tools.iter().zip(back.iter()) {
        assert_eq!(orig.name, rt.name);
        assert_eq!(orig.description, rt.description);
    }
    // Re-encoding the decoded tools yields identical signature lines.
    let compact2 = encode_tools(&back).unwrap();
    assert_eq!(compact.rendered, compact2.rendered);
}

#[test]
fn decode_tools_bypassed_tool_has_no_schema() {
    let tool = ToolDef::new(
        "fancy",
        Some("Does fancy things.".to_string()),
        Some(json!({"type": "object", "properties": {"x": {"oneOf": [{"type": "string"}]}}})),
    );
    let compact = encode_tools(&[tool]).unwrap();
    let back = decode_tools(&compact).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].parameters, None);
}

/// Semantic check: decoded schemas accept exactly the same probe values as the
/// originals (valid values pass, invalid values fail on both sides).
#[test]
fn round_trip_validation_agrees() {
    let tools = vec![calendar_tool(), nested_tool()];
    let compact = encode_tools(&tools).unwrap();
    let back = decode_tools(&compact).unwrap();

    let probes: Vec<(&str, serde_json::Value, bool)> = vec![
        (
            "create_calendar_event",
            json!({"title": "t", "start": "2026-10-05T15:00:00+05:30"}),
            true,
        ),
        (
            "create_calendar_event",
            json!({"title": "t", "start": "2026-10-05T15:00:00+05:30", "visibility": "private"}),
            true,
        ),
        (
            "create_calendar_event",
            json!({"start": "2026-10-05T15:00:00+05:30"}),
            false,
        ),
        (
            "create_calendar_event",
            json!({"title": "t", "start": "2026-10-05T15:00:00+05:30", "visibility": "weird"}),
            false,
        ),
        (
            "create_calendar_event",
            json!({"title": "t", "start": "2026-10-05T15:00:00+05:30", "duration_min": "30"}),
            false,
        ),
        ("deploy", json!({"target": {"region": "r"}}), true),
        (
            "deploy",
            json!({"target": {"region": "r", "opts": {"retries": 2, "dry_run": true}}, "when": "2026-10-02"}),
            true,
        ),
        ("deploy", json!({"target": {"opts": {"retries": 2}}}), false),
        ("deploy", json!({"target": "r"}), false),
    ];
    for (name, args, valid) in probes {
        let orig = tools.iter().find(|t| t.name == name).unwrap();
        let rt = back.iter().find(|t| t.name == name).unwrap();
        let o = decode_calls(
            &format!("<<call {name} {}>>", serde_json::to_string(&args).unwrap()),
            std::slice::from_ref(orig),
        );
        let r = decode_calls(
            &format!("<<call {name} {}>>", serde_json::to_string(&args).unwrap()),
            std::slice::from_ref(rt),
        );
        assert_eq!(o.is_ok(), valid, "original disagrees on {args}");
        assert_eq!(r.is_ok(), valid, "round-tripped disagrees on {args}");
    }
}

// ─── property-style: generated schemas ──────────────────────────────────────

/// Build a battery of schemas from a small grammar and check the round trip is
/// stable and validation agrees on probe values.
#[test]
fn generated_schema_round_trips() {
    fn atom(i: usize) -> serde_json::Value {
        match i % 7 {
            0 => json!({"type": "string"}),
            1 => json!({"type": "integer"}),
            2 => json!({"type": "number"}),
            3 => json!({"type": "boolean"}),
            4 => json!({"type": "string", "format": "date-time"}),
            5 => json!({"type": "string", "enum": ["a", "b-c", "d e"]}),
            _ => json!({"type": "array", "items": {"type": "string"}}),
        }
    }
    fn obj(depth: usize, seed: usize) -> serde_json::Value {
        let mut props = serde_json::Map::new();
        for k in 0..3 {
            let key = format!("f{depth}_{k}");
            let v = if depth < 2 && (seed + k).is_multiple_of(3) {
                obj(depth + 1, seed + k)
            } else {
                atom(seed + k)
            };
            props.insert(key, v);
        }
        let mut o = serde_json::Map::new();
        o.insert("type".to_string(), json!("object"));
        o.insert("properties".to_string(), serde_json::Value::Object(props));
        o.insert("required".to_string(), json!([format!("f{depth}_0")]));
        serde_json::Value::Object(o)
    }

    for seed in 0..25 {
        let tool = ToolDef::new(
            format!("tool_{seed}"),
            Some(format!("Tool number {seed}.")),
            Some(obj(0, seed)),
        );
        let compact = encode_tools(std::slice::from_ref(&tool)).unwrap();
        assert_eq!(compact.compacted, vec![true]);
        let back = decode_tools(&compact).unwrap();
        assert_eq!(back.len(), 1);
        // Stable: re-encoding the decoded form is byte-identical.
        let compact2 = encode_tools(&back).unwrap();
        assert_eq!(compact.rendered, compact2.rendered, "seed {seed}");
        // Decoded schema validates the same probe values as the original.
        let probe = json!({"f0_0": "x"});
        let mk = |t: &ToolDef| {
            decode_calls(
                &format!(
                    "<<call {} {}>>",
                    t.name,
                    serde_json::to_string(&probe).unwrap()
                ),
                std::slice::from_ref(t),
            )
            .is_ok()
        };
        assert_eq!(mk(&tool), mk(&back[0]), "seed {seed}");
    }
}

#[test]
fn tool_call_serde_shape() {
    let call = ToolCall::new("send_email", json!({"to": ["a@b.c"]}));
    let v = serde_json::to_value(&call).unwrap();
    assert_eq!(v["name"], json!("send_email"));
    assert_eq!(v["arguments"]["to"], json!(["a@b.c"]));
}
