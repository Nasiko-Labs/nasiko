//! End-to-end behaviour through the public API: the published decoder cases, chunking
//! independence, and encode → decode round trips.

use nasiko_tool_compact::{
    Error, Event, StreamDecoder, ToolCall, ToolDef, decode, decode_calls, decode_tools,
    encode_tools, render_calls,
};
use serde_json::{Value, json};

fn tools() -> Vec<ToolDef> {
    serde_json::from_value(json!([
        {
            "name": "create_calendar_event",
            "description": "Create an event in the user's calendar.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            }
        },
        {
            "name": "send_email",
            "description": "Send an email from the user's account.",
            "parameters": {
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                    "subject": {"type": "string", "description": "Subject line"},
                    "body": {"type": "string", "description": "Plain-text body"},
                    "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
                },
                "required": ["to", "subject", "body"]
            }
        }
    ]))
    .unwrap()
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        name: name.into(),
        arguments: args.as_object().unwrap().clone(),
    }
}

/// Feed chunks one by one, the way the eval does.
fn stream(chunks: &[&str]) -> Result<Vec<ToolCall>, Error> {
    let mut d = StreamDecoder::new(&tools())?;
    let mut events = Vec::new();
    for c in chunks {
        events.extend(d.push(c)?);
    }
    events.extend(d.finish()?);
    Ok(events
        .into_iter()
        .filter_map(|e| match e {
            Event::Call(c) => Some(c),
            Event::Text(_) => None,
        })
        .collect())
}

#[test]
fn published_decoder_cases() {
    // dc-001 valid single call
    assert_eq!(
        stream(&["<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>"]).unwrap(),
        vec![call("create_calendar_event", json!({"title": "Design review", "start": "2026-10-05T15:00:00+05:30"}))]
    );
    // dc-002 marker split across stream chunks
    assert_eq!(
        stream(&[
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">"
        ])
        .unwrap(),
        vec![call(
            "create_calendar_event",
            json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30"})
        )]
    );
    // dc-003 '>>' inside a string argument
    assert_eq!(
        stream(&["<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>"]).unwrap(),
        vec![call("send_email", json!({"to": ["sam@example.com"], "subject": "a >> b", "body": "x"}))]
    );
    // dc-004 unknown tool
    assert_eq!(
        stream(&["<<call delete_everything {}>>"])
            .unwrap_err()
            .code(),
        "unknown_tool"
    );
    // dc-005 missing required field + bad enum
    assert_eq!(
        stream(&["<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>"])
            .unwrap_err()
            .code(),
        "invalid_arguments"
    );
}

/// Outputs that must decode identically however they are chunked.
fn samples() -> Vec<&'static str> {
    vec![
        "Sure. <<call send_email {\"to\":[\"a@b.co\"],\"subject\":\"x >> y\",\"body\":\"<<call nope {}>>\"}>> Done.",
        "<<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-04T10:00:00Z\"}>>\n<<call send_email {\"to\":[],\"subject\":\"s\",\"body\":\"b\\\"}\"}>>",
        "Use x << 2 and <<calls or <<call>; no tool needed. <",
        "It's sunny today.",
        "<<cal",
        "",
    ]
}

