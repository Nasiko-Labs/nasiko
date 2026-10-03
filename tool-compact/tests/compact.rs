use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools,
    render_call, strip_calls,
};
use serde_json::{Value, json};

fn event_tool() -> ToolDef {
    ToolDef {
        name: "create_event".into(),
        description: Some("Create an event.".into()),
        parameters: Some(json!({
            "type":"object",
            "properties":{
                "title":{"type":"string","description":"Event title"},
                "start":{"type":"string","format":"date-time"},
                "visibility":{"type":"string","enum":["public","private"]},
                "attendees":{"type":"array","items":{"type":"string"}},
                "location":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}
            },
            "required":["title","start"],
            "additionalProperties":false
        })),
    }
}

fn valid_call() -> ToolCall {
    ToolCall {
        name: "create_event".into(),
        arguments: json!({"title":"Review","start":"2026-10-05T15:00:00+05:30","visibility":"private"}),
    }
}

fn semantic_schema(mut value: Value) -> Value {
    match &mut value {
        Value::Object(fields) => {
            for child in fields.values_mut() {
                *child = semantic_schema(child.take());
            }
            if let Some(Value::Array(required)) = fields.get_mut("required") {
                required.sort_by_key(Value::to_string);
            }
        }
        Value::Array(items) => {
            for item in items {
                *item = semantic_schema(item.take());
            }
        }
        _ => {}
    }
    value
}

#[test]
fn definitions_roundtrip_and_render_deterministically() {
    let tools = vec![event_tool()];
    let encoded = encode_tools(&tools).unwrap();
    assert_eq!(encoded, encode_tools(&tools).unwrap());
    assert!(encoded.text.contains("title!:str"));
    assert!(encoded.text.contains("visibility?:str{"));
    assert!(encoded.text.contains("obj!{"));
    let decoded = decode_tools(&encoded).unwrap();
    assert_eq!(decoded[0].name, tools[0].name);
    assert_eq!(decoded[0].description, tools[0].description);
    assert_eq!(
        semantic_schema(decoded[0].parameters.clone().unwrap()),
        semantic_schema(tools[0].parameters.clone().unwrap())
    );
}

#[test]
fn absent_parameters_are_distinct_from_an_empty_object() {
    let tools = vec![ToolDef {
        name: "ping".into(),
        description: None,
        parameters: None,
    }];
    assert_eq!(decode_tools(&encode_tools(&tools).unwrap()).unwrap(), tools);
    assert!(decode_calls("<<call ping {}>>", &tools).is_ok());
    assert!(decode_calls("<<call ping {\"x\":1}>>", &tools).is_err());
}

#[test]
fn nullable_and_numeric_enums_roundtrip() {
    let tools = vec![ToolDef {
        name: "rate".into(),
        description: None,
        parameters: Some(json!({"type":"object","properties":{
            "score":{"type":["integer","null"],"enum":[1,2,null]}
        }})),
    }];
    let decoded = decode_tools(&encode_tools(&tools).unwrap()).unwrap();
    assert_eq!(decoded, tools);
    assert!(decode_calls("<<call rate {\"score\":null}>>", &tools).is_ok());
    assert!(decode_calls("<<call rate {\"score\":3}>>", &tools).is_err());
}

#[test]
fn root_and_array_item_descriptions_survive_roundtrip() {
    let tool = ToolDef {
        name: "invite".into(),
        description: Some("Invite attendees".into()),
        parameters: Some(json!({
            "type":"object",
            "description":"Invitation arguments",
            "properties":{"emails":{
                "type":"array",
                "description":"Recipient list",
                "items":{"type":"string","description":"Email address"}
            }},
            "required":["emails"]
        })),
    };
    assert_eq!(
        decode_tools(&encode_tools(std::slice::from_ref(&tool)).unwrap()).unwrap(),
        vec![tool]
    );
}

#[test]
fn multiple_calls_and_surrounding_text() {
    let tools = vec![event_tool()];
    let call = render_call(&valid_call());
    let text = format!("Planning now.\n{call}\nThen {call}\nDone.");
    assert_eq!(
        decode_calls(&text, &tools).unwrap(),
        vec![valid_call(), valid_call()]
    );
    assert_eq!(
        strip_calls(&text, &tools).unwrap(),
        "Planning now.\n\nThen \nDone."
    );
    assert!(decode_calls("Just an answer.", &tools).unwrap().is_empty());
}

#[test]
fn unknown_and_invalid_calls_fail_closed() {
    let tools = vec![event_tool()];
    assert!(matches!(
        decode_calls("<<call erase {}>>", &tools),
        Err(CompactError::UnknownTool(_))
    ));
    for args in [
        json!({"start":"date"}),
        json!({"title":"T","start":"date","visibility":"secret"}),
        json!({"title":"T","start":"date","attendees":[17]}),
        json!({"title":"T","start":"date","location":{"city":"X","bad":1}}),
        json!({"title":"T","start":"date","extra":1}),
        json!({"title":10,"start":"date"}),
    ] {
        let text = render_call(&ToolCall {
            name: "create_event".into(),
            arguments: args,
        });
        assert!(
            matches!(
                decode_calls(&text, &tools),
                Err(CompactError::InvalidArguments(_))
            ),
            "{text}"
        );
    }
    assert!(decode_calls("<<call create_event {bad json}>>", &tools).is_err());
    assert!(decode_calls("<<call create_event {}>", &tools).is_err());
    assert!(decode_calls("<<callx create_event {}>>", &tools).is_err());
}

#[test]
fn rejects_unsupported_schema_keywords() {
    for keyword in [
        "oneOf",
        "anyOf",
        "allOf",
        "$ref",
        "patternProperties",
        "minimum",
    ] {
        let mut tool = event_tool();
        tool.parameters.as_mut().unwrap()["properties"]["title"][keyword] = Value::Bool(true);
        assert!(matches!(
            encode_tools(&[tool]),
            Err(CompactError::UnsupportedSchema(_))
        ));
    }
}

#[test]
fn stream_matches_nonstreaming_at_every_character_boundary() {
    let tools = vec![event_tool()];
    let text = format!("start {} end", render_call(&valid_call()));
    let expected = decode_calls(&text, &tools).unwrap();
    for boundary in text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
    {
        let mut decoder = StreamDecoder::new(&tools).unwrap();
        let mut got = decoder.push(&text[..boundary]).unwrap();
        got.extend(decoder.push(&text[boundary..]).unwrap());
        got.extend(decoder.finish().unwrap());
        assert_eq!(got, expected, "split at {boundary}");
    }
}

#[test]
fn string_escaping_and_marker_inside_argument() {
    let tools = vec![event_tool()];
    let call = ToolCall {
        name: "create_event".into(),
        arguments: json!({"title":"a >> \"quote\" \\ slash","start":"date"}),
    };
    assert_eq!(
        decode_calls(&render_call(&call), &tools).unwrap(),
        vec![call]
    );
}

#[test]
fn malformed_inputs_do_not_panic_or_invent_calls() {
    let tools = vec![event_tool()];
    for n in 0..1024 {
        let text = (0..n)
            .map(|i| match (i * 17 + n) % 7 {
                0 => '<',
                1 => '>',
                2 => '{',
                3 => '}',
                4 => '"',
                5 => '\\',
                _ => 'x',
            })
            .collect::<String>();
        let _ = decode_calls(&text, &tools);
    }
    assert!(
        decode_calls(&"plain answer ".repeat(200_000), &tools)
            .unwrap()
            .is_empty()
    );
}
