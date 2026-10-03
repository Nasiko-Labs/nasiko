use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools,
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn tool(name: &str, parameters: Value) -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": name,
            "description": format!("Use {name} safely."),
            "parameters": parameters
        }
    }))
    .expect("test tool is valid")
}

fn calendar_tool() -> ToolDef {
    tool(
        "create_calendar_event",
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "duration_min": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Duration in minutes"
                },
                "location": {
                    "type": "object",
                    "properties": {"room": {"type": "string"}},
                    "required": ["room"],
                    "additionalProperties": false
                }
            },
            "required": ["title", "start"],
            "additionalProperties": false
        }),
    )
}

#[test]
fn supported_schema_round_trip_preserves_every_field() {
    let mut without_description = calendar_tool();
    without_description.function.name = "without_description".to_string();
    without_description.function.description = None;
    let mut empty_description = calendar_tool();
    empty_description.function.name = "empty_description".to_string();
    empty_description.function.description = Some(String::new());
    let contradictory_enum = tool(
        "contradictory_enum",
        json!({
            "type": "object",
            "properties": {"code": {"type": "string", "enum": [7]}}
        }),
    );
    let tools = vec![
        calendar_tool(),
        without_description,
        empty_description,
        contradictory_enum,
    ];

    let compact = encode_tools(&tools).expect("schema is supported");
    let decoded = decode_tools(&compact).expect("compact form decodes");

    assert_eq!(decoded, tools);
    assert!(compact.definitions().contains("extra=false"));
    assert!(compact.definitions().contains("start:datetime"));
    assert!(
        compact
            .definitions()
            .contains("visibility?:\"public\"|\"private\"")
    );
    assert!(
        compact
            .definitions()
            .contains("location?:{room:str} extra=false")
    );
    assert!(
        compact
            .definitions()
            .contains("duration_min?:int where {\"minimum\":1}")
    );
    assert!(compact.definitions().contains("code?:str enum[7]"));
}

#[test]
fn required_and_optional_arguments_are_validated() {
    let tools = [calendar_tool()];
    let valid = decode_calls(
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T10:00:00+05:30"}>>"#,
        &tools,
    )
    .expect("optional fields may be absent");
    assert_eq!(valid.len(), 1);

    let missing = decode_calls(
        r#"<<call create_calendar_event {"start":"2026-10-05T10:00:00+05:30"}>>"#,
        &tools,
    )
    .expect_err("required title is missing");
    assert_eq!(missing.code().as_str(), "invalid_arguments");
}

#[test]
fn nested_objects_arrays_enums_and_additional_properties_are_validated() {
    let tools = [calendar_tool()];
    let valid = decode_calls(
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T10:00:00+05:30","visibility":"private","attendees":["riya@example.com"],"location":{"room":"A"}}>>"#,
        &tools,
    )
    .expect("nested arguments satisfy the schema");
    assert_eq!(valid[0].arguments["location"]["room"], "A");

    for invalid in [
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T10:00:00+05:30","visibility":"secret"}>>"#,
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T10:00:00+05:30","attendees":[7]}>>"#,
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T10:00:00+05:30","extra":true}>>"#,
    ] {
        assert_eq!(
            decode_calls(invalid, &tools)
                .expect_err("arguments violate the schema")
                .code()
                .as_str(),
            "invalid_arguments"
        );
    }
}

#[test]
fn unknown_tools_and_duplicate_definitions_are_rejected() {
    let tools = [calendar_tool()];
    assert_eq!(
        decode_calls("<<call erase_everything {}>>", &tools)
            .expect_err("tool is unknown")
            .code()
            .as_str(),
        "unknown_tool"
    );

    let duplicates = [calendar_tool(), calendar_tool()];
    assert!(matches!(
        encode_tools(&duplicates),
        Err(CompactError::DuplicateTool(_))
    ));
}

#[test]
fn references_are_recursively_unsupported() {
    let tool = tool(
        "lookup",
        json!({
            "type": "object",
            "properties": {"item": {"$ref": "https://example.test/item.json"}}
        }),
    );
    let error = encode_tools(&[tool]).expect_err("reference would require resolution");
    assert_eq!(error.code().as_str(), "unsupported_schema");
}

