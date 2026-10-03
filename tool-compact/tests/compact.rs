use nasiko_tool_compact::{
    CompactTools, Error, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools,
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
        description: Some("Send an email.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string", "format": "email"}},
                "subject": {"type": "string"},
                "body": {"type": "string"}
            },
            "required": ["to", "subject", "body"]
        })),
    }
}

fn tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

fn args(call: &ToolCall) -> Value {
    serde_json::from_str(&call.arguments).unwrap()
}

const CAL_CALL: &str =
    r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#;

// ─── encode ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn encodes_signatures_and_is_smaller() {
    let compact = encode_tools(&tools()).unwrap();
    assert!(compact.prompt.contains(
        "create_calendar_event(title:str, start:datetime, attendees?:[str], duration_min?:int, visibility?:\"public\"|\"private\")"
    ));
    assert!(
        compact
            .prompt
            .contains("send_email(to:[email], subject:str, body:str)")
    );
    assert!(compact.prompt.contains("<<call name {\"arg\":value}>>"));
    let native: usize = tools()
        .iter()
        .map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}}).to_string().len())
        .sum();
    assert!(
        compact.prompt.len() * 10 < native * 7,
        "expected >30% fewer bytes: {} vs {native}",
        compact.prompt.len()
    );
}

#[test]
fn encoding_is_deterministic() {
    assert_eq!(
        encode_tools(&tools()).unwrap(),
        encode_tools(&tools()).unwrap()
    );
}

#[test]
fn keeps_disambiguating_descriptions_drops_restated_ones() {
    let mut t = calendar();
    t.parameters.as_mut().unwrap()["properties"]["title"]["description"] = json!("The title");
    let prompt = encode_tools(&[t]).unwrap().prompt;
    assert!(prompt.contains("start: Start time, ISO 8601"));
    assert!(!prompt.contains("title: The title"));
}

#[test]
fn unsupported_schemas_bypass() {
    for schema in [
        json!({"type":"object","properties":{"a":{"oneOf":[{"type":"string"},{"type":"integer"}]}}}),
        json!({"type":"object","properties":{"a":{"$ref":"#/defs/x"}}}),
        json!({"type":"object","properties":{"a":{"type":"string","pattern":"^a"}}}),
        json!({"type":"object","properties":{"a":{"type":["string","null"]}}}),
        json!({"type":"object","properties":{"a":{"type":"object"}}}),
        json!({"type":"object","properties":{"a":{"type":"integer","minimum":1}}}),
        json!({"type":"object","properties":{"bad-name":{"type":"string"}}}),
    ] {
        let t = ToolDef {
            name: "f".into(),
            description: None,
            parameters: Some(schema.clone()),
        };
        assert!(
            matches!(
                encode_tools(std::slice::from_ref(&t)),
                Err(Error::Bypass(_))
            ),
            "{schema}"
        );
        assert!(
            matches!(StreamDecoder::new(&[t]), Err(Error::Bypass(_))),
            "{schema}"
        );
    }
}

#[test]
fn bypasses_when_not_smaller() {
    let t = ToolDef {
        name: "f".into(),
        description: None,
        parameters: None,
    };
    assert!(matches!(encode_tools(&[t]), Err(Error::Bypass(_))));
}

#[test]
fn decode_tools_preserves_schema_meaning() {
    let original = tools();
    let back = decode_tools(&encode_tools(&original).unwrap()).unwrap();
    assert_eq!(back.len(), 2);
    for (o, b) in original.iter().zip(&back) {
        assert_eq!(o.name, b.name);
        let (os, bs) = (
            o.parameters.as_ref().unwrap(),
            b.parameters.as_ref().unwrap(),
        );
        assert_eq!(sorted(&os["required"]), sorted(&bs["required"]));
        for (k, v) in os["properties"].as_object().unwrap() {
            assert_eq!(shape(v), shape(&bs["properties"][k]), "{k}");
        }
    }
}

/// Type-level view of a property schema (descriptions ignored).
fn shape(v: &Value) -> Value {
    json!({
        "type": v["type"], "format": v["format"], "enum": v["enum"],
        "items": if v["items"].is_null() { Value::Null } else { shape(&v["items"]) },
    })
}

fn sorted(v: &Value) -> Vec<String> {
    let mut r: Vec<String> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect();
    r.sort();
    r
}

