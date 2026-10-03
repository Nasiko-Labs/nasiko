use nasiko_tool_compact::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "create_event".into(),
        description: Some("Create an event.".into()),
        parameters: json!({
            "type":"object", "properties":{
                "title":{"type":"string","minLength":1},
                "start":{"type":"string","format":"date-time"},
                "visibility":{"type":"string","enum":["public","private"]},
                "attendees":{"type":"array","items":{"type":"object","properties":{
                    "email":{"type":"string"}, "count":{"type":"integer","minimum":1}
                },"required":["email"],"additionalProperties":false}}
            },"required":["title","start"],"additionalProperties":false
        }),
    }]
}

fn valid_call() -> ToolCall {
    ToolCall {
        name: "create_event".into(),
        arguments: json!({
            "title":"Review", "start":"2026-10-05T15:00:00+05:30"
        }),
    }
}

#[test]
fn definitions_roundtrip_without_hidden_originals() {
    let tools = tools();
    let encoded = encode_tools(&tools).unwrap();
    let independent = CompactTools {
        definitions: encoded.definitions.clone(),
    };
    assert_eq!(decode_tools(&independent).unwrap(), tools);
}

#[test]
fn version_metadata_is_not_a_tool_but_a_tool_named_ct1_is_preserved() {
    let mut defs = tools();
    defs[0].name = "ct1".into();
    let encoded = encode_tools(&defs).unwrap();
    assert!(encoded.catalog().unwrap().starts_with("ct1("));
    let independent = CompactTools {
        definitions: format!("ct1\n{}", encoded.catalog().unwrap()),
    };
    assert_eq!(decode_tools(&independent).unwrap(), defs);
    assert!(
        CompactTools {
            definitions: "future\n".into()
        }
        .catalog()
        .is_err()
    );
}

#[test]
fn all_supported_keywords_and_presence_roundtrip() {
    let schemas = [
        json!({"type":"object"}),
        json!({"type":"object","required":[]}),
        json!({"type":"object","properties":{}}),
        json!({"type":"object","properties":{},"required":[]}),
        json!({"type":"object","description":"params","title":"Title","default":{},"examples":[{}],
            "readOnly":false,"writeOnly":true,"deprecated":false,"minProperties":0,"maxProperties":4,
            "additionalProperties":{"type":"number","minimum":-10,"exclusiveMaximum":10},
            "properties":{
                "":{"type":"null","const":null},
                "a,b?:{}@#/中文":{"type":"array","minItems":0,"maxItems":10,"uniqueItems":true,
                    "items":{"type":"string","format":"date","minLength":0,"maxLength":20,"enum":["2026-10-05"]}},
                "p":{"type":"integer","maximum":50,"exclusiveMinimum":1},
                "b":{"type":"boolean"}
            }, "required":["b","p"]}),
        json!({"type":"object","properties":{"p":{"type":"array"},"q":{"type":"object","enum":[{}]}}}),
    ];
    for parameters in schemas {
        let defs = vec![ToolDef {
            name: "t".into(),
            description: Some("quoted \"\n >> description".into()),
            parameters,
        }];
        assert_eq!(decode_tools(&encode_tools(&defs).unwrap()).unwrap(), defs);
    }
}

#[test]
fn multiple_calls_preserve_plain_text_and_escapes() {
    let defs = tools();
    let mut first = valid_call();
    first.arguments["title"] = json!("a >> b, \"quoted\", \\ path, \n 中文 <<call x {}>>");
    let calls = vec![first, valid_call()];
    let rendered = render_calls(&calls, &defs).unwrap();
    let decoded = decode_response(&format!("Before\n{rendered}\nAfter"), &defs).unwrap();
    assert_eq!(decoded.calls, calls);
    assert_eq!(decoded.text, "Before\n\n\nAfter");
    assert_eq!(
        decode_calls("Plain answer with < and >>.", &defs).unwrap(),
        vec![]
    );
}