#[test]
fn chunking_never_changes_the_result() {
    for text in samples() {
        let whole = decode(text, &tools()).map(|d| (d.text, d.calls));
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        // every single split point
        for &(i, _) in &chars {
            let got = stream(&[&text[..i], &text[i..]]);
            assert_eq!(
                got.map_err(|e| e.code()),
                whole.clone().map(|w| w.1).map_err(|e| e.code()),
                "split at {i} of {text:?}"
            );
        }
        // one character per chunk, also checking the released text
        let mut d = StreamDecoder::new(&tools()).unwrap();
        let mut out_text = String::new();
        let mut calls = Vec::new();
        let mut err = None;
        for (i, c) in &chars {
            match d.push(&text[*i..*i + c.len_utf8()]) {
                Ok(evs) => collect(evs, &mut out_text, &mut calls),
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        let result = match err {
            Some(e) => Err(e),
            None => d.finish().map(|evs| {
                collect(evs, &mut out_text, &mut calls);
                (out_text, calls)
            }),
        };
        assert_eq!(
            result.map_err(|e| e.code()),
            whole.map_err(|e| e.code()),
            "char-by-char {text:?}"
        );
    }
}

fn collect(events: Vec<Event>, text: &mut String, calls: &mut Vec<ToolCall>) {
    for e in events {
        match e {
            Event::Text(t) => text.push_str(&t),
            Event::Call(c) => calls.push(c),
        }
    }
}

#[test]
fn text_around_calls_is_preserved() {
    let d = decode(samples()[0], &tools()).unwrap();
    assert_eq!(d.text, "Sure.  Done.");
    assert_eq!(d.calls.len(), 1);
    assert_eq!(d.calls[0].arguments["body"], "<<call nope {}>>");

    let plain = decode(samples()[2], &tools()).unwrap();
    assert_eq!(plain.text, samples()[2]);
    assert!(plain.calls.is_empty());
    assert_eq!(decode("<<cal", &tools()).unwrap().text, "<<cal");
}

#[test]
fn multiple_calls_keep_order() {
    let calls = decode_calls(samples()[1], &tools()).unwrap();
    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["create_calendar_event", "send_email"]);
    assert_eq!(calls[1].arguments["body"], "b\"}");
}

#[test]
fn fails_closed() {
    let cases = [
        (
            "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\"",
            "malformed_call",
        ),
        (
            "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\",\"body\":\"b\"}",
            "malformed_call",
        ),
        (
            "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\",\"body\":\"b\"} trailing",
            "malformed_call",
        ),
        (
            "<<call send_email {\"to\":\"a\",\"subject\":\"s\",\"body\":\"b\"}>>",
            "invalid_arguments",
        ),
        (
            "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\",\"subject\":\"t\",\"body\":\"b\"}>>",
            "invalid_arguments",
        ),
        (
            "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\",\"body\":\"b\",\"bcc\":[]}>>",
            "invalid_arguments",
        ),
        ("<<call send_email {to: [\"a\"]}>>", "invalid_arguments"),
        (
            "ok <<call create_calendar_event {\"title\":\"A\",\"start\":\"2026-10-04T10:00:00Z\"}>> <<call ghost {}>>",
            "unknown_tool",
        ),
        // The name is known to be wrong before the call breaks off.
        ("<<call ghost {\"a\":", "unknown_tool"),
        ("<<call ghost oops", "unknown_tool"),
        ("<<call  {}>>", "malformed_call"),
    ];
    for (text, code) in cases {
        assert_eq!(decode(text, &tools()).unwrap_err().code(), code, "{text}");
    }
}

#[test]
fn rendered_calls_round_trip() {
    let calls = vec![
        call(
            "send_email",
            json!({"to": ["sam@example.com"], "subject": "a >> b", "body": "line\n\"quoted\" }>>"}),
        ),
        call(
            "create_calendar_event",
            json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30", "duration_min": 30, "visibility": "private"}),
        ),
    ];
    assert_eq!(
        decode_calls(&render_calls(&calls), &tools()).unwrap(),
        calls
    );
}

#[test]
fn definitions_round_trip_through_decode_tools() {
    let compact = encode_tools(&tools()).unwrap();
    let back = decode_tools(&compact).unwrap();
    assert_eq!(back.len(), 2);
    for (orig, got) in tools().iter().zip(&back) {
        assert_eq!(orig.name, got.name);
        assert_eq!(orig.description, got.description);
        let (o, g) = (
            orig.parameters.as_ref().unwrap(),
            got.parameters.as_ref().unwrap(),
        );
        let sorted = |v: &Value| {
            let mut r: Vec<String> = serde_json::from_value(v.clone()).unwrap();
            r.sort();
            r
        };
        assert_eq!(sorted(&o["required"]), sorted(&g["required"]));
        for (field, schema) in o["properties"].as_object().unwrap() {
            let mut want = schema.clone();
            let mut have = g["properties"][field].clone();
            // Descriptions that only repeat the field name are dropped by design.
            if have.get("description").is_none() {
                want.as_object_mut().unwrap().remove("description");
            }
            want.as_object_mut().unwrap().remove("description");
            have.as_object_mut().unwrap().remove("description");
            assert_eq!(want, have, "{}.{field}", orig.name);
        }
    }
    // Re-encoding the decoded tools is a fixed point.
    assert_eq!(encode_tools(&back).unwrap(), compact);
}

/// A sanity floor only. Real savings are measured in tokens by the eval example.
#[test]
fn compact_prompt_is_smaller_than_native_tools() {
    let compact = encode_tools(&tools()).unwrap();
    let native: Vec<Value> = tools()
        .into_iter()
        .map(|t| json!({"type": "function", "function": t}))
        .collect();
    let native = serde_json::to_string(&native).unwrap();
    assert!(
        compact.prompt().len() < native.len(),
        "{}\nvs {} bytes",
        compact.prompt(),
        native.len()
    );
}

#[test]
fn unsupported_schemas_are_reported_not_approximated() {
    let mut t = tools();
    t[0].parameters.as_mut().unwrap()["properties"]["duration_min"]["minimum"] = json!(1);
    let err = encode_tools(&t).unwrap_err();
    assert_eq!(err.code(), "unsupported_schema");
    assert!(err.to_string().contains("create_calendar_event"));
    assert!(StreamDecoder::new(&t).is_err());
}

/// Awkward schemas: whatever the encoding loses must not change what the decoder accepts.
#[test]
fn tricky_schemas_survive_the_round_trip() {
    let schemas = [
        json!({"type": "object", "properties": {
            "items": {"type": "array", "description": "Line items (one per SKU)", "items": {
                "type": "object",
                "properties": {"sku": {"type": "string"}, "qty": {"type": "integer", "description": "How many"}},
                "required": ["sku"]
            }},
            "note": {"type": ["string", "null"], "description": "Shown :) to \"users\""},
            "tags": {"type": "array", "items": {"enum": ["a", "in progress", "null", "2fa", 3]}},
            "meta": {"type": "object"},
            "empty": {"type": "object", "properties": {}, "additionalProperties": false},
            "extra": {"type": "object", "properties": {"k": {"type": "boolean"}}, "additionalProperties": true},
            "grid": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}},
            "maybe_rows": {"type": "array", "items": {"type": ["object", "null"], "properties": {"x": {"type": "integer"}}}},
            "when": {"type": "string", "format": "date", "description": "Día — 日付, (ISO)"},
            "anything": {"description": "Free-form"},
            "link": {"type": "string", "format": "uri"}
        }, "required": ["items", "when"]}),
        json!({"type": "object", "properties": {}}),
        json!({"type": "object"}),
    ];
    let probes = [
        json!({"items": [{"sku": "A", "qty": 2}], "when": "2026-10-04"}),
        json!({"items": [], "when": "2026-10-04", "note": null, "tags": ["in progress", 3, "null"]}),
        json!({"items": [{"qty": 2}], "when": "2026-10-04"}),
        json!({"items": [], "when": "04-10-2026"}),
        json!({"items": [], "when": "2026-10-04", "tags": ["b"]}),
        json!({"items": [], "when": "2026-10-04", "extra": {"k": true, "other": 1}, "meta": {"z": []}}),
        json!({"items": [], "when": "2026-10-04", "empty": {"x": 1}}),
        json!({"items": [], "when": "2026-10-04", "grid": [[1, 2.5], []], "maybe_rows": [null, {"x": 1}]}),
        json!({"items": [], "when": "2026-10-04", "maybe_rows": [{"x": "1"}]}),
        json!({"items": [], "when": "2026-10-04", "anything": {"a": [1]}, "link": "https://x.dev"}),
        json!({"items": [], "when": "2026-10-04", "link": "not a uri"}),
        json!({}),
        json!({"unexpected": 1}),
    ];
    for (n, schema) in schemas.iter().enumerate() {
        let original = vec![ToolDef { name: format!("tool_{n}"), description: Some("Do (it).".into()), parameters: Some(schema.clone()) }];
        let compact = encode_tools(&original).unwrap();
        let normalized = decode_tools(&compact).unwrap();
        assert_eq!(encode_tools(&normalized).unwrap(), compact, "not a fixed point:\n{}", compact.definitions);
        assert_eq!(decode_tools(&encode_tools(&normalized).unwrap()).unwrap(), normalized);
        let mut accepted = 0;
        for args in &probes {
            let text = format!("<<call tool_{n} {args}>>");
            let want = decode_calls(&text, &original).map_err(|e| e.code());
            assert_eq!(
                want,
                decode_calls(&text, &normalized).map_err(|e| e.code()),
                "schema {n} disagrees on {args}\n{}",
                compact.definitions
            );
            accepted += usize::from(want.is_ok());
        }
        // The probes must exercise both outcomes where the schema allows it, or the comparison
        // proves nothing. Schema 1 takes no arguments; schema 2 takes any object.
        match n {
            0 => assert!(accepted > 1 && accepted < probes.len() - 1, "{accepted} accepted"),
            1 => assert_eq!(accepted, 1),
            _ => assert_eq!(accepted, probes.len()),
        }
        if n == 0 {
            println!("{}", compact.definitions);
        }
    }
}
