//! Property tests: the invariants, over generated tools, arguments and chunkings rather than
//! hand-picked ones.

use std::collections::BTreeSet;

use nasiko_tool_compact::{
    CompactError, Decoded, Event, StreamDecoder, ToolCall, ToolDef, decode, decode_tools,
    encode_tools, render_calls,
};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::{Index, select};
use serde_json::{Map, Value, json};

// ── generators ──────────────────────────────────────────────────────────────────────────────

fn description() -> impl Strategy<Value = Option<String>> {
    proptest::option::of(any::<String>())
}

fn described(mut schema: Value, description: Option<String>) -> Value {
    if let (Some(map), Some(description)) = (schema.as_object_mut(), description) {
        map.insert("description".into(), Value::String(description));
    }
    schema
}

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"type": "string"})),
        select(vec!["date-time", "email", "uri", "datetime"])
            .prop_map(|format| json!({"type": "string", "format": format})),
        Just(json!({"type": "integer"})),
        Just(json!({"type": "number"})),
        Just(json!({"type": "boolean"})),
        Just(json!({"type": "object"})),
        vec(enum_value(), 1..4).prop_map(|values| json!({"type": "string", "enum": values})),
        vec(any::<i64>(), 1..4).prop_map(|values| json!({"type": "integer", "enum": values})),
    ]
}

/// Leans on the values that are hard to write bare: type keywords, integers, empty, quotes.
fn enum_value() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_][a-z0-9_-]{0,6}",
        select(vec![
            "str", "int", "obj", "datetime", "42", "-7", "", "it's", "a|b", "x y"
        ])
        .prop_map(str::to_string),
        any::<String>(),
    ]
}

fn key() -> impl Strategy<Value = String> {
    prop_oneof!["[a-z_][a-z0-9_-]{0,6}", any::<String>()]
}

fn object(inner: impl Strategy<Value = Value>) -> impl Strategy<Value = Value> {
    let props = vec((key(), inner, any::<bool>()), 0..4);
    (props, any::<bool>(), any::<bool>()).prop_map(|(props, closed, reversed)| {
        let mut properties = Map::new();
        let mut wanted = BTreeSet::new();
        for (key, schema, required) in props {
            if required {
                wanted.insert(key.clone());
            } else {
                wanted.remove(&key);
            }
            properties.insert(key, schema);
        }
        // `required` need not follow property order, and its own order has to survive.
        let mut required: Vec<Value> = properties
            .keys()
            .filter(|key| wanted.contains(*key))
            .map(|key| Value::String(key.clone()))
            .collect();
        if reversed {
            required.reverse();
        }
        let mut out = Map::new();
        out.insert("type".into(), json!("object"));
        out.insert("properties".into(), Value::Object(properties));
        if !required.is_empty() {
            out.insert("required".into(), Value::Array(required));
        }
        if closed {
            out.insert("additionalProperties".into(), json!(false));
        }
        Value::Object(out)
    })
}

fn node() -> impl Strategy<Value = Value> {
    let leaf = (leaf(), description()).prop_map(|(schema, d)| described(schema, d));
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            (inner.clone(), description())
                .prop_map(|(items, d)| described(json!({"type": "array", "items": items}), d)),
            (object(inner), description()).prop_map(|(schema, d)| described(schema, d)),
        ]
    })
}

fn tools() -> impl Strategy<Value = Vec<ToolDef>> {
    let tool = (
        "[A-Za-z0-9_.-]{1,10}",
        description(),
        proptest::option::of(object(node())),
    );
    vec(tool, 1..4).prop_map(|tools| {
        tools
            .into_iter()
            .enumerate()
            .map(|(i, (name, description, parameters))| ToolDef {
                // The index keeps generated names distinct.
                name: format!("{name}{i}"),
                description,
                parameters,
            })
            .collect()
    })
}