#[test]
fn malformed_native_text_protocol_never_executes_a_partial_batch() {
    let defs = tools();
    let valid = render_calls(&[valid_call()], &defs).unwrap();
    for output in [
        format!("[TOOL_CALLS]create_event(title:bad) {valid}"),
        format!("{valid} [TOOL_CALLS]create_event(title:bad)"),
    ] {
        for split in 0..=output.len() {
            let mut stream = StreamDecoder::new(&defs).unwrap();
            let result = (|| {
                stream.push(&output[..split])?;
                stream.push(&output[split..])?;
                stream.finish()
            })();
            assert_eq!(result.unwrap_err().code(), "malformed_output");
        }
    }
    let mut literal = valid_call();
    literal.arguments["title"] = json!("Literal [TOOL_CALLS] in a value");
    let output = render_calls(&[literal.clone()], &defs).unwrap();
    assert_eq!(decode_calls(&output, &defs).unwrap(), vec![literal]);
}

#[test]
fn native_json_frames_mix_with_compact_frames_at_every_utf8_boundary() {
    let defs = tools();
    let mut call = valid_call();
    call.arguments["title"] = json!("中文 [TOOL_CALLS] >> \"quoted\"");
    let args = call.arguments.to_string();
    let compact = render_calls(&[call.clone()], &defs).unwrap();
    for frame in [
        format!("[TOOL_CALLS]create_event{args}"),
        format!("[TOOL_CALLS]create_event({args})"),
    ] {
        let output = format!("Before {frame} between {compact} after");
        for split in (0..=output.len()).filter(|i| output.is_char_boundary(*i)) {
            let mut stream = StreamDecoder::new(&defs).unwrap();
            stream.push(&output[..split]).unwrap();
            stream.push(&output[split..]).unwrap();
            let result = stream.finish_response().unwrap();
            assert_eq!(result.calls, vec![call.clone(), call.clone()]);
            assert_eq!(result.text, "Before  between  after");
        }
    }
    for bad in [
        format!("[TOOL_CALLS]create_event{args}}}"),
        format!("[TOOL_CALLS]create_event({args}))"),
        "[TOOL_CALLS]create_event{".into(),
        "[TOOL_CALLS]".into(),
    ] {
        assert_eq!(
            decode_calls(&bad, &defs).unwrap_err().code(),
            "malformed_output"
        );
    }
    assert_eq!(
        decode_calls("[TOOL_CALLS]unknown{}", &defs)
            .unwrap_err()
            .code(),
        "unknown_tool"
    );
    assert_eq!(
        decode_calls("[TOOL_CALLS]create_event{}", &defs)
            .unwrap_err()
            .code(),
        "invalid_arguments"
    );
}

#[test]
fn unknown_missing_enum_type_and_nested_fail_closed() {
    let defs = tools();
    assert_eq!(
        decode_calls("<<call remove_all {}>>", &defs)
            .unwrap_err()
            .code(),
        "unknown_tool"
    );
    for args in [
        json!({}),
        json!({"title":3,"start":"2026-10-05T15:00:00Z"}),
        json!({"title":"x","start":"not-a-date"}),
        json!({"title":"","start":"2026-10-05T15:00:00Z"}),
        json!({"title":"x","start":"2026-10-05T15:00:00Z","visibility":"secret"}),
        json!({"title":"x","start":"2026-10-05T15:00:00Z","unexpected":1}),
        json!({"title":"x","start":"2026-10-05T15:00:00Z","attendees":[{"count":2}]}),
        json!({"title":"x","start":"2026-10-05T15:00:00Z","attendees":[{"email":"e","count":0}]}),
    ] {
        let output = format!("<<call create_event {args}>>");
        assert_eq!(
            decode_calls(&output, &defs).unwrap_err().code(),
            "invalid_arguments"
        );
    }
}

