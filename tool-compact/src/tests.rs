//! Unit tests over the public API, with the eval set's tools as fixtures.

use serde_json::{Value, json};

use crate::*;

fn calendar() -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {
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
        }
    }))
    .unwrap()
}

fn email() -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {
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
    }))
    .unwrap()
}

fn tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

fn args(call: &ToolCall) -> Value {
    call.arguments_value().unwrap()
}

/// Feed `chunks` through a fresh stream decoder; return calls and text, or the first error.
fn stream(chunks: &[&str], tools: &[ToolDef]) -> Result<Decoded> {
    let mut d = StreamDecoder::new(tools)?;
    let mut events = Vec::new();
    for c in chunks {
        events.extend(d.push(c)?);
    }
    events.extend(d.finish()?);
    let mut out = Decoded::default();
    for e in events {
        match e {
            StreamEvent::Text(t) => out.text.push_str(&t),
            StreamEvent::Call { call, .. } => out.calls.push(call),
        }
    }
    Ok(out)
}

// ── encoding ────────────────────────────────────────────────────────────────

#[test]
fn renders_the_documented_format() {
    let c = encode_tools(&[calendar()]).unwrap();
    assert_eq!(
        c.definitions,
        "create_calendar_event: Create an event in the user's calendar.\n\
         \x20title: str\n\
         \x20start: str(date-time) # Start time, ISO 8601\n\
         \x20attendees?: str[] # Attendee emails\n\
         \x20duration_min?: int # Duration in minutes\n\
         \x20visibility?: 'public'|'private'"
    );
    assert!(c.system_prompt().starts_with(INSTRUCTIONS));
}

#[test]
fn definitions_contain_no_double_quotes() {
    // A `"` costs a `\"` escape once the prompt sits inside a JSON request body.
    let c = encode_tools(&tools()).unwrap();
    assert!(!c.definitions.contains('"'), "{}", c.definitions);
}

#[test]
fn encoding_is_deterministic() {
    assert_eq!(
        encode_tools(&tools()).unwrap(),
        encode_tools(&tools()).unwrap()
    );
}

#[test]
fn nested_objects_arrays_and_annotations_render_and_parse_back() {
    let tool = ToolDef::function(
        "place_order",
        Some("Place an order."),
        Some(json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "description": "Line items",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sku": {"type": "string", "pattern": "^[A-Z]{3}-\\d+$"},
                            "qty": {"type": "integer", "minimum": 1, "maximum": 99, "default": 1},
                            "tags": {"type": "array", "items": {"type": "string", "enum": ["gift", "fragile"]}}
                        },
                        "required": ["sku"]
                    }
                },
                "address": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "zip": {"type": ["string", "null"], "maxLength": 10},
                        "country": {"type": "string", "enum": ["IN", "US", "it's"]}
                    },
                    "required": ["zip"]
                },
                "note": {"description": "Anything"},
                "express": {"type": "boolean", "default": false},
                "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}}
            },
            "required": ["items", "address"]
        })),
    );
    let c = encode_tools(std::slice::from_ref(&tool)).unwrap();
    assert_eq!(
        c.definitions,
        "place_order(closed): Place an order.\n\
         \x20items: obj[](minItems=1) { # Line items\n\
         \x20 sku: str(pattern='^[A-Z]{3}-\\\\d+$')\n\
         \x20 qty?: int(min=1,max=99,default=1)\n\
         \x20 tags?: ('gift'|'fragile')[]\n\
         \x20}\n\
         \x20address: obj(closed) {\n\
         \x20 zip: str|null(maxLen=10)\n\
         \x20 country?: 'IN'|'US'|'it\\'s'\n\
         \x20}\n\
         \x20express?: bool(default=false)\n\
         \x20matrix?: num[][]\n\
         \x20note?: any # Anything"
    );
    let back = decode_tools(&c).unwrap();
    assert_eq!(back, vec![tool]);
}