#[test]
fn nested_objects_round_trip_through_decode_tools() {
    let t = ToolDef {
        name: "book".into(),
        description: Some(
            "Book a room, with a long description so compaction is smaller than native".into(),
        ),
        parameters: Some(json!({"type":"object","properties":{
            "room":{"type":"object","properties":{"floor":{"type":"integer"},"name":{"type":"string"}},"required":["name"]},
            "tags":{"type":"array","items":{"type":"object","properties":{"k":{"type":"string"}},"required":["k"]}}
        },"required":["room"]})),
    };
    let compact = encode_tools(&[t]).unwrap();
    assert!(
        compact
            .prompt
            .contains("book(room:{name:str, floor?:int}, tags?:[{k:str}])"),
        "{}",
        compact.prompt
    );
    let back = decode_tools(&compact).unwrap();
    assert_eq!(
        back[0].parameters.as_ref().unwrap()["properties"]["room"]["required"],
        json!(["name"])
    );
    assert_eq!(
        back[0].parameters.as_ref().unwrap()["properties"]["tags"]["items"]["properties"]["k"]["type"],
        "string"
    );
}

#[test]
fn compact_tools_with_no_tools_is_bypass() {
    assert!(matches!(encode_tools(&[]), Err(Error::Bypass(_))));
    let _ = CompactTools {
        prompt: String::new(),
    };
}

// ─── decode ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn decodes_a_single_call() {
    let calls = decode_calls(CAL_CALL, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(
        args(&calls[0]),
        json!({"title":"Retro","start":"2026-10-04T10:00:00+05:30"})
    );
}

#[test]
fn arguments_text_is_passed_through_unmodified() {
    let text = r#"<<call send_email { "to": ["a@x.com"], "subject": "Hi", "body": "b" }>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(
        calls[0].arguments,
        r#"{ "to": ["a@x.com"], "subject": "Hi", "body": "b" }"#
    );
}

#[test]
fn decodes_multiple_calls_with_text_around() {
    let text = format!(
        "Sure, doing both.\n<<call send_email {{\"to\":[\"sam@example.com\"],\"subject\":\"Build\",\"body\":\"Green\"}}>>\nand then\n{CAL_CALL}\nDone!"
    );
    let calls = decode_calls(&text, &tools()).unwrap();
    assert_eq!(
        calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["send_email", "create_calendar_event"]
    );
}

#[test]
fn plain_answer_has_no_calls() {
    assert!(
        decode_calls("It's sunny. 1 << 3 and a >> b.", &tools())
            .unwrap()
            .is_empty()
    );
    assert!(decode_calls("", &tools()).unwrap().is_empty());
}

#[test]
fn close_marker_inside_a_string_argument_is_not_the_end() {
    let text =
        r#"<<call send_email {"to":["a@x.com"],"subject":"a >> b","body":"x <<call y {}>> z"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(args(&calls[0])["subject"], "a >> b");
    assert_eq!(args(&calls[0])["body"], "x <<call y {}>> z");
}

fn err(text: &str) -> Error {
    decode_calls(text, &tools()).unwrap_err()
}