#[test]
fn malformed_duplicate_and_truncated_output_never_produces_calls() {
    let defs = tools();
    for text in [
        "<<ca",
        "<<call",
        "<<call create_event",
        "<<call create_event {",
        "<<call create_event {}>",
        "<<call create_event []>>",
        "<<call create_event {bad}>>",
        "<<call create_event {]>>",
        "<<callX create_event {}>>",
    ] {
        assert!(decode_calls(text, &defs).is_err(), "{text}");
    }
    let duplicate =
        r#"<<call create_event {"title":"a","title":"b","start":"2026-10-05T15:00:00Z"}>>"#;
    assert_eq!(
        decode_calls(duplicate, &defs).unwrap_err().code(),
        "invalid_arguments"
    );
    let nested_duplicate = r#"<<call create_event {"title":"a","start":"2026-10-05T15:00:00Z","attendees":[{"email":"a","email":"b"}]}>>"#;
    assert!(decode_calls(nested_duplicate, &defs).is_err());
    let first = render_calls(&[valid_call()], &defs).unwrap();
    assert!(decode_calls(&format!("{first} <<call remove_all {{}}>>"), &defs).is_err());
}

#[test]
fn every_utf8_split_including_closing_marker() {
    let defs = tools();
    let mut call = valid_call();
    call.arguments["title"] = json!("中文 >> \" x \\");
    let output = render_calls(&[call.clone()], &defs).unwrap();
    for boundary in (0..=output.len()).filter(|i| output.is_char_boundary(*i)) {
        let mut stream = StreamDecoder::new(&defs).unwrap();
        stream.push(&output[..boundary]).unwrap();
        stream.push(&output[boundary..]).unwrap();
        assert_eq!(stream.finish().unwrap(), vec![call.clone()]);
    }
    let mut stream = StreamDecoder::new(&defs).unwrap();
    for c in output.chars() {
        stream.push(&c.to_string()).unwrap();
    }
    assert_eq!(stream.finish().unwrap(), vec![call]);
}

#[test]
fn optional_whitespace_is_grammar_not_json_repair() {
    let defs = tools();
    assert_eq!(
        decode_calls(
            r#"<<call create_event{"title":"Review","start":"2026-10-05T15:00:00+05:30"}>>"#,
            &defs
        )
        .unwrap(),
        vec![valid_call()]
    );
    assert!(
        decode_calls(
            r#"<<call create_event{"title":"Review","start":"2026-10-05T15:00:00+05:30"}}>>"#,
            &defs
        )
        .is_err()
    );
}

#[test]
fn both_prefix_forms_and_a_tool_named_call_are_unambiguous() {
    let defs = tools();
    let args = r#"{"title":"Review","start":"2026-10-05T15:00:00+05:30"}"#;
    for prefix in ["create_event", "call create_event"] {
        let output = format!("<<{prefix} {args}>>");
        for boundary in 0..=output.len() {
            let mut stream = StreamDecoder::new(&defs).unwrap();
            stream.push(&output[..boundary]).unwrap();
            stream.push(&output[boundary..]).unwrap();
            assert_eq!(stream.finish().unwrap(), vec![valid_call()]);
        }
    }
    let mut named = defs[0].clone();
    named.name = "call".into();
    for output in [
        format!("<<call{args}>>"),
        format!("<<call {args}>>"),
        format!("<<call call {args}>>"),
    ] {
        assert_eq!(
            decode_calls(&output, &[named.clone()]).unwrap()[0].name,
            "call"
        );
    }
    assert_eq!(
        decode_calls("<<delete_everything {}>>", &defs)
            .unwrap_err()
            .code(),
        "unknown_tool"
    );
}

#[test]
fn unsupported_schema_does_not_silently_drop_constraints() {
    for unsupported in [
        json!({"type":"object","$ref":"https://example.invalid/schema"}),
        json!({"type":"object","properties":{"x":{"type":["string","null"]}}}),
        json!({"type":"object","properties":{"x":{"type":"string","pattern":"abc"}}}),
        json!({"type":"object","properties":{"x":{"type":"string","format":"email"}}}),
        json!({"type":"object","properties":{"x":{"type":"number","multipleOf":0.01}}}),
        json!({"type":"object","required":["undeclared"]}),
    ] {
        let defs = [ToolDef {
            name: "t".into(),
            description: None,
            parameters: unsupported,
        }];
        assert_eq!(
            encode_tools(&defs).unwrap_err().code(),
            "unsupported_schema"
        );
    }
}