#[test]
fn decode_tools_reproduces_the_eval_tools() {
    let c = encode_tools(&tools()).unwrap();
    // Everything survives except the two descriptions that only restate their names.
    let mut expected = tools();
    let props = |t: &mut ToolDef, f: &str| -> Value {
        t.function.parameters.as_mut().unwrap()["properties"][f]
            .as_object_mut()
            .unwrap()
            .remove("description")
            .unwrap()
    };
    assert_eq!(props(&mut expected[0], "title"), "Event title");
    assert_eq!(props(&mut expected[1], "cc"), "CC emails");
    assert_eq!(decode_tools(&c).unwrap(), expected);
}

#[test]
fn only_descriptions_that_restate_names_are_dropped() {
    use crate::encode::is_redundant_description as redundant;
    assert!(redundant("Event title", "title", "create_calendar_event"));
    assert!(redundant("CC emails", "cc", "send_email"));
    assert!(redundant("The user ID", "userId", "get_user"));
    assert!(redundant("Cities", "city", "weather"));
    // One new content word keeps the description.
    assert!(!redundant(
        "Attendee emails",
        "attendees",
        "create_calendar_event"
    ));
    assert!(!redundant(
        "Duration in minutes",
        "duration_min",
        "create_calendar_event"
    ));
    assert!(!redundant("Recipient emails", "to", "send_email"));
    assert!(!redundant("Subject line", "subject", "send_email"));
    // Stopwords alone carry nothing to compare, so nothing is dropped.
    assert!(!redundant("the", "the", "t"));
}

#[test]
fn unsupported_features_bypass_with_a_reason() {
    for (schema, feature) in [
        (
            json!({"type":"object","properties":{"a":{"anyOf":[{"type":"string"},{"type":"integer"}]}}}),
            "anyOf",
        ),
        (
            json!({"type":"object","properties":{"a":{"$ref":"#/defs/x"}}}),
            "$ref",
        ),
        (
            json!({"type":"object","properties":{"a":{"const":"x"}}}),
            "const",
        ),
        (
            json!({"type":"object","additionalProperties":true,"properties":{}}),
            "additionalProperties",
        ),
        (
            json!({"type":"object","properties":{"a":{"type":"string","pattern":"(?=x)"}}}),
            "pattern",
        ),
        (
            json!({"type":"object","properties":{"a":{"type":"number","enum":[1.5,2.5]}}}),
            "enum",
        ),
        (
            json!({"type":"object","properties":{"a":{"type":"array","items":{"type":"string","description":"x"}}}}),
            "items",
        ),
        (
            json!({"type":"object","properties":{"a b":{"type":"string"}}}),
            "quoting",
        ),
        (
            json!({"type":"object","properties":{"a":{"type":"string","default":["x"]}}}),
            "default",
        ),
        (
            json!({"type":"object","properties":{},"required":["ghost"]}),
            "ghost",
        ),
    ] {
        let tool = ToolDef::function("t", None, Some(schema.clone()));
        let e = encode_tools(&[tool]).unwrap_err();
        assert_eq!(e.code(), "unsupported_schema", "{schema}");
        assert!(
            e.to_string().contains(feature),
            "{e} should mention {feature}"
        );
    }
}

#[test]
fn invalid_tools_are_refused() {
    let bad_name = ToolDef::function("has space", None, None);
    assert_eq!(
        encode_tools(&[bad_name]).unwrap_err().code(),
        "invalid_tool"
    );
    let dup = vec![email(), email()];
    assert_eq!(encode_tools(&dup).unwrap_err().code(), "invalid_tool");
    let not_object = ToolDef::function("t", None, Some(json!({"type": "string"})));
    assert_eq!(
        encode_tools(&[not_object]).unwrap_err().code(),
        "invalid_tool"
    );
    let mut extra = email();
    extra.extra.insert("strict".into(), json!(true));
    assert_eq!(
        encode_tools(&[extra]).unwrap_err().code(),
        "unsupported_schema"
    );
}