#[test]
fn unknown_tool_is_an_error() {
    assert_eq!(
        err(r#"<<call send_sms {"to":"1"}>>"#),
        Error::UnknownTool("send_sms".into())
    );
    assert_eq!(
        err(r#"<<call send_sms {"to":"1"}>>"#).code(),
        "unknown_tool"
    );
}

#[test]
fn invalid_arguments_are_errors() {
    for text in [
        r#"<<call create_calendar_event {"start":"2026-10-04T10:00:00+05:30"}>>"#, // missing title
        r#"<<call create_calendar_event {"title":"x","start":"s","visibility":"secret"}>>"#, // bad enum
        r#"<<call create_calendar_event {"title":"x","start":"s","duration_min":"30"}>>"#, // wrong type
        r#"<<call create_calendar_event {"title":"x","start":"s","duration_min":30.5}>>"#, // not an int
        r#"<<call create_calendar_event {"title":"x","start":"s","extra":1}>>"#, // unknown key
        r#"<<call create_calendar_event {"title":null,"start":"s"}>>"#,          // null
        r#"<<call send_email {"to":"a@x.com","subject":"s","body":"b"}>>"#,      // string, not list
        r#"<<call send_email {"to":[1],"subject":"s","body":"b"}>>"#,            // bad item type
    ] {
        let e = err(text);
        assert!(
            matches!(e, Error::InvalidArguments { .. }),
            "{text} -> {e:?}"
        );
        assert_eq!(e.code(), "invalid_arguments");
    }
}

#[test]
fn malformed_markers_are_errors() {
    for text in [
        r#"<<call create_calendar_event {"title":"x",}>>"#, // bad JSON
        r#"<<call create_calendar_event [1]>>"#,            // not an object
        r#"<<call create_calendar_event {"title":"x","start":"s"}"#, // no >>
        r#"<<call create_calendar_event {"title":"x","start":"s"}> >"#,
        r#"<<call create_calendar_event {"title":"x""#, // truncated
        "<<call create_calendar_event",                 // truncated name
        "<<call ",                                      // nothing
        "<<call {}>>",                                  // no name
    ] {
        let e = err(text);
        assert_eq!(e.code(), "invalid_arguments", "{text} -> {e:?}");
    }
}

#[test]
fn one_bad_call_fails_the_whole_reply() {
    let text = format!(r#"{CAL_CALL} <<call send_sms {{}}>>"#);
    assert!(decode_calls(&text, &tools()).is_err());
}

// ─── streaming ──────────────────────────────────────────────────────────────────────────────

fn stream(chunks: &[&str]) -> Result<Vec<ToolCall>, Error> {
    let mut d = StreamDecoder::new(&tools())?;
    let mut out = vec![];
    for c in chunks {
        out.extend(d.push(c)?);
    }
    out.extend(d.finish()?);
    Ok(out)
}

#[test]
fn marker_split_across_chunks() {
    let calls = stream(&[
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ])
    .unwrap();
    assert_eq!(calls, decode_calls(CAL_CALL, &tools()).unwrap());
}

#[test]
fn call_is_emitted_as_soon_as_it_closes() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert!(
        d.push("hello <<call create_calendar_event {\"title\":\"a\",\"start\":\"b\"}")
            .unwrap()
            .is_empty()
    );
    assert_eq!(d.push(">>  trailing <<ca").unwrap().len(), 1);
    assert!(d.finish().unwrap().is_empty()); // dangling `<<ca` is just text
}

#[test]
fn unknown_tool_fails_fast_and_stays_failed() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert_eq!(
        d.push("<<call nope {").unwrap_err(),
        Error::UnknownTool("nope".into())
    );
    assert!(d.push("}>>").is_err());
}

#[test]
fn unterminated_call_errors_at_finish() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert!(
        d.push("<<call create_calendar_event {\"title\":")
            .unwrap()
            .is_empty()
    );
    assert!(d.finish().is_err());
}

/// Splitting the text anywhere (one cut, then every pair of cuts) must not change the result.
#[test]
fn any_chunking_gives_the_same_result() {
    let texts = [
        format!(
            "Plan:\n{CAL_CALL}\nthen <<call send_email {{\"to\":[\"a@x.com\"],\"subject\":\"a >> b\",\"body\":\"é<<call\"}}>> ok << >> <<"
        ),
        "no calls here, just << and >> and <<cal".to_string(),
        format!("{CAL_CALL}{CAL_CALL}"),
        r#"<<call create_calendar_event {"title":"x","start":"s","visibility":"secret"}>>"#
            .to_string(),
        r#"<<call create_calendar_event {"title":"x""#.to_string(),
    ];
    for text in &texts {
        let whole = decode_calls(text, &tools());
        let cuts: Vec<usize> = (0..=text.len())
            .filter(|&i| text.is_char_boundary(i))
            .collect();
        for &i in &cuts {
            assert_eq!(
                stream(&[&text[..i], &text[i..]]),
                whole,
                "cut at {i} of {text}"
            );
            for &j in cuts.iter().filter(|&&j| j >= i) {
                assert_eq!(
                    stream(&[&text[..i], &text[i..j], &text[j..]]),
                    whole,
                    "cuts {i},{j} of {text}"
                );
            }
        }
        // And one byte-ish char at a time.
        let singles: Vec<String> = text.chars().map(String::from).collect();
        let singles: Vec<&str> = singles.iter().map(String::as_str).collect();
        assert_eq!(stream(&singles), whole, "char-by-char {text}");
    }
}