/// A value that satisfies `schema`.
fn instance(schema: &Value) -> BoxedStrategy<Value> {
    let choices = schema.get("enum").and_then(Value::as_array).cloned();
    match (schema["type"].as_str(), choices) {
        (_, Some(choices)) => select(choices).boxed(),
        (Some("string"), _) => any::<String>().prop_map(Value::String).boxed(),
        (Some("integer"), _) => any::<i64>().prop_map(Value::from).boxed(),
        (Some("number"), _) => (-1.0e9..1.0e9f64).prop_map(Value::from).boxed(),
        (Some("boolean"), _) => any::<bool>().prop_map(Value::Bool).boxed(),
        (Some("array"), _) => vec(instance(&schema["items"]), 0..3)
            .prop_map(Value::Array)
            .boxed(),
        _ => match schema.get("properties").and_then(Value::as_object) {
            None => Just(json!({"anything": [1, "goes"]})).boxed(),
            Some(properties) => {
                let required = schema.get("required").and_then(Value::as_array);
                let mut acc = Just(Map::new()).boxed();
                for (key, property) in properties {
                    let needed = required.is_some_and(|r| r.iter().any(|k| k == key));
                    let key = key.clone();
                    acc = (acc, instance(property), any::<bool>())
                        .prop_map(move |(mut map, value, include)| {
                            if needed || include {
                                map.insert(key.clone(), value);
                            }
                            map
                        })
                        .boxed();
                }
                acc.prop_map(Value::Object).boxed()
            }
        },
    }
}

/// Tools, plus one valid call to each.
fn tools_and_calls() -> impl Strategy<Value = (Vec<ToolDef>, Vec<ToolCall>)> {
    tools().prop_flat_map(|tools| {
        let args: Vec<BoxedStrategy<Value>> = tools
            .iter()
            .map(|tool| match &tool.parameters {
                Some(schema) => instance(schema),
                None => Just(json!({})).boxed(),
            })
            .collect();
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        (Just(tools), args).prop_map(move |(tools, args)| {
            let calls = names
                .iter()
                .zip(args)
                .map(|(name, args)| ToolCall {
                    name: name.clone(),
                    arguments: args.to_string(),
                })
                .collect();
            (tools, calls)
        })
    })
}

/// Prose that leans on the characters the marker is made of, but never contains one.
fn filler() -> impl Strategy<Value = String> {
    "[a-z <>{}\"\n]{0,12}|<<cal|<<|<".prop_filter("contains a marker", |s| !s.contains("<<call"))
}

/// Output assembled from the pieces a decoder has to tell apart, in any order.
fn noise() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("<<call ".to_string()),
        Just("<<call".to_string()),
        Just(">>".to_string()),
        Just("{".to_string()),
        Just("}".to_string()),
        Just("\"".to_string()),
        Just("\\".to_string()),
        Just("echo ".to_string()),
        Just("{\"msg\":\"hi\"}".to_string()),
        Just("{\"msg\":1}".to_string()),
        Just("nope ".to_string()),
        "[ a-z<>\\[\\]:,é]{0,6}",
    ];
    vec(piece, 0..12).prop_map(|pieces| pieces.concat())
}

fn echo() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "echo".into(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}},
            "required": ["msg"]
        })),
    }]
}

// ── helpers ─────────────────────────────────────────────────────────────────────────────────

fn chunked(text: &str, cuts: &[Index]) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut points: Vec<usize> = cuts.iter().map(|cut| cut.index(chars.len() + 1)).collect();
    points.push(chars.len());
    points.sort_unstable();
    let mut chunks = Vec::new();
    let mut start = 0;
    for end in points {
        chunks.push(chars[start..end].iter().collect());
        start = end;
    }
    chunks
}

fn decode_chunks(chunks: &[String], tools: &[ToolDef]) -> Result<Decoded, CompactError> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.push(chunk)?);
    }
    events.extend(decoder.finish()?);
    let mut decoded = Decoded::default();
    for event in events {
        match event {
            Event::Text(text) => decoded.text.push_str(&text),
            Event::Call(call) => decoded.calls.push(call),
        }
    }
    Ok(decoded)
}

