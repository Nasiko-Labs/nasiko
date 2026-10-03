use nasiko_tool_compact::*;
use proptest::prelude::*;
use serde_json::{Value, json};
fn tools() -> Vec<ToolDef> {
    serde_json::from_value(json!([
        {
            "type": "function",
            "function": {
                "name": "send",
                "description": "Send a message",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Exact text"
                        },
                        "nested": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "mode": {
                                        "type": "string",
                                        "enum": [
                                            "a",
                                            "b"
                                        ]
                                    }
                                },
                                "required": [
                                    "mode"
                                ],
                                "additionalProperties": false
                            }
                        },
                        "at": {
                            "type": "string",
                            "format": "date-time"
                        }
                    },
                    "required": [
                        "text"
                    ],
                    "additionalProperties": false
                }
            }
        }
    ]))
    .unwrap()
}
fn call(text: &str) -> ToolCall {
    ToolCall {
        name: "send".into(),
        arguments: json!({"text":text}),
    }
}
#[test]
fn schema_roundtrip() {
    let t = tools();
    let c = encode_tools(&t).unwrap();
    assert_eq!(decode_tools(&c).unwrap(), t);
    assert!(c.definitions.contains("Exact text"));
}
#[test]
fn mixed_text_multiple_and_plain() {
    let t = tools();
    let c = vec![call("hello"), call("a >> b <<call unknown {}>>")];
    let r = format!("before {} after", render_calls(&c, &t).unwrap());
    assert_eq!(decode_calls(&r, &t).unwrap(), c);
    assert!(decode_calls("plain answer", &t).unwrap().is_empty());
}
#[test]
fn all_utf8_split_positions() {
    let t = tools();
    let c = vec![call("नमस्ते 🦀 \\\" >>")];
    let text = render_calls(&c, &t).unwrap();
    for index in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
        let mut d = StreamDecoder::new(&t).unwrap();
        d.push(&text[..index]).unwrap();
        d.push(&text[index..]).unwrap();
        assert_eq!(d.finish().unwrap(), c);
    }
}
#[test]
fn unknown_and_invalid_fail_closed() {
    let t = tools();
    assert_eq!(
        decode_calls("<<call nope {}>>", &t).unwrap_err(),
        CompactError::UnknownTool
    );
    for args in [
        json!({}),
        json!({"text":4}),
        json!({"text":"x","nested":[{"mode":"secret"}]}),
        json!({"text":"x","nested":[{}]}),
        json!({"text":"x","extra":1}),
        json!({"text":"x","at":"tomorrow"}),
    ] {
        let s = format!("<<call send {args}>>");
        assert_eq!(
            decode_calls(&s, &t).unwrap_err(),
            CompactError::InvalidArguments
        );
    }
}
#[test]
fn malformed_and_duplicate_keys() {
    for text in [
        "<<call send",
        "<<call send {",
        "<<call send {}>",
        "<<call send {\"text\":\"a\",\"text\":\"b\"}>>",
        "<<call send {\"text\":\"a\",\"nested\":[{\"mode\":\"a\",\"mode\":\"b\"}]}>>",
        "answer <<ca",
        "<<callx send {}>>",
        "<<call send {bad}>>",
    ] {
        assert_eq!(
            decode_calls(text, &tools()).unwrap_err(),
            CompactError::MalformedOutput,
            "{text}"
        );
    }
}
#[test]
fn atomic_and_sticky_errors() {
    let t = tools();
    let mut d = StreamDecoder::new(&t).unwrap();
    d.push(&render_calls(&[call("valid")], &t).unwrap())
        .unwrap();
    assert_eq!(
        d.push("<<call nope {}>>").unwrap_err(),
        CompactError::UnknownTool
    );
    assert_eq!(d.push("other").unwrap_err(), CompactError::UnknownTool);
    assert_eq!(d.finish().unwrap_err(), CompactError::UnknownTool);
    assert_eq!(d.finish().unwrap_err(), CompactError::Finished);
}
#[test]
fn unsupported_never_loses_information() {
    let mut t = tools();
    for (k, v) in [
        ("oneOf", json!([])),
        ("$ref", json!("#/x")),
        ("pattern", json!(".*")),
    ] {
        t[0].function.parameters.as_mut().unwrap()[k] = v;
        assert!(matches!(
            encode_tools(&t),
            Err(CompactError::UnsupportedSchema(_))
        ));
        t[0].function
            .parameters
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(k);
    }
}
#[test]
fn numeric_array_and_string_constraints() {
    let mut t = tools();
    t[0].function.parameters = Some(json!({
        "type": "object",
        "properties": {
            "x": {
                "type": "array",
                "minItems": 1,
                "maxItems": 2,
                "uniqueItems": true,
                "items": {
                    "type": "integer",
                    "minimum": 0,
                    "exclusiveMaximum": 10
                }
            },
            "s": {
                "type": "string",
                "minLength": 1,
                "maxLength": 2
            }
        },
        "required": [
            "x"
        ],
        "additionalProperties": false
    }));
    for args in [
        json!({"x":[]}),
        json!({"x":[1,1]}),
        json!({"x":[1.5]}),
        json!({"x":[10]}),
        json!({"x":[-1]}),
        json!({"x":[1],"s":"abc"}),
    ] {
        assert_eq!(
            decode_calls(&format!("<<call send {args}>>"), &t).unwrap_err(),
            CompactError::InvalidArguments
        );
    }
    assert!(decode_calls("<<call send {\"x\":[1,2],\"s\":\"🦀\"}>>", &t).is_ok());
    assert_eq!(decode_tools(&encode_tools(&t).unwrap()).unwrap(), t);
}
#[test]
fn limits() {
    let t = tools();
    let mut d = StreamDecoder::new(&t).unwrap();
    assert_eq!(
        d.push(&"x".repeat(MAX_BYTES + 1)).unwrap_err(),
        CompactError::ResourceLimit
    );
    let calls = vec![call("x"); MAX_CALLS + 1];
    assert_eq!(
        render_calls(&calls, &t).unwrap_err(),
        CompactError::ResourceLimit
    );
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test] fn arbitrary_strings_roundtrip(text in any::<String>()) {let t=tools();let c=vec![call(&text)];let r=render_calls(&c,&t).unwrap();prop_assert_eq!(decode_calls(&r,&t).unwrap(),c);}
    #[test] fn arbitrary_chunk_sizes(text in any::<String>(), size in 1usize..32) {let t=tools();let c=vec![call(&text)];let r=render_calls(&c,&t).unwrap();let chars:Vec<char>=r.chars().collect();let mut d=StreamDecoder::new(&t).unwrap();for chunk in chars.chunks(size){d.push(&chunk.iter().collect::<String>()).unwrap();}prop_assert_eq!(d.finish().unwrap(),c);}
    #[test] fn schema_tree_roundtrip(names in prop::collection::vec("[a-z]{1,8}",1..12), required in any::<bool>()) {let mut t=tools();let p:serde_json::Map<String,Value>=names.iter().map(|n|(n.clone(),json!({"type":"array","items":{"type":"string","enum":["x","y"]}}))).collect();t[0].function.parameters=Some(json!({"type":"object","properties":p}));if required{t[0].function.parameters.as_mut().unwrap()["required"]=json!(p.keys().collect::<Vec<_>>());}prop_assert_eq!(decode_tools(&encode_tools(&t).unwrap()).unwrap(),t);}
}