#[test]
fn malformed_truncated_and_duplicate_key_calls_fail_closed() {
    let tools = [calendar_tool()];
    assert_eq!(
        decode_calls(r#"<<call create_calendar_event {"title":"Retro"}"#, &tools)
            .expect_err("closing marker is missing")
            .code()
            .as_str(),
        "truncated_call"
    );
    assert_eq!(
        decode_calls("<<call create_calendar_event []>>", &tools)
            .expect_err("arguments must be an object")
            .code()
            .as_str(),
        "malformed_syntax"
    );
    assert_eq!(
        decode_calls(
            r#"<<call create_calendar_event {"title":"A","title":"B","start":"2026-10-05T10:00:00+05:30"}>>"#,
            &tools,
        )
        .expect_err("duplicate keys are ambiguous")
        .code()
        .as_str(),
        "invalid_arguments"
    );
}

#[test]
fn quotes_escapes_unicode_and_closing_markers_inside_strings_are_safe() {
    let tools = [tool(
        "echo",
        json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        }),
    )];
    let text = r#"before <<call echo {"text":"नमस्ते: a >> b, quote \" and slash \\"}>> after"#;
    let calls = decode_calls(text, &tools).expect("string content cannot close the call");
    assert_eq!(
        calls[0].arguments["text"],
        "नमस्ते: a >> b, quote \" and slash \\"
    );
}

#[test]
fn multiple_calls_text_and_plain_answers_are_supported_atomically() {
    let tools = [tool(
        "echo",
        json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        }),
    )];
    let output = "first <<call echo {\"text\":\"a\"}>> middle <<call echo {\"text\":\"b\"}>> end";
    let calls = decode_calls(output, &tools).expect("both calls are valid");
    assert_eq!(calls.len(), 2);
    assert!(
        decode_calls("A normal answer with < punctuation.", &tools)
            .expect("plain answer is valid")
            .is_empty()
    );

    let mixed = "<<call echo {\"text\":\"ok\"}>> <<call echo {}>>";
    assert!(decode_calls(mixed, &tools).is_err());
}

#[test]
fn every_character_boundary_matches_non_streaming_decode() {
    let tools = [calendar_tool()];
    let fixture = "text <<call create_calendar_event {\"title\":\"設計 >> review\",\"start\":\"2026-10-05T10:00:00+05:30\"}>> tail";
    let expected = decode_calls(fixture, &tools).expect("fixture is valid");
    for split in fixture
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(fixture.len()))
    {
        let mut decoder = StreamDecoder::new(&tools).expect("tools are valid");
        decoder.feed(&fixture[..split]).expect("first chunk parses");
        decoder
            .feed(&fixture[split..])
            .expect("second chunk parses");
        assert_eq!(decoder.finish().expect("stream is complete"), expected);
    }
}

proptest! {
    #[test]
    fn random_chunk_partitions_match_non_streaming(chunk_sizes in prop::collection::vec(1usize..12, 0..40)) {
        let tools = [calendar_tool()];
        let fixture = "text <<call create_calendar_event {\"title\":\"設計 >> review\",\"start\":\"2026-10-05T10:00:00+05:30\"}>> tail";
        let expected = decode_calls(fixture, &tools).expect("fixture is valid");
        let boundaries = fixture.char_indices().map(|(index, _)| index).chain(std::iter::once(fixture.len())).collect::<Vec<_>>();
        let mut decoder = StreamDecoder::new(&tools).expect("tools are valid");
        let mut character = 0;
        for size in chunk_sizes {
            if character >= boundaries.len() - 1 {
                break;
            }
            let next = (character + size).min(boundaries.len() - 1);
            decoder.feed(&fixture[boundaries[character]..boundaries[next]]).expect("chunk parses");
            character = next;
        }
        decoder.feed(&fixture[boundaries[character]..]).expect("remainder parses");
        prop_assert_eq!(decoder.finish().expect("stream is complete"), expected);
    }

    #[test]
    fn supported_primitive_schemas_round_trip(required in any::<bool>(), enum_values in prop::collection::vec("[a-z]{1,8}", 1..6)) {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "value": {"type": "string", "enum": enum_values},
                "count": {"type": "integer", "minimum": 0}
            },
            "additionalProperties": false
        });
        if required {
            schema["required"] = json!(["value"]);
        }
        let tools = [tool("property_test", schema)];
        let compact = encode_tools(&tools).expect("schema is supported");
        prop_assert_eq!(decode_tools(&compact).expect("compact schema decodes"), tools);
    }

    #[test]
    fn arbitrary_malformed_input_never_panics(input in any::<String>()) {
        let tools = [calendar_tool()];
        let mut decoder = StreamDecoder::new(&tools).expect("tools are valid");
        let _ = decoder.feed(&input);
        let _ = decoder.finish();
    }
}

#[test]
fn decoded_shape_is_simple_and_router_ids_remain_external() {
    let tools = [calendar_tool()];
    let calls = decode_calls(
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T10:00:00+05:30"}>>"#,
        &tools,
    )
    .expect("call is valid");
    assert_eq!(
        calls,
        vec![ToolCall {
            name: "create_calendar_event".to_string(),
            arguments: json!({
                "title": "Retro",
                "start": "2026-10-05T10:00:00+05:30"
            }),
        }]
    );
}