// ── properties ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn schemas_round_trip_exactly(tools in tools()) {
        let compact = encode_tools(&tools).unwrap();
        prop_assert_eq!(decode_tools(&compact).unwrap(), tools);
    }

    #[test]
    fn the_compact_form_is_canonical(tools in tools()) {
        let once = encode_tools(&tools).unwrap();
        let again = encode_tools(&decode_tools(&once).unwrap()).unwrap();
        prop_assert_eq!(once, again);
    }

    #[test]
    fn definitions_are_one_line_per_tool(tools in tools()) {
        let compact = encode_tools(&tools).unwrap();
        prop_assert_eq!(compact.definitions.split('\n').count(), tools.len());
    }

    #[test]
    fn valid_calls_decode_to_themselves_amid_any_prose(
        (tools, calls) in tools_and_calls(),
        fillers in vec(filler(), 4),
    ) {
        let mut output = String::new();
        let mut prose = String::new();
        for (call, filler) in calls.iter().zip(&fillers) {
            output.push_str(filler);
            prose.push_str(filler);
            output.push_str(&render_calls(std::slice::from_ref(call)).unwrap());
        }
        output.push_str(&fillers[3]);
        prose.push_str(&fillers[3]);

        let decoded = decode(&output, &tools).unwrap();
        prop_assert_eq!(decoded.calls, calls);
        prop_assert_eq!(decoded.text, prose);
    }

    #[test]
    fn chunking_never_changes_a_valid_result(
        (tools, calls) in tools_and_calls(),
        cuts in vec(any::<Index>(), 0..8),
    ) {
        let output = render_calls(&calls).unwrap();
        let whole = decode(&output, &tools);
        prop_assert_eq!(decode_chunks(&chunked(&output, &cuts), &tools), whole);
    }

    #[test]
    fn chunking_never_changes_any_result_including_errors(
        output in noise(),
        cuts in vec(any::<Index>(), 0..8),
    ) {
        let whole = decode(&output, &echo());
        prop_assert_eq!(decode_chunks(&chunked(&output, &cuts), &echo()), whole);
    }

    #[test]
    fn whatever_decodes_is_a_real_call_to_an_offered_tool(output in noise()) {
        if let Ok(decoded) = decode(&output, &echo()) {
            for call in decoded.calls {
                prop_assert_eq!(&call.name, "echo");
                let args: Value = serde_json::from_str(&call.arguments).unwrap();
                prop_assert!(args["msg"].is_string());
                prop_assert_eq!(args.as_object().map(Map::len), Some(1));
            }
        }
    }

    #[test]
    fn dropping_a_required_argument_is_always_an_error((tools, calls) in tools_and_calls()) {
        for (tool, call) in tools.iter().zip(&calls) {
            let required = tool
                .parameters
                .as_ref()
                .and_then(|schema| schema.get("required"))
                .and_then(Value::as_array)
                .and_then(|names| names.first())
                .and_then(Value::as_str);
            let Some(required) = required else { continue };
            let mut args: Value = serde_json::from_str(&call.arguments).unwrap();
            args.as_object_mut().unwrap().remove(required);
            let output = format!("<<call {} {args}>>", call.name);
            let error = decode(&output, &tools).unwrap_err();
            prop_assert_eq!(error.as_label(), "invalid_arguments");
        }
    }

    #[test]
    fn decoding_arbitrary_text_never_panics(output in any::<String>(), cuts in vec(any::<Index>(), 0..4)) {
        let _ = decode(&output, &echo());
        let _ = decode_chunks(&chunked(&output, &cuts), &echo());
    }

    #[test]
    fn parsing_arbitrary_definitions_never_panics(definitions in prop_oneof![any::<String>(), "[a-z(){}\\[\\]:,?!|' <>-]{0,40}"]) {
        let _ = decode_tools(&nasiko_tool_compact::CompactTools {
            definitions,
            instructions: String::new(),
        });
    }
}
