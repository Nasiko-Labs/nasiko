use super::*;
use serde_json::{Map, Value, json};

const TRAILER: &str = "Emit <<call name {json}>>.\nExample: <<call do_thing {\"k\":\"v\"}>>";

fn tool(name: &str, description: Option<&str>, parameters: Option<Value>) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: description.map(str::to_string),
        parameters,
    }
}

fn assert_trailer(text: &str) {
    let mut lines = text.lines();
    let last = lines.next_back().unwrap();
    let prev = lines.next_back().unwrap();
    assert_eq!(format!("{prev}\n{last}"), TRAILER);
    assert!(!last.contains("create_calendar_event"));
    assert!(!last.contains("send_email"));
}

fn norm(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                if k == "description" {
                    continue;
                }
                let mut child = norm(&map[k]);
                if k == "required"
                    && let Value::Array(arr) = &mut child
                {
                    arr.sort_by(|a, b| a.as_str().unwrap_or("").cmp(b.as_str().unwrap_or("")));
                    if arr.is_empty() {
                        continue;
                    }
                }
                out.insert(k.clone(), child);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(norm).collect()),
        other => other.clone(),
    }
}

fn assert_round_trip(tools: &[ToolDef]) {
    let compact = encode_tools(tools).unwrap();
    assert_trailer(&compact.text);
    let back = decode_tools(&compact).unwrap();
    assert_eq!(back.len(), tools.len());
    for (orig, dec) in tools.iter().zip(&back) {
        assert_eq!(orig.name, dec.name);
        let want = orig
            .description
            .as_deref()
            .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "));
        let want = want.filter(|d| !d.is_empty());
        assert_eq!(dec.description, want);
        match (&orig.parameters, &dec.parameters) {
            (None, None) => {}
            (Some(a), Some(b)) => assert_eq!(norm(a), norm(b), "schema drifted for {}", orig.name),
            _ => panic!("parameters presence drifted for {}", orig.name),
        }
    }
    assert_eq!(encode_tools(&back).unwrap().text, compact.text);
}

#[test]
fn exact_line_sorts_properties_and_marks_optional() {
    let tools = [tool(
        "do_work",
        Some("Do the work."),
        Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "count": {"type": "integer"},
                "flag": {"type": "boolean"},
                "ratio": {"type": "number"},
                "when": {"type": "string", "format": "date-time"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "mode": {"type": "string", "enum": ["fast", "slow"]}
            },
            "required": ["title", "count"]
        })),
    )];
    let text = encode_tools(&tools).unwrap().text;
    assert_eq!(
        text,
        "do_work(count:int, flag?:bool, mode?:fast|slow, ratio?:number, tags?:[str], title:str, when?:datetime) - Do the work.\n".to_string() + TRAILER
    );
}

#[test]
fn round_trip_two_generic_tools() {
    let tools = [
        tool("ping", Some("Say hello."), None),
        tool(
            "make_box",
            Some("Build a box."),
            Some(json!({
                "type": "object",
                "properties": {
                    "label": {"type": "string"}
                },
                "required": ["label"]
            })),
        ),
    ];
    assert_round_trip(&tools);
    let text = encode_tools(&tools).unwrap().text;
    assert!(text.starts_with("ping - Say hello.\nmake_box(label:str) - Build a box.\n"));
}

#[test]
fn round_trip_sample_tools() {
    let tools = [
        tool(
            "create_calendar_event",
            Some("Create an event in the user's calendar."),
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
        ),
        tool(
            "send_email",
            Some("Send an email from the user's account."),
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
        ),
    ];
    assert_round_trip(&tools);
}

#[test]
fn optional_versus_required() {
    let tools = [tool(
        "note",
        None,
        Some(json!({
            "type": "object",
            "properties": {
                "body": {"type": "string"},
                "tag": {"type": "string"}
            },
            "required": ["body"]
        })),
    )];
    let text = encode_tools(&tools).unwrap().text;
    assert!(text.starts_with("note(body:str, tag?:str)\n"));
    assert_round_trip(&tools);
}