#[test]
fn excessive_nesting_bypasses() {
    let mut schema = json!({"type": "string"});
    for _ in 0..20 {
        schema = json!({"type": "object", "properties": {"x": schema}});
    }
    let e = encode_tools(&[ToolDef::function("deep", None, Some(schema))]).unwrap_err();
    assert_eq!(e.code(), "unsupported_schema");
}

#[test]
fn descriptions_are_whitespace_collapsed_not_removed() {
    let tool = ToolDef::function(
        "t",
        Some("Line one.\n\n  Line   two."),
        Some(json!({"type":"object","properties":{"a":{"type":"string","description":"x\ny"}}})),
    );
    let c = encode_tools(&[tool]).unwrap();
    assert_eq!(c.definitions, "t: Line one. Line two.\n a?: str # x y");
}

#[test]
fn tool_without_parameters_takes_no_arguments() {
    let ping = ToolDef::function("ping", Some("Health check"), None);
    let c = encode_tools(std::slice::from_ref(&ping)).unwrap();
    assert_eq!(c.definitions, "ping: Health check");
    let calls = decode_calls("<<call ping>>", std::slice::from_ref(&ping)).unwrap();
    assert_eq!(calls[0].arguments, "{}");
    let e = decode_calls("<<call ping {\"x\":1}>>", &[ping]).unwrap_err();
    assert_eq!(e.code(), "invalid_arguments");
}

// ── decoding ────────────────────────────────────────────────────────────────

#[test]
fn decodes_a_single_call() {
    let calls = decode_calls(
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(args(&calls[0])["title"], "Design review");
}

#[test]
fn decodes_multiple_calls_with_surrounding_text() {
    let out = decode(
        "Sure — doing both.\n<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build\",\"body\":\"Green\"}>>\n\
         <<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"duration_min\":30}>>\nDone.",
        &tools(),
    )
    .unwrap();
    let names: Vec<&str> = out.calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["send_email", "create_calendar_event"]);
    assert_eq!(out.text, "Sure — doing both.\n\n\nDone.");
}

#[test]
fn a_plain_answer_has_no_calls() {
    let out = decode("I can't check the weather, sorry.", &tools()).unwrap();
    assert!(out.calls.is_empty());
    assert_eq!(out.text, "I can't check the weather, sorry.");
}

#[test]
fn arguments_are_passed_through_byte_for_byte() {
    let raw = r#"{ "to": ["a@b.c"], "subject": "x",  "body": "café \"quoted\"" }"#;
    let calls = decode_calls(&format!("<<call send_email {raw}>>"), &tools()).unwrap();
    assert_eq!(calls[0].arguments, raw);
}

