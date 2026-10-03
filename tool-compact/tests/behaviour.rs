//! Behaviour tests for `nasiko-tool-compact`: encoding, fail-closed decoding, streaming across
//! split markers, schema round-trips, and a deterministic property test.

use serde_json::{json, Value};

use nasiko_tool_compact::{
    decode_calls, decode_tools, encode_tools, CompactError, StreamDecoder, ToolCall, ToolDef,
};

/// The `create_calendar_event` tool from the public eval set.
fn calendar_tool() -> ToolDef {
    ToolDef {
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
    }
}

fn tools() -> Vec<ToolDef> {
    vec![calendar_tool()]
}

#[test]
fn encodes_signature_with_types_optionality_and_enum() {
    let compact = encode_tools(&tools()).unwrap();
    let block = compact.tools_block;
    assert!(block.contains("create_calendar_event("), "{block}");
    assert!(block.contains("title:str"), "{block}");
    assert!(block.contains("start:datetime"), "{block}");
    assert!(block.contains("duration_min?:int"), "{block}");
    assert!(block.contains("attendees?:[str]"), "{block}");
    assert!(block.contains("visibility?:public|private"), "{block}");
    assert!(block.contains(" - Create an event"), "{block}");
    // Required args carry no `?`.
    assert!(!block.contains("title?:"), "{block}");
}

#[test]
fn decodes_single_call() {
    let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], json!("Design review"));
}

#[test]
fn decodes_multiple_calls_with_text_between() {
    let email = ToolDef {
        name: "send_email".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"to": {"type": "array", "items": {"type": "string"}}, "subject": {"type": "string"}},
            "required": ["to"]
        })),
    };
    let tools = vec![calendar_tool(), email];
    let text = r#"Sure. <<call send_email {"to":["sam@example.com"],"subject":"hi"}>> and also <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>> done."#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "send_email");
    assert_eq!(calls[1].name, "create_calendar_event");
}

#[test]
fn plain_answer_yields_no_calls() {
    let calls = decode_calls("It is sunny today.", &tools()).unwrap();
    assert!(calls.is_empty());
}

#[test]
fn unknown_tool_fails_closed() {
    let text = r#"<<call delete_everything {"confirm":true}>>"#;
    let err = decode_calls(text, &tools()).unwrap_err();
    assert_eq!(err, CompactError::UnknownTool("delete_everything".to_string()));
    assert_eq!(err.as_wire(), "unknown_tool");
}

#[test]
fn missing_required_field_fails_closed() {
    let text = r#"<<call create_calendar_event {"title":"No start time"}>>"#;
    let err = decode_calls(text, &tools()).unwrap_err();
    assert_eq!(err.as_wire(), "invalid_arguments");
    assert!(matches!(err, CompactError::InvalidArguments { .. }));
}

#[test]
fn enum_violation_fails_closed() {
    let text = r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
    let err = decode_calls(text, &tools()).unwrap_err();
    assert_eq!(err.as_wire(), "invalid_arguments");
}

#[test]
fn wrong_type_fails_closed() {
    let text = r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":"thirty"}>>"#;
    let err = decode_calls(text, &tools()).unwrap_err();
    assert_eq!(err.as_wire(), "invalid_arguments");
}

#[test]
fn close_marker_inside_string_argument_is_safe() {
    // `>>` and braces appear inside a JSON string value and must not end the marker early.
    let text = r#"<<call create_calendar_event {"title":"ship it >> now {really}","start":"2026-10-05T15:00:00+05:30"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["title"], json!("ship it >> now {really}"));
}

#[test]
fn escaped_quotes_in_argument_are_preserved() {
    let text = r#"<<call create_calendar_event {"title":"say \"hi\"","start":"2026-10-05T15:00:00+05:30"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls[0].arguments["title"], json!("say \"hi\""));
}

#[test]
fn stream_decoder_handles_marker_split_across_chunks() {
    let chunks = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let mut dec = StreamDecoder::new(&tools());
    let mut calls: Vec<ToolCall> = Vec::new();
    for chunk in chunks {
        for res in dec.push(chunk) {
            calls.push(res.unwrap());
        }
    }
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], json!("Retro"));
    assert!(dec.pending().is_empty());
}

#[test]
fn stream_decoder_splits_inside_string_with_markers() {
    // The closing `>>` sequence also appears inside the string, split across chunks.
    let chunks = ["<<call create_calendar_event {\"title\":\"a >", "> b\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>"];
    let mut dec = StreamDecoder::new(&tools());
    let mut calls = Vec::new();
    for chunk in chunks {
        calls.extend(dec.push(chunk).into_iter().map(|r| r.unwrap()));
    }
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["title"], json!("a >> b"));
}

#[test]
fn stream_decoder_reports_unknown_tool_error() {
    let mut dec = StreamDecoder::new(&tools());
    let out = dec.push(r#"<<call nope {"x":1}>>"#);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].as_ref().unwrap_err().as_wire(), "unknown_tool");
}

#[test]
fn decode_tools_round_trips_schema_shape() {
    let compact = encode_tools(&tools()).unwrap();
    let recovered = decode_tools(&compact).unwrap();
    assert_eq!(recovered.len(), 1);
    let params = recovered[0].parameters.as_ref().unwrap();
    let props = &params["properties"];
    assert_eq!(props["title"]["type"], json!("string"));
    assert_eq!(props["start"]["format"], json!("date-time"));
    assert_eq!(props["duration_min"]["type"], json!("integer"));
    assert_eq!(props["attendees"]["type"], json!("array"));
    assert_eq!(props["attendees"]["items"]["type"], json!("string"));
    assert_eq!(props["visibility"]["enum"], json!(["public", "private"]));

    let required: Vec<&str> = params["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(required.contains(&"title"));
    assert!(required.contains(&"start"));
    assert!(!required.contains(&"duration_min"));
    assert_eq!(recovered[0].description.as_deref(), Some("Create an event in the user's calendar."));
}

/// Deterministic property test: for many generated argument objects, rendering a call and
/// decoding it must reproduce the exact arguments. Uses a small LCG so the run stays
/// reproducible and dependency-free.
#[test]
fn property_render_then_decode_is_identity() {
    let mut rng: u64 = 0x9E3779B97F4A7C15;
    let mut next = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        rng >> 33
    };
    let tools = tools();
    for _ in 0..500 {
        let mut args = serde_json::Map::new();
        args.insert("title".to_string(), Value::String(format!("evt-{}", next())));
        // Exercise the escaping and marker-collision paths.
        if next() % 2 == 0 {
            let idx = (next() % 3) as usize;
            let tricky = ["a >> b", "quote \" here", "brace {x} end"][idx];
            args.insert("title".to_string(), Value::String(tricky.to_string()));
        }
        args.insert("start".to_string(), Value::String("2026-10-05T15:00:00+05:30".to_string()));
        if next() % 2 == 0 {
            args.insert("duration_min".to_string(), Value::Number((next() % 120).into()));
        }
        if next() % 2 == 0 {
            args.insert("visibility".to_string(), Value::String(if next() % 2 == 0 { "public" } else { "private" }.to_string()));
        }
        let args_value = Value::Object(args);
        let rendered = format!(
            "<<call create_calendar_event {}>>",
            serde_json::to_string(&args_value).unwrap()
        );
        let calls = decode_calls(&rendered, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, args_value);
    }
}