#[test]
fn enums_round_trip_including_special_characters() {
    let tools = [tool(
        "paint",
        Some("Pick a color."),
        Some(json!({
            "type": "object",
            "properties": {
                "shade": {"type": "string", "enum": ["public", "a|b", "x y", "str"]},
                "only": {"type": "string", "enum": ["str"]}
            },
            "required": ["shade", "only"]
        })),
    )];
    let text = encode_tools(&tools).unwrap().text;
    assert!(
        text.starts_with("paint(only:\"str\", shade:public|\"a|b\"|\"x y\"|str) - Pick a color.\n")
    );
    assert_round_trip(&tools);
}

#[test]
fn nested_objects_round_trip() {
    let tools = [tool(
        "place",
        Some("Place a user."),
        Some(json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "age": {"type": "integer"}
                    },
                    "required": ["name"]
                }
            },
            "required": ["user"]
        })),
    )];
    let text = encode_tools(&tools).unwrap().text;
    assert!(text.starts_with("place(user:{age?:int, name:str}) - Place a user.\n"));
    assert_round_trip(&tools);
}

#[test]
fn arrays_of_objects_round_trip() {
    let tools = [tool(
        "pack",
        Some("Pack items."),
        Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sku": {"type": "string"},
                            "qty": {"type": "integer"}
                        },
                        "required": ["sku"]
                    }
                }
            },
            "required": ["items"]
        })),
    )];
    let text = encode_tools(&tools).unwrap().text;
    assert!(text.starts_with("pack(items:[{qty?:int, sku:str}]) - Pack items.\n"));
    assert_round_trip(&tools);
}

#[test]
fn unsupported_schemas_fail_closed() {
    let cases = [
        json!({"type": "object", "properties": {"a": {"$ref": "#/defs/A"}}, "required": ["a"]}),
        json!({"type": "object", "properties": {"a": {"oneOf": [{"type": "string"}]}}, "required": ["a"]}),
        json!({"type": "object", "properties": {"a": {"anyOf": [{"type": "string"}]}}, "required": ["a"]}),
        json!({"type": "object", "properties": {"a": {"allOf": [{"type": "string"}]}}, "required": ["a"]}),
        json!({"type": "object", "patternProperties": {"^x": {"type": "string"}}, "properties": {}}),
        json!({"type": "object", "properties": {}, "additionalProperties": {"type": "string", "pattern": "^x"}}),
        json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "^x"}}}),
        json!({"type": "object", "properties": {"a": {"type": "string", "format": "email"}}}),
        json!({"type": "string"}),
    ];
    for schema in cases {
        let err = encode_tools(&[tool("t", None, Some(schema))]).unwrap_err();
        assert_eq!(err, Error::Unsupported);
    }
}

#[test]
fn encoding_is_deterministic_regardless_of_property_order() {
    let a = tool(
        "note",
        Some("Write  a\nnote."),
        Some(json!({
            "type": "object",
            "required": ["z", "a"],
            "properties": {
                "z": {"type": "integer"},
                "a": {"type": "string"}
            }
        })),
    );
    let b = tool(
        "note",
        Some("Write  a\nnote."),
        Some(json!({
            "type": "object",
            "properties": {
                "a": {"type": "string"},
                "z": {"type": "integer"}
            },
            "required": ["a", "z"]
        })),
    );
    let first = encode_tools(std::slice::from_ref(&a)).unwrap();
    let second = encode_tools(std::slice::from_ref(&a)).unwrap();
    let flipped = encode_tools(&[b]).unwrap();
    assert_eq!(first.text, second.text);
    assert_eq!(first.text, flipped.text);
    assert!(
        first
            .text
            .starts_with("note(a:str, z:int) - Write a note.\n")
    );
    assert_round_trip(&[a]);
}