#[test]
fn unusual_names_annotations_and_required_presence() {
    let mut t = tools();
    for required in [
        None,
        Some(json!([])),
        Some(json!(["space key", "emoji🦀"])),
        Some(json!(["not-described"])),
    ] {
        let mut schema = json!({
            "type": "object",
            "title": "A \"quoted\" title",
            "description": "lines\nwith symbols } ] @ <<call >>",
            "properties": {
                "space key": {
                    "type": "string",
                    "default": "x",
                    "examples": [
                        "a"
                    ]
                },
                "emoji🦀": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {

                        },
                        "required": [

                        ]
                    }
                }
            },
            "additionalProperties": {
                "type": "integer"
            }
        });
        if let Some(required) = required {
            schema["required"] = required;
        }
        t[0].function.parameters = Some(schema);
        assert_eq!(decode_tools(&encode_tools(&t).unwrap()).unwrap(), t);
    }
}
#[test]
fn absent_parameters_and_descriptions() {
    let mut t = tools();
    t[0].function.description = None;
    t[0].function.parameters = None;
    assert_eq!(decode_tools(&encode_tools(&t).unwrap()).unwrap(), t);
    assert!(decode_calls("<<call send {}>>", &t).is_ok());
}
#[test]
fn schemas_with_boolean_and_null() {
    let mut t = tools();
    t[0].function.parameters = Some(
        json!({"type":"object","properties":{"a":{"type":"boolean"},"b":{"type":"null"}},"required":["a","b"]}),
    );
    assert_eq!(decode_tools(&encode_tools(&t).unwrap()).unwrap(), t);
    assert!(decode_calls("<<call send {\"a\":true,\"b\":null}>>", &t).is_ok());
    assert_eq!(
        decode_calls("<<call send {\"a\":1,\"b\":null}>>", &t).unwrap_err(),
        CompactError::InvalidArguments
    );
}

#[test]
fn numeric_precision_limits_fail_closed() {
    let mut t = tools();
    t[0].function.parameters =
        Some(json!({"type":"object","properties":{"n":{"type":"integer","minimum":0}}}));
    assert_eq!(
        decode_calls("<<call send {\"n\":9007199254740993}>>", &t).unwrap_err(),
        CompactError::InvalidArguments
    );
    t[0].function.parameters.as_mut().unwrap()["properties"]["n"]["minimum"] =
        json!(9007199254740994u64);
    assert!(matches!(
        encode_tools(&t),
        Err(CompactError::UnsupportedSchema(_))
    ));
}

#[test]
fn short_form_is_explicit_and_validated() {
    let t = tools();
    assert_eq!(
        decode_calls("<<send {\"text\":\"hello >>\"}>>", &t).unwrap(),
        vec![call("hello >>")]
    );
    assert_eq!(
        decode_calls("<<unknown {}>>", &t).unwrap_err(),
        CompactError::UnknownTool
    );
    assert_eq!(
        decode_calls("<<send {}>>", &t).unwrap_err(),
        CompactError::InvalidArguments
    );
    assert_eq!(
        decode_calls("<<send {not-json}>>", &t).unwrap_err(),
        CompactError::MalformedOutput
    );
}
#[test]
fn every_short_form_utf8_split() {
    let t = tools();
    let text = "<<send {\"text\":\"नमस्ते\"}>>";
    for i in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
        let mut d = StreamDecoder::new(&t).unwrap();
        d.push(&text[..i]).unwrap();
        d.push(&text[i..]).unwrap();
        assert_eq!(d.finish().unwrap(), vec![call("नमस्ते")]);
    }
}
#[test]
fn tool_named_call_works_in_both_forms() {
    let mut t = tools();
    t[0].function.name = "call".into();
    for text in [
        "<<call call {\"text\":\"hello\"}>>",
        "<<call {\"text\":\"hello\"}>>",
    ] {
        assert_eq!(
            decode_calls(text, &t).unwrap(),
            vec![ToolCall {
                name: "call".into(),
                arguments: json!({"text":"hello"})
            }]
        );
    }
}
