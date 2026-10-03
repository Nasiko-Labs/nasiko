//! Property tests: for generated schemas and arguments, encode → decode_tools is lossless,
//! render → decode round-trips at any stream split, and decoding never panics on noise.

use nasiko_tool_compact::{
    StreamDecoder, ToolCall, ToolDef, decode, decode_tools, encode_tools, render_calls,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// A generated schema node paired with a generator of valid values for it.
#[derive(Debug, Clone)]
enum S {
    Str,
    Int,
    Bool,
    DateTime,
    Enum(Vec<String>),
    Array(Box<S>),
    Object(Vec<(String, S, bool, Option<String>)>),
}

fn name() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_][a-z0-9_]{0,10}",
        // names that need quoting
        "[a-z]{1,4} [a-z]{1,4}",
    ]
}

fn text() -> impl Strategy<Value = String> {
    // Includes the grammar's own punctuation and multi-byte characters.
    prop::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9 ]{1,6}",
            Just(">>".to_string()),
            Just("<<call ".to_string()),
            Just("\"".to_string()),
            Just("\\".to_string()),
            Just("'".to_string()),
            Just("}{".to_string()),
            Just("|,:?".to_string()),
            Just("\n".to_string()),
            Just("é🎉".to_string()),
        ],
        0..5,
    )
    .prop_map(|v| v.concat())
}

fn schema() -> BoxedStrategy<S> {
    let leaf = prop_oneof![
        Just(S::Str),
        Just(S::Int),
        Just(S::Bool),
        Just(S::DateTime),
        prop::collection::btree_set(prop_oneof!["[a-z][a-z_-]{0,6}", text()], 1..4)
            .prop_map(|s| S::Enum(s.into_iter().collect())),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            inner.clone().prop_map(|s| S::Array(Box::new(s))),
            fields(inner).prop_map(S::Object),
        ]
    })
    .boxed()
}

fn fields(
    inner: impl Strategy<Value = S> + Clone,
) -> impl Strategy<Value = Vec<(String, S, bool, Option<String>)>> {
    prop::collection::btree_map(
        name(),
        (inner, any::<bool>(), prop::option::of(text())),
        0..4,
    )
    .prop_map(|m| m.into_iter().map(|(k, (s, r, d))| (k, s, r, d)).collect())
}

fn to_json(s: &S) -> Value {
    match s {
        S::Str => json!({"type": "string"}),
        S::Int => json!({"type": "integer"}),
        S::Bool => json!({"type": "boolean"}),
        S::DateTime => json!({"type": "string", "format": "date-time"}),
        S::Enum(v) => json!({"type": "string", "enum": v}),
        S::Array(i) => json!({"type": "array", "items": to_json(i)}),
        S::Object(fs) => {
            let mut props = Map::new();
            for (k, s, _, d) in fs {
                let mut p = to_json(s);
                if let Some(d) = d {
                    p["description"] = json!(d);
                }
                props.insert(k.clone(), p);
            }
            let req: Vec<&String> = fs.iter().filter(|f| f.2).map(|f| &f.0).collect();
            let mut o = json!({"type": "object", "properties": props});
            if !req.is_empty() {
                o["required"] = json!(req);
            }
            o
        }
    }
}

fn value(s: &S) -> BoxedStrategy<Value> {
    match s {
        S::Str => text().prop_map(Value::from).boxed(),
        S::Int => any::<i32>().prop_map(Value::from).boxed(),
        S::Bool => any::<bool>().prop_map(Value::from).boxed(),
        S::DateTime => Just(json!("2026-10-04T10:00:00+05:30")).boxed(),
        S::Enum(v) => prop::sample::select(v.clone())
            .prop_map(Value::from)
            .boxed(),
        S::Array(i) => prop::collection::vec(value(i), 0..3)
            .prop_map(Value::Array)
            .boxed(),
        S::Object(fs) => {
            let parts: Vec<BoxedStrategy<Option<(String, Value)>>> = fs
                .iter()
                .map(|(k, s, req, _)| {
                    let k = k.clone();
                    let v = value(s);
                    if *req {
                        v.prop_map(move |v| Some((k.clone(), v))).boxed()
                    } else {
                        prop::option::of(v.prop_map(move |v| (k.clone(), v))).boxed()
                    }
                })
                .collect();
            parts
                .prop_map(|kv| Value::Object(kv.into_iter().flatten().collect()))
                .boxed()
        }
    }
}

fn tool_and_args() -> impl Strategy<Value = (ToolDef, Value, Option<String>)> {
    (fields(schema()), prop::option::of(text())).prop_flat_map(|(fs, desc)| {
        let root = S::Object(fs);
        let tool = ToolDef {
            name: "gen_tool".into(),
            description: desc.clone(),
            parameters: Some(to_json(&root)),
        };
        (Just(tool), value(&root), Just(desc))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn schema_survives_encoding((tool, _, _) in tool_and_args()) {
        let compact = encode_tools(std::slice::from_ref(&tool)).unwrap();
        prop_assert_eq!(decode_tools(&compact).unwrap(), vec![tool]);
    }

    #[test]
    fn valid_calls_round_trip_at_any_split(
        (tool, args, _) in tool_and_args(),
        before in "[a-zA-Z .]{0,12}",
        split in any::<prop::sample::Index>(),
    ) {
        let call = ToolCall { name: tool.name.clone(), arguments: args.to_string() };
        let text = format!("{before}{}", render_calls(std::slice::from_ref(&call)));
        let tools = [tool];

        let whole = decode(&text, &tools).unwrap();
        prop_assert_eq!(&whole.calls, &vec![call]);
        prop_assert_eq!(&whole.text, &before);

        let cuts: Vec<usize> = (0..=text.len()).filter(|&i| text.is_char_boundary(i)).collect();
        let cut = cuts[split.index(cuts.len())];
        let mut d = StreamDecoder::new(&tools).unwrap();
        d.push(&text[..cut]).unwrap();
        d.push(&text[cut..]).unwrap();
        prop_assert_eq!(d.finish().unwrap(), whole);
    }

    #[test]
    fn noise_never_panics_and_never_invents_calls(noise in text()) {
        // No `<<call ` + known tool name in the noise ⇒ never a call.
        let tools = [ToolDef { name: "known_tool_x".into(), description: None, parameters: None }];
        if let Ok(d) = decode(&noise, &tools) {
            prop_assert!(d.calls.is_empty());
            prop_assert_eq!(d.text, noise);
        }
    }
}
