use nasiko_tool_compact::{
    CompactError, CompactTools, Limits, StreamDecoder, ToolCall, ToolDef, decode_calls,
    decode_output, decode_tools, encode_tools, render_calls,
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn tool(schema: Value) -> ToolDef {
    ToolDef {
        name: "run".into(),
        description: Some("Use only for the requested operation.".into()),
        parameters: Some(schema),
    }
}

fn text_tool() -> ToolDef {
    tool(
        json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
    )
}

#[test]
fn schemas_reconstruct_from_text_without_originals() {
    let schemas = vec![
        json!({}),
        json!(true),
        json!(false),
        json!({"type":"object"}),
        json!({"type":"object","properties":{}}),
        json!({"type":"object","properties":{},"required":[]}),
        json!({"type":"object","required":["not_declared"]}),
        json!({"type":["object","null"],"properties":{"field":{"type":"string"}}}),
        json!({"type":"object","properties":{"optional":{"type":"string","default":"x","examples":["hello"]},"z":{"type":"integer"},"a":{"type":"boolean"}},"required":["z","a"]}),
        json!({"type":"object","properties":{"nested":{"type":"object","properties":{"a b?":{"type":"array","items":{"type":"string","enum":["a","b"],"description":"Quotes: \" and >> 🌍"}}},"required":["a b?"],"additionalProperties":false}},"required":["nested"],"title":"Complete schema"}),
        json!({"type":"object","additionalProperties":{"type":"integer","minimum":0}}),
    ];
    for schema in schemas {
        let tools = vec![tool(schema)];
        let encoded = encode_tools(&tools).unwrap();
        let transported: CompactTools =
            serde_json::from_str(&serde_json::to_string(&encoded).unwrap()).unwrap();
        assert_eq!(decode_tools(&transported).unwrap(), tools);
    }
    let no_parameters = vec![ToolDef {
        name: "noop".into(),
        description: None,
        parameters: None,
    }];
    assert_eq!(
        decode_tools(&encode_tools(&no_parameters).unwrap()).unwrap(),
        no_parameters
    );
}

#[test]
fn preserves_multiple_calls_and_surrounding_text() {
    let decoded = decode_output("Before. <<call run {\"text\":\"first\"}>> Middle. <<call run {\"text\":\"second\"}>> After.", &[text_tool()]).unwrap();
    assert_eq!(decoded.text, "Before.  Middle.  After.");
    assert_eq!(decoded.calls.len(), 2);
    assert_eq!(decoded.calls[1].arguments, json!({"text":"second"}));
}

#[test]
fn plain_answers_remain_byte_identical() {
    for text in ["", "No tool is needed. 🌏", "x < y", "A trailing <"] {
        let decoded = decode_output(text, &[text_tool()]).unwrap();
        assert_eq!(decoded.text, text);
        assert!(decoded.calls.is_empty());
    }
}

#[test]
fn every_byte_boundary_handles_escapes_markers_and_unicode() {
    let text = "Before <<call run {\"text\":\"Escaped \\\" quote, \\\\ slash, >> and <<call, नमस्ते 🌏\"}>> after";
    let tools = [text_tool()];
    let expected = decode_output(text, &tools).unwrap();
    for split in 0..=text.len() {
        let mut decoder = StreamDecoder::new(&tools).unwrap();
        decoder.push_bytes(&text.as_bytes()[..split]).unwrap();
        decoder.push_bytes(&text.as_bytes()[split..]).unwrap();
        assert_eq!(decoder.finish().unwrap(), expected, "split {split}");
    }
}

#[test]
fn rejects_unknown_tools_missing_fields_invalid_types_enums_and_extra_keys() {
    assert_eq!(
        decode_calls("<<call missing {}>>", &[text_tool()]),
        Err(CompactError::UnknownTool)
    );
    for args in [
        "{}",
        "{\"text\":123}",
        "{\"text\":null}",
        "{\"text\":\"ok\",\"extra\":true}",
        "[]",
    ] {
        assert!(decode_calls(&format!("<<call run {args}>>"), &[text_tool()]).is_err());
    }
    let tools = [tool(
        json!({"type":"object","properties":{"visibility":{"type":"string","enum":["public","private"]}},"required":["visibility"]}),
    )];
    assert_eq!(
        decode_calls("<<call run {\"visibility\":\"secret\"}>>", &tools),
        Err(CompactError::InvalidArguments)
    );
}

#[test]
fn honors_nested_constraints_formats_and_additional_properties_default() {
    let tools = [tool(json!({"type":"object","properties":{
        "items":{"type":"array","minItems":1,"maxItems":2,"uniqueItems":true,"items":{"type":"integer","minimum":0,"maximum":3}},
        "time":{"type":"string","format":"date-time"},"text":{"type":"string","minLength":1,"maxLength":2}},"required":["items","time","text"]}))];
    let valid = json!({"items":[1,2],"time":"2026-10-03T10:00:00+05:30","text":"🌏","undeclared":"allowed"});
    let call = |args: &Value| format!("<<call run {args}>>");
    assert!(decode_calls(&call(&valid), &tools).is_ok());
    for (key, value) in [
        ("items", json!([])),
        ("items", json!([1, 1])),
        ("items", json!([4])),
        ("time", json!("tomorrow")),
        ("text", json!("long")),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert_eq!(
            decode_calls(&call(&invalid), &tools),
            Err(CompactError::InvalidArguments)
        );
    }
}

#[test]
fn rejects_duplicate_keys_at_every_nesting_level() {
    for args in [
        "{\"text\":\"first\",\"text\":\"second\"}",
        "{\"nested\":{\"key\":1,\"key\":2}}",
    ] {
        assert_eq!(
            decode_calls(&format!("<<call run {args}>>"), &[tool(json!({}))]),
            Err(CompactError::InvalidArguments)
        );
    }
}

#[test]
fn malformed_or_truncated_calls_never_release_a_partial_batch() {
    for suffix in [
        "<<call run {",
        "<<call run {\"text\":\"x\"}>",
        "<<call run {\"text\":\"x\"} junk",
        "<<call run {\"text\":\"x\",}>>",
        "<<cal run {}>>",
        "<<call ",
        "<<",
        "<<call run {\"text\":\"unterminated}>>",
    ] {
        let output = format!("<<call run {{\"text\":\"valid first call\"}}>>{suffix}");
        assert!(decode_calls(&output, &[text_tool()]).is_err(), "{suffix}");
    }
    let mut decoder = StreamDecoder::new(&[text_tool()]).unwrap();
    assert!(decoder.push("<<call missing {}>>").is_err());
    assert!(decoder.push("plain text").is_err());
    assert!(decoder.finish().is_err());
}

#[test]
fn output_call_count_and_depth_limits_fail_closed() {
    let tools = [text_tool()];
    let mut decoder = StreamDecoder::with_limits(
        &tools,
        Limits {
            max_output_bytes: 4,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(decoder.push("12345"), Err(CompactError::LimitExceeded));
    let mut decoder = StreamDecoder::with_limits(
        &tools,
        Limits {
            max_calls: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        decoder.push("<<call run {\"text\":\"ok\"}>>"),
        Err(CompactError::LimitExceeded)
    );
    let mut decoder = StreamDecoder::with_limits(
        &tools,
        Limits {
            max_json_depth: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        decoder.push("<<call run {\"nested\":{}"),
        Err(CompactError::LimitExceeded)
    );
    let mut decoder = StreamDecoder::with_limits(
        &tools,
        Limits {
            max_call_bytes: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        decoder.push("<<call run {\"text\""),
        Err(CompactError::LimitExceeded)
    );
}

#[test]
fn unsupported_features_and_invalid_schemas_are_explicit_errors() {
    for schema in [
        json!({"$ref":"https://example.com/schema"}),
        json!({"$ref":"file:///secret"}),
        json!({"type":"string","pattern":".*"}),
        json!({"oneOf":[{},{}]}),
        json!({"unknown":true}),
        json!({"format":"custom"}),
    ] {
        assert!(matches!(
            encode_tools(&[tool(schema)]),
            Err(CompactError::UnsupportedSchema(_))
        ));
    }
    for schema in [
        json!({"type":"bogus"}),
        json!({"type":"object","required":["a","a"]}),
        json!({"minimum":"bad"}),
    ] {
        assert!(matches!(
            encode_tools(&[tool(schema)]),
            Err(CompactError::InvalidSchema(_))
        ));
    }
    assert!(encode_tools(&[text_tool(), text_tool()]).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_unicode_arguments_survive_arbitrary_byte_chunks(text in ".{0,256}", chunk_size in 1usize..40) {
        let tools = [text_tool()];
        let calls = vec![ToolCall {name:"run".into(),arguments:json!({"text":text})}];
        let rendered = render_calls(&calls, &tools).unwrap();
        let mut decoder = StreamDecoder::new(&tools).unwrap();
        for chunk in rendered.as_bytes().chunks(chunk_size) { decoder.push_bytes(chunk).unwrap(); }
        prop_assert_eq!(decoder.finish().unwrap().calls, calls);
    }

    #[test]
    fn arbitrary_property_names_and_descriptions_roundtrip(name in ".{0,40}", description in ".{0,100}", required in any::<bool>()) {
        let mut schema = json!({"type":"object","properties":{name.clone():{"type":"string","description":description}}});
        if required { schema["required"] = json!([name]); }
        let tools = [tool(schema)];
        let encoded = encode_tools(&tools).unwrap();
        prop_assert_eq!(decode_tools(&encoded).unwrap(), tools.to_vec());
    }

    #[test]
    fn arbitrary_output_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let mut decoder = StreamDecoder::new(&[text_tool()]).unwrap();
        let _ = decoder.push_bytes(&bytes);
        let _ = decoder.finish();
    }
}