#[test]
fn names_and_descriptions_escape_special_characters() {
    let tools = [tool(
        "say \"hi\"",
        Some("path \\ tmp"),
        Some(json!({
            "type": "object",
            "properties": {
                "a b": {"type": "string", "enum": ["c\\d"]}
            },
            "required": ["a b"]
        })),
    )];
    let text = encode_tools(&tools).unwrap().text;
    assert!(text.starts_with("\"say \\\"hi\\\"\"(\"a b\":\"c\\\\d\") - path \\\\ tmp\n"));
    assert_round_trip(&tools);
}

#[test]
fn decode_rejects_text_outside_the_grammar() {
    let err = decode_tools(&CompactTools {
        text: "not a tool".into(),
    })
    .unwrap_err();
    assert_eq!(err, Error::InvalidCompact);
}

fn book_and_mail() -> Vec<ToolDef> {
    vec![
        tool(
            "book_slot",
            Some("Book a slot."),
            Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]},
                    "owner": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}},
                        "required": ["name"]
                    }
                },
                "required": ["title", "start"]
            })),
        ),
        tool(
            "mail_note",
            Some("Send a note."),
            Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"}
                },
                "required": ["to", "subject", "body"]
            })),
        ),
        tool("ping", Some("Say hello."), None),
    ]
}

fn feed(chunks: &[&str], tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools);
    for chunk in chunks {
        decoder.push(chunk)?;
    }
    decoder.finish()
}

#[test]
fn decode_plain_text_and_text_around_calls() {
    let tools = book_and_mail();
    assert_eq!(decode_calls("What's the weather?", &tools).unwrap(), vec![]);
    let text = "Sure.\n<<call ping {}>>\nDone. <<call book_slot {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>> thanks";
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(
        calls,
        vec![
            ToolCall {
                name: "ping".into(),
                arguments: "{}".into(),
            },
            ToolCall {
                name: "book_slot".into(),
                arguments: r#"{"title":"Retro","start":"2026-10-04T10:00:00+05:30"}"#.into(),
            },
        ]
    );
}

#[test]
fn decode_keeps_gt_gt_inside_strings() {
    let tools = book_and_mail();
    let raw = r#"{"to":["sam@example.com"],"subject":"a >> b","body":"x"}"#;
    let text = format!("<<call mail_note {raw}>>");
    let calls = decode_calls(&text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "mail_note");
    assert_eq!(calls[0].arguments, raw);
}