#[test]
fn close_marker_inside_a_string_does_not_end_the_call() {
    let calls = decode_calls(
        r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b } {","body":"x \" >>"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(args(&calls[0])["subject"], "a >> b } {");
    assert_eq!(args(&calls[0])["body"], "x \" >>");
}

#[test]
fn whitespace_and_newlines_inside_a_call_are_allowed() {
    let calls = decode_calls(
        "<<call   send_email\n{\n  \"to\": [\"a@b.c\"],\n  \"subject\": \"s\",\n  \"body\": \"b\"\n}\n>>",
        &tools(),
    )
    .unwrap();
    assert_eq!(calls[0].name, "send_email");
}

#[test]
fn marker_lookalikes_are_text() {
    let out = decode("use <<callback>> and << call and <<", &tools()).unwrap();
    assert!(out.calls.is_empty());
    assert_eq!(out.text, "use <<callback>> and << call and <<");
}

#[test]
fn error_codes_match_the_eval_contract() {
    let cases = [
        ("<<call delete_everything {}>>", "unknown_tool"),
        (
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
            "invalid_arguments",
        ),
        (
            r#"<<call create_calendar_event {"title":"x"}>>"#,
            "invalid_arguments",
        ),
        (
            r#"<<call create_calendar_event {"title":"x","start":"s","bogus":1}>>"#,
            "invalid_arguments",
        ),
        (
            r#"<<call create_calendar_event {"title":"x","start":"s","duration_min":"30"}>>"#,
            "invalid_arguments",
        ),
        (
            r#"<<call create_calendar_event {"title":"x","start":"s","duration_min":1.5}>>"#,
            "invalid_arguments",
        ),
        (
            r#"<<call create_calendar_event {"title":"x","start":"s","attendees":"a@b.c"}>>"#,
            "invalid_arguments",
        ),
        (
            r#"<<call create_calendar_event {"title":null,"start":"s"}>>"#,
            "invalid_arguments",
        ),
        (
            r#"<<call create_calendar_event {"title":"x","start":"s"}"#,
            "malformed_call",
        ),
        (
            r#"<<call create_calendar_event {"title":"x",}>>"#,
            "malformed_call",
        ),
        (
            r#"<<call create_calendar_event {"title":"x","title":"y","start":"s"}>>"#,
            "malformed_call",
        ),
        (
            r#"<<call create_calendar_event("title")>>"#,
            "malformed_call",
        ),
        (r#"<<call {"title":"x"}>>"#, "malformed_call"),
        (
            r#"<<call create_calendar_event {"title":"x","start":"s"} trailing>>"#,
            "malformed_call",
        ),
    ];
    for (text, code) in cases {
        let e = decode_calls(text, &tools()).expect_err(text);
        assert_eq!(e.code(), code, "{text}: {e}");
    }
}

#[test]
fn a_call_written_without_the_call_keyword_is_an_error_not_text() {
    // Seen live from a small model. Returning "no calls" here would silently drop the call.
    for text in [
        "<<create_calendar_event {\"title\":\"x\",\"start\":\"s\"}>>",
        "<<send_email>>\n{\"to\":[],\"subject\":\"a\",\"body\":\"b\"}",
        "Sure. <<send_email",
    ] {
        let e = decode_calls(text, &tools()).expect_err(text);
        assert_eq!(e.code(), "malformed_call", "{text}: {e}");
    }
    // Split across chunks, the name is still recognised.
    let e = stream(&["ok <<send", "_em", "ail {}>>"], &tools()).unwrap_err();
    assert_eq!(e.code(), "malformed_call");
    // `<<` before anything that is not an offered tool stays text.
    let out = decode("a <<send_emails>> b <<other {}>> c <<", &tools()).unwrap();
    assert!(out.calls.is_empty());
    assert_eq!(out.text, "a <<send_emails>> b <<other {}>> c <<");
}

#[test]
fn one_bad_call_fails_the_whole_response() {
    // Returning the good call alone would silently drop half of what the model asked for.
    let text = "<<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\"}>> <<call nope {}>>";
    assert_eq!(
        decode_calls(text, &tools()).unwrap_err().code(),
        "unknown_tool"
    );
}

#[test]
fn integral_floats_are_integers_but_are_not_rewritten() {
    let raw = r#"{"title":"x","start":"s","duration_min":30.0}"#;
    let calls = decode_calls(&format!("<<call create_calendar_event {raw}>>"), &tools()).unwrap();
    assert_eq!(calls[0].arguments, raw);
}

#[test]
fn annotation_limits_are_enforced() {
    let tool = ToolDef::function(
        "t",
        None,
        Some(json!({"type":"object","properties":{
            "n": {"type":"integer","minimum":1,"maximum":3},
            "s": {"type":"string","minLength":2,"maxLength":3,"pattern":"^a"},
            "l": {"type":"array","items":{"type":"integer"},"minItems":1,"maxItems":2}
        }})),
    );
    let ok = |a: &str| decode_calls(&format!("<<call t {a}>>"), std::slice::from_ref(&tool));
    assert!(ok(r#"{"n":2,"s":"ab","l":[1]}"#).is_ok());
    for bad in [
        r#"{"n":0}"#,
        r#"{"n":4}"#,
        r#"{"s":"a"}"#,
        r#"{"s":"abcd"}"#,
        r#"{"s":"bb"}"#,
        r#"{"l":[]}"#,
        r#"{"l":[1,2,3]}"#,
        r#"{"l":["x"]}"#,
    ] {
        assert_eq!(ok(bad).unwrap_err().code(), "invalid_arguments", "{bad}");
    }
}

// ── streaming ───────────────────────────────────────────────────────────────

#[test]
fn eval_split_marker_case() {
    let out = stream(
        &[
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ],
        &tools(),
    )
    .unwrap();
    assert_eq!(out.calls.len(), 1);
    assert_eq!(args(&out.calls[0])["title"], "Retro");
}

#[test]
fn every_split_point_gives_the_one_shot_result() {
    let text = "Okay. <<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"x >> \\\"y\\\" }\",\"body\":\"é\"}>> then <<callback <<call create_calendar_event {\"title\":\"T\",\"start\":\"s\"}>> end <";
    let whole = decode(text, &tools()).unwrap();
    assert_eq!(whole.calls.len(), 2);
    let boundaries: Vec<usize> = (0..=text.len())
        .filter(|i| text.is_char_boundary(*i))
        .collect();
    for &i in &boundaries {
        for &j in boundaries.iter().filter(|j| **j >= i) {
            let got = stream(&[&text[..i], &text[i..j], &text[j..]], &tools()).unwrap();
            assert_eq!(got, whole, "split at {i},{j}");
        }
    }
}

#[test]
fn text_is_streamed_without_waiting_for_the_end() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert_eq!(
        d.push("Hello wor").unwrap(),
        vec![StreamEvent::Text("Hello wor".into())]
    );
    // A possible marker start is held back, not emitted.
    assert_eq!(
        d.push("ld <<ca").unwrap(),
        vec![StreamEvent::Text("ld ".into())]
    );
    // ...and released as text once it cannot be a marker.
    assert_eq!(
        d.push("t").unwrap(),
        vec![StreamEvent::Text("<<cat".into())]
    );
    assert_eq!(d.finish().unwrap(), vec![]);
}

#[test]
fn calls_are_indexed_in_order() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    let mut idx = Vec::new();
    for chunk in [
        "<<call send_email {\"to\":[],\"subject\":\"a\",\"body\":\"b\"}>>",
        "<<call send_email {\"to\":[],\"subject\":\"c\",\"body\":\"d\"}>>",
    ] {
        for e in d.push(chunk).unwrap() {
            if let StreamEvent::Call { index, .. } = e {
                idx.push(index);
            }
        }
    }
    assert_eq!(idx, [0, 1]);
}

#[test]
fn an_error_poisons_the_decoder() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert!(d.push("<<call nope {}>>").is_err());
    let again = d.push("<<call send_email {\"to\":[],\"subject\":\"a\",\"body\":\"b\"}>>");
    assert_eq!(again.unwrap_err().code(), "unknown_tool");
    assert_eq!(d.finish().unwrap_err().code(), "unknown_tool");
}

#[test]
fn unterminated_call_at_end_of_stream_is_an_error() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    d.push("<<call send_email {\"to\":[").unwrap();
    assert_eq!(d.finish().unwrap_err().code(), "malformed_call");
}

#[test]
fn an_oversized_call_is_cut_off() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    d.push("<<call send_email {\"body\":\"").unwrap();
    let big = "x".repeat(MAX_CALL_BYTES + 1);
    assert_eq!(d.push(&big).unwrap_err().code(), "malformed_call");
}

// ── rendering ───────────────────────────────────────────────────────────────

#[test]
fn rendered_calls_round_trip() {
    let calls = vec![
        ToolCall {
            name: "send_email".into(),
            arguments: r#"{"to":["sam@example.com"],"subject":"a >> b","body":"line\nbreak"}"#
                .into(),
        },
        ToolCall {
            name: "create_calendar_event".into(),
            arguments: r#"{"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30}"#
                .into(),
        },
    ];
    let text = render_calls(&calls).unwrap();
    let back = decode_calls(&text, &tools()).unwrap();
    assert_eq!(back.len(), 2);
    for (a, b) in calls.iter().zip(&back) {
        assert_eq!(a.name, b.name);
        assert_eq!(args(a), args(b));
    }
}
