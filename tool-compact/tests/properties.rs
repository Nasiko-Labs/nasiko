//! Property tests: the decoder's guarantees must hold for inputs nobody wrote by hand.

use nasiko_tool_compact::{
    StreamDecoder, StreamEvent, ToolDef, call, decode, decode_calls, decode_tools, encode_tools,
    render_calls,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

fn email_tool() -> ToolDef {
    ToolDef {
        name: "send_email".into(),
        description: Some("Send an email.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string"},
                "body": {"type": "string"},
                "priority": {"type": "integer", "enum": [1, 2, 3]}
            },
            "required": ["to", "subject", "body"]
        })),
    }
}

/// Feed `text` split at the given byte offsets (snapped to char boundaries).
fn stream_split(
    text: &str,
    cuts: &[usize],
    tools: &[ToolDef],
) -> Result<(String, Vec<String>), String> {
    let mut offsets: Vec<usize> = cuts
        .iter()
        .map(|&c| {
            let mut c = c % (text.len() + 1);
            while !text.is_char_boundary(c) {
                c -= 1;
            }
            c
        })
        .collect();
    offsets.sort_unstable();
    offsets.dedup();
    let mut d = StreamDecoder::new(tools).map_err(|e| e.code().to_string())?;
    let mut events = Vec::new();
    let mut prev = 0;
    for &o in offsets.iter().chain(std::iter::once(&text.len())) {
        events.extend(d.push(&text[prev..o]).map_err(|e| e.code().to_string())?);
        prev = o;
    }
    events.extend(d.finish().map_err(|e| e.code().to_string())?);
    let mut prose = String::new();
    let mut args = Vec::new();
    for e in events {
        match e {
            StreamEvent::Text(t) => prose.push_str(&t),
            StreamEvent::Call(c) => args.push(c.arguments),
        }
    }
    Ok((prose, args))
}

fn whole(text: &str, tools: &[ToolDef]) -> Result<(String, Vec<String>), String> {
    decode(text, tools)
        .map(|d| (d.text, d.calls.into_iter().map(|c| c.arguments).collect()))
        .map_err(|e| e.code().to_string())
}

/// Strings biased toward the characters that matter to the grammar.
fn tricky_string() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just(">>".to_string()),
            Just("<<call ".to_string()),
            Just("}".to_string()),
            Just("{".to_string()),
            Just("\"".to_string()),
            Just("\\".to_string()),
            Just("\n".to_string()),
            Just("é🎉".to_string()),
            "[a-z ]{0,4}",
        ],
        0..8,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Any string content survives render → decode exactly, `>>` and quotes included.
    #[test]
    fn string_arguments_round_trip(subject in tricky_string(), body in tricky_string(), to in tricky_string()) {
        let args = json!({"to": [to], "subject": subject, "body": body});
        let calls = vec![call("send_email", &args)];
        let decoded = decode_calls(&render_calls(&calls), &[email_tool()]).unwrap();
        prop_assert_eq!(decoded[0].arguments_value().unwrap(), args);
    }

    /// Splitting the stream anywhere never changes the result: same prose, same calls, same error.
    #[test]
    fn stream_splits_are_invisible(
        pre in tricky_string(),
        subject in tricky_string(),
        post in tricky_string(),
        cuts in prop::collection::vec(any::<usize>(), 0..12),
    ) {
        let tools = [email_tool()];
        let args = json!({"to": ["a@b.c"], "subject": subject, "body": "b"});
        let text = format!("{pre}{}{post}", render_calls(&[call("send_email", &args)]));
        prop_assert_eq!(stream_split(&text, &cuts, &tools), whole(&text, &tools));
    }

    /// Arbitrary garbage never panics, and anything accepted is valid against the schema.
    #[test]
    fn arbitrary_output_never_yields_an_unvalidated_call(text in "(<<call |send_email |\\{|\\}|\"|>>|[a-z:,\\[\\] 0-9]){0,40}") {
        if let Ok(calls) = decode_calls(&text, &[email_tool()]) {
            for c in calls {
                prop_assert_eq!(c.name.as_str(), "send_email");
                let v = c.arguments_value().unwrap();
                let obj = v.as_object().unwrap();
                for k in ["to", "subject", "body"] {
                    prop_assert!(obj.contains_key(k));
                }
            }
        }
    }

    /// Mutating one argument of a valid call to the wrong type is always rejected.
    #[test]
    fn type_violations_are_always_rejected(field in prop::sample::select(vec!["to", "subject", "body", "priority"]), n in any::<i32>()) {
        let mut args = json!({"to": ["a@b.c"], "subject": "s", "body": "b", "priority": 1});
        args[field] = match field {
            "to" => json!("not-a-list"),
            "priority" => json!(if (1..=3).contains(&n) { 99 } else { n }),
            _ => json!(n),
        };
        let text = render_calls(&[call("send_email", &args)]);
        let err = decode_calls(&text, &[email_tool()]).unwrap_err();
        prop_assert_eq!(err.code(), "invalid_arguments");
    }

    /// Random supported schemas survive encode → decode_tools → encode unchanged.
    #[test]
    fn schemas_round_trip(params in object_schema(2)) {
        let tool = ToolDef { name: "t".into(), description: Some("d".into()), parameters: Some(params) };
        let compact = encode_tools(std::slice::from_ref(&tool)).unwrap();
        let back = decode_tools(&compact).unwrap();
        prop_assert_eq!(&back[0], &tool, "compact form was {}", compact.definitions);
        prop_assert_eq!(encode_tools(&back).unwrap(), compact);
    }
}

fn leaf_schema() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"type": "string"})),
        Just(json!({"type": "string", "format": "date-time"})),
        Just(json!({"type": "string", "format": "email"})),
        Just(json!({"type": "integer"})),
        Just(json!({"type": "number"})),
        Just(json!({"type": "boolean"})),
        prop::collection::btree_set("[a-z][a-z_]{0,6}|str|int|has space|\"q\"", 1..4).prop_map(
            |vals| json!({"type": "string", "enum": vals.into_iter().collect::<Vec<_>>()})
        ),
        prop::collection::btree_set(-5i64..50, 1..4).prop_map(
            |vals| json!({"type": "integer", "enum": vals.into_iter().collect::<Vec<_>>()})
        ),
    ]
}

fn type_schema(depth: u32) -> BoxedStrategy<Value> {
    if depth == 0 {
        return leaf_schema().boxed();
    }
    prop_oneof![
        3 => leaf_schema(),
        1 => type_schema(depth - 1).prop_map(|items| json!({"type": "array", "items": items})),
        1 => object_schema(depth - 1),
    ]
    .boxed()
}

fn object_schema(depth: u32) -> BoxedStrategy<Value> {
    prop::collection::btree_map(
        "[a-z][a-z0-9_]{0,8}",
        (
            type_schema(depth),
            prop::option::of("[A-Za-z ,()\"'>-]{1,20}"),
            any::<bool>(),
        ),
        0..5,
    )
    .prop_map(|props| {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for (name, (mut schema, desc, req)) in props {
            if let Some(d) = desc.and_then(|d| {
                let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
                (!d.is_empty()).then_some(d)
            }) {
                schema["description"] = Value::String(d);
            }
            if req {
                required.push(Value::String(name.clone()));
            }
            properties.insert(name, schema);
        }
        let mut obj = json!({"type": "object", "properties": properties});
        if !required.is_empty() {
            obj["required"] = Value::Array(required);
        }
        obj
    })
    .boxed()
}