#[test]
fn bounds_and_poisoned_stream() {
    let defs = tools();
    let mut stream = StreamDecoder::new(&defs).unwrap();
    assert_eq!(
        stream.push(&"x".repeat(MAX_BYTES + 1)).unwrap_err(),
        CompactError::LimitExceeded
    );
    assert_eq!(
        stream.push("anything").unwrap_err(),
        CompactError::LimitExceeded
    );
    assert_eq!(stream.finish().unwrap_err(), CompactError::LimitExceeded);
    let valid = render_calls(&[valid_call()], &defs).unwrap();
    assert_eq!(
        decode_calls(&valid.repeat(MAX_CALLS + 1), &defs).unwrap_err(),
        CompactError::LimitExceeded
    );
}

#[test]
fn integer_bounds_do_not_round_to_float() {
    let defs = [ToolDef {
        name: "t".into(),
        description: None,
        parameters: json!({
            "type":"object","properties":{"x":{"type":"integer","maximum":9007199254740992u64}},
            "required":["x"]
        }),
    }];
    assert!(decode_calls(r#"<<call t {"x":9007199254740992}>>"#, &defs).is_ok());
    assert!(decode_calls(r#"<<call t {"x":9007199254740993}>>"#, &defs).is_err());
}

#[test]
fn json_schema_numeric_equality_and_array_uniqueness() {
    let defs = [ToolDef {
        name: "t".into(),
        description: None,
        parameters: json!({
            "type":"object","properties":{
                "x":{"type":"number","enum":[1]},
                "a":{"type":"array","items":{"type":"number"},"uniqueItems":true}
            },"required":["x"]
        }),
    }];
    assert!(decode_calls(r#"<<call t {"x":1.0}>>"#, &defs).is_ok());
    assert!(decode_calls(r#"<<call t {"x":1,"a":[1,1.0]}>>"#, &defs).is_err());
}

fn schema_strategy() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"type":"string"})),
        Just(json!({"type":"integer","minimum":0})),
        Just(json!({"type":"boolean"})),
        Just(json!({"type":"null"})),
    ]
    .prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            inner
                .clone()
                .prop_map(|item| json!({"type":"array","items":item})),
            (inner.clone(), inner, any::<bool>()).prop_map(|(a, b, required)| {
                let mut s = json!({"type":"object","properties":{"a":a,"b":b}});
                if required {
                    s["required"] = json!(["b", "a"]);
                }
                s
            }),
        ]
    })
}

proptest! {
    #[test]
    fn arbitrary_strings_roundtrip_through_random_chunks(title in ".{0,150}", chunks in prop::collection::vec(1usize..20, 1..30)) {
        let defs = tools();
        let mut call = valid_call();
        call.arguments["title"] = Value::String(format!("x{title}"));
        let output = render_calls(&[call.clone()], &defs).unwrap();
        let chars:Vec<char> = output.chars().collect();
        let mut stream = StreamDecoder::new(&defs).unwrap();
        let mut i=0; let mut n=0;
        while i < chars.len() {
            let end=(i+chunks[n % chunks.len()]).min(chars.len());
            stream.push(&chars[i..end].iter().collect::<String>()).unwrap();
            i=end; n+=1;
        }
        prop_assert_eq!(stream.finish().unwrap(), vec![call]);
    }

    #[test]
    fn nested_schema_roundtrip(schema in schema_strategy(), description in prop::option::of(".{0,80}")) {
        let defs=vec![ToolDef { name:"tool".into(), description, parameters:json!({
            "type":"object","properties":{"payload":schema},"required":["payload"]
        }) }];
        prop_assert_eq!(decode_tools(&encode_tools(&defs).unwrap()).unwrap(), defs);
    }

    #[test]
    fn arbitrary_text_does_not_panic(text in ".{0,500}") {
        let _ = decode_calls(&text, &tools());
    }
}