#[test]
fn decode_fails_closed_on_unknown_tool_and_bad_args() {
    let tools = book_and_mail();
    assert_eq!(
        decode_calls("<<call delete_everything {}>>", &tools).unwrap_err(),
        Error::UnknownTool
    );
    assert_eq!(
        decode_calls(
            r#"<<call book_slot {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
            &tools
        )
        .unwrap_err(),
        Error::InvalidArguments
    );
    assert_eq!(
        decode_calls(
            r#"<<call book_slot {"title":"x","start":"tomorrow"}>>"#,
            &tools
        )
        .unwrap_err(),
        Error::InvalidArguments
    );
    assert_eq!(
        decode_calls(r#"<<call book_slot {"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":"30"}>>"#, &tools)
            .unwrap_err(),
        Error::InvalidArguments
    );
    assert_eq!(
        decode_calls(r#"<<call ping {"x":1}>>"#, &tools).unwrap_err(),
        Error::InvalidArguments
    );
    decode_calls(
        r#"<<call book_slot {"title":"x","start":"2026-10-05T15:00:00Z"}>>"#,
        &tools,
    )
    .unwrap();
    assert_eq!(
        decode_calls(
            r#"<<call book_slot {"title":"x","start":"2026-10-05T15:00:00"}>>"#,
            &tools
        )
        .unwrap_err(),
        Error::InvalidArguments
    );
}

#[test]
fn decode_nested_object_and_array() {
    let tools = book_and_mail();
    let raw = r#"{"title":"Retro","start":"2026-10-04T10:00:00+05:30","attendees":["a@b.c"],"owner":{"name":"An"}}"#;
    let calls = decode_calls(&format!("<<call book_slot {raw}>>"), &tools).unwrap();
    assert_eq!(calls[0].arguments, raw);
    assert_eq!(
        decode_calls(
            r#"<<call book_slot {"title":"Retro","start":"2026-10-04T10:00:00+05:30","owner":{}}>>"#,
            &tools
        )
        .unwrap_err(),
        Error::InvalidArguments
    );
}

#[test]
fn stream_split_inside_utf8_matches_full_decode() {
    let tools = book_and_mail();
    let text = "Go. <<call mail_note {\"to\":[\"sam@example.com\"],\"subject\":\"café 😀\",\"body\":\"東京\"}>> ok";
    let expect = decode_calls(text, &tools).unwrap();
    let bytes = text.as_bytes();
    for i in 0..=bytes.len() {
        let mut decoder = StreamDecoder::new(&tools);
        decoder.push(&bytes[..i]).unwrap();
        decoder.push(&bytes[i..]).unwrap();
        assert_eq!(decoder.finish().unwrap(), expect, "split at {i}");
    }
    let emoji = "😀".as_bytes();
    let mut decoder = StreamDecoder::new(&tools);
    decoder.push(&emoji[..1]).unwrap();
    assert_eq!(decoder.finish().unwrap_err(), Error::InvalidArguments);
}

#[test]
fn stream_split_matches_full_decode_at_every_byte() {
    let tools = book_and_mail();
    let text =
        "Go. <<call book_slot {\"title\":\"Ret>>ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>> ok";
    let expect = decode_calls(text, &tools).unwrap();
    for i in 0..=text.len() {
        let Some(left) = text.get(..i) else { continue };
        let right = text.get(i..).unwrap();
        let got = feed(&[left, right], &tools).unwrap();
        assert_eq!(got, expect, "split at {i}");
    }
    let chunks = [
        "<<ca",
        "ll book_slot {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let joined = chunks.concat();
    assert_eq!(
        feed(&chunks, &tools).unwrap(),
        decode_calls(&joined, &tools).unwrap()
    );
}

#[test]
fn decode_string_keeps_braces_quotes_and_backslashes() {
    let tools = book_and_mail();
    let raw = r#"{"to":["sam@example.com"],"subject":"a >> b { } \"q\" \\","body":"x"}"#;
    let calls = decode_calls(&format!("<<call mail_note {raw}>>"), &tools).unwrap();
    assert_eq!(calls[0].arguments, raw);
    let parsed: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(parsed["subject"], "a >> b { } \"q\" \\");
}

#[test]
fn decode_unicode_and_emoji_arguments() {
    let tools = book_and_mail();
    let raw = r#"{"to":["sam@example.com"],"subject":"café 😀","body":"東京"}"#;
    let calls = decode_calls(&format!("note <<call mail_note {raw}>> end"), &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, raw);
    let parsed: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(parsed["subject"], "café 😀");
    assert_eq!(parsed["body"], "東京");
}

fn bag(extra: Option<Value>) -> ToolDef {
    let mut schema = json!({
        "type": "object",
        "properties": {"label": {"type": "string"}},
        "required": ["label"]
    });
    if let Some(extra) = extra {
        schema
            .as_object_mut()
            .unwrap()
            .insert("additionalProperties".to_string(), extra);
    }
    tool("bag", Some("Hold items."), Some(schema))
}

#[test]
fn additional_properties_true_or_schema_allows_extra_keys() {
    let open = [bag(Some(json!(true)))];
    assert_round_trip(&open);
    assert!(encode_tools(&open).unwrap().text.contains("..."));
    decode_calls(r#"<<call bag {"label":"a","n":1}>>"#, &open).unwrap();

    let typed = [bag(Some(json!({"type": "string"})))];
    assert_round_trip(&typed);
    assert!(encode_tools(&typed).unwrap().text.contains("...:str"));
    decode_calls(r#"<<call bag {"label":"a","note":"x"}>>"#, &typed).unwrap();
    assert_eq!(
        decode_calls(r#"<<call bag {"label":"a","n":1}>>"#, &typed).unwrap_err(),
        Error::InvalidArguments
    );
}

#[test]
fn additional_properties_false_or_absent_rejects_extra_keys() {
    let closed = [bag(Some(json!(false)))];
    encode_tools(&closed).unwrap();
    assert_eq!(
        decode_calls(r#"<<call bag {"label":"a","note":"x"}>>"#, &closed).unwrap_err(),
        Error::InvalidArguments
    );
    decode_calls(r#"<<call bag {"label":"a"}>>"#, &closed).unwrap();

    let absent = [bag(None)];
    assert_eq!(
        decode_calls(r#"<<call bag {"label":"a","note":"x"}>>"#, &absent).unwrap_err(),
        Error::InvalidArguments
    );
}

#[test]
fn additional_properties_unrepresentable_schema_is_unsupported() {
    let tools = [bag(Some(
        json!({"type": "string", "pattern": "^x", "oneOf": [{"type": "string"}]}),
    ))];
    assert_eq!(encode_tools(&tools).unwrap_err(), Error::Unsupported);
    let tools = [bag(Some(json!({"$ref": "#/defs/A"})))];
    assert_eq!(encode_tools(&tools).unwrap_err(), Error::Unsupported);
}

#[test]
fn extra_unknown_field_is_invalid_arguments() {
    let tools = book_and_mail();
    let err = decode_calls(
        r#"<<call book_slot {"title":"x","start":"2026-10-05T15:00:00Z","note":"extra"}>>"#,
        &tools,
    )
    .unwrap_err();
    assert_eq!(err, Error::InvalidArguments);
}

#[test]
fn bad_enum_is_invalid_arguments() {
    let tools = book_and_mail();
    let err = decode_calls(
        r#"<<call book_slot {"title":"x","start":"2026-10-05T15:00:00Z","visibility":"secret"}>>"#,
        &tools,
    )
    .unwrap_err();
    assert_eq!(err, Error::InvalidArguments);
}

#[test]
fn missing_required_field_is_invalid_arguments() {
    let tools = book_and_mail();
    let err = decode_calls(
        r#"<<call book_slot {"start":"2026-10-05T15:00:00Z"}>>"#,
        &tools,
    )
    .unwrap_err();
    assert_eq!(err, Error::InvalidArguments);
}

#[test]
fn tool_not_offered_is_unknown_tool() {
    let offered = [tool("ping", Some("Say hello."), None)];
    let err = decode_calls(
        r#"<<call book_slot {"title":"x","start":"2026-10-05T15:00:00Z"}>>"#,
        &offered,
    )
    .unwrap_err();
    assert_eq!(err, Error::UnknownTool);
}

#[test]
fn made_up_tool_round_trips_nested_array_optional_and_enum() {
    let tools = [tool(
        "file_bundle",
        Some("File a bundle."),
        Some(json!({
            "type": "object",
            "properties": {
                "label": {"type": "string"},
                "priority": {"type": "string", "enum": ["low", "high"]},
                "owner": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "nick": {"type": "string"}
                    },
                    "required": ["name"]
                },
                "parts": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sku": {"type": "string"},
                            "qty": {"type": "integer"}
                        },
                        "required": ["sku"]
                    }
                }
            },
            "required": ["label", "owner", "parts"]
        })),
    )];
    assert_round_trip(&tools);
    let raw = r#"{"label":"box","owner":{"name":"An"},"parts":[{"sku":"a1"}],"priority":"low"}"#;
    let calls = decode_calls(&format!("<<call file_bundle {raw}>>"), &tools).unwrap();
    assert_eq!(calls[0].arguments, raw);
    let bare = r#"{"label":"box","owner":{"name":"An"},"parts":[{"sku":"a1"}]}"#;
    decode_calls(&format!("<<call file_bundle {bare}>>"), &tools).unwrap();
}

#[test]
fn stream_finish_does_not_guess_a_partial_call() {
    let tools = book_and_mail();
    let err = feed(&[r#"<<call book_slot {"title":"x"}"#], &tools).unwrap_err();
    assert_eq!(err, Error::InvalidArguments);
    let err = feed(&["<<call book_slot {\"title\":\"x\"}>"], &tools).unwrap_err();
    assert_eq!(err, Error::InvalidArguments);
}
