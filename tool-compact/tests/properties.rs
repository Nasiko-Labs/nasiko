//! Property tests over the public API.
//!
//! Schemas are generated in canonical form (the form `decode_tools` writes back), so schema
//! preservation can be checked with plain equality.

use nasiko_tool_compact::{
    Decoded, StreamDecoder, StreamEvent, ToolCall, ToolDef, decode, decode_calls, decode_tools,
    encode_tools, is_redundant_description, render_call,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

fn name() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,7}"
}

/// Descriptions in normalized form: trimmed, single spaces, including grammar-significant
/// characters (`#`, `'`, `:`, `(`) and non-ASCII.
fn description() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9#':()|?é ]{1,24}".prop_filter_map("blank", |s| {
        let n = s.split_whitespace().collect::<Vec<_>>().join(" ");
        (!n.is_empty()).then_some(n)
    })
}

fn with_description(schema: Value, desc: Option<String>) -> Value {
    let mut m = schema.as_object().cloned().unwrap_or_default();
    if let Some(d) = desc {
        m.insert("description".into(), Value::String(d));
    }
    Value::Object(m)
}

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"type": "string"})),
        Just(json!({"type": "string", "format": "date-time"})),
        (0u64..4, 4u64..9)
            .prop_map(|(a, b)| json!({"type": "string", "minLength": a, "maxLength": b})),
        Just(json!({"type": "integer"})),
        (-5i64..0, 0i64..5)
            .prop_map(|(a, b)| json!({"type": "integer", "minimum": a, "maximum": b})),
        Just(json!({"type": "number"})),
        Just(json!({"type": "boolean", "default": true})),
        Just(json!({"type": ["string", "null"]})),
        Just(json!({})),
        prop::collection::hash_set("[a-z'\\\\ \n|,()]{0,5}", 1..4).prop_map(|vals| {
            let mut v: Vec<String> = vals.into_iter().collect();
            v.sort();
            json!({"type": "string", "enum": v})
        }),
        prop::collection::btree_set(-50i64..50, 1..4).prop_map(
            |vals| json!({"type": "integer", "enum": vals.into_iter().collect::<Vec<_>>()})
        ),
    ]
}

/// A property schema (may carry a description).
fn schema() -> impl Strategy<Value = Value> {
    let leaf_d =
        (leaf(), prop::option::of(description())).prop_map(|(s, d)| with_description(s, d));
    leaf_d.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            // Array items carry no description (the grammar has nowhere to put one).
            (
                inner.clone(),
                prop::option::of(0u64..3),
                prop::option::of(description())
            )
                .prop_map(|(item, min, d)| {
                    let mut item = item.as_object().cloned().unwrap_or_default();
                    item.remove("description");
                    let mut m = Map::new();
                    m.insert("type".into(), "array".into());
                    if !item.is_empty() {
                        m.insert("items".into(), Value::Object(item));
                    }
                    if let Some(min) = min {
                        m.insert("minItems".into(), min.into());
                    }
                    with_description(Value::Object(m), d)
                }),
            (object(inner), prop::option::of(description()))
                .prop_map(|(o, d)| with_description(o, d)),
        ]
    })
}

fn object(inner: impl Strategy<Value = Value>) -> impl Strategy<Value = Value> {
    (
        prop::collection::btree_map(name(), inner, 0..5),
        any::<u64>(),
        any::<bool>(),
    )
        .prop_map(|(props, mask, closed)| {
            let required: Vec<Value> = props
                .keys()
                .enumerate()
                .filter(|(i, _)| mask >> (i % 64) & 1 == 1)
                .map(|(_, k)| Value::String(k.clone()))
                .collect();
            let mut m = Map::new();
            m.insert("type".into(), "object".into());
            m.insert(
                "properties".into(),
                Value::Object(props.into_iter().collect()),
            );
            if !required.is_empty() {
                m.insert("required".into(), Value::Array(required));
            }
            if closed {
                m.insert("additionalProperties".into(), false.into());
            }
            Value::Object(m)
        })
}

fn tool() -> impl Strategy<Value = ToolDef> {
    (name(), prop::option::of(description()), object(schema())).prop_map(|(n, d, params)| {
        let mut params = params;
        params.as_object_mut().unwrap().remove("description");
        ToolDef::function(&n, d.as_deref(), Some(params))
    })
}

fn tools() -> impl Strategy<Value = Vec<ToolDef>> {
    prop::collection::vec(tool(), 1..4).prop_map(|mut ts| {
        ts.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        ts.dedup_by(|a, b| a.function.name == b.function.name);
        ts
    })
}

/// A value valid against `schema` (canonical form as generated above).
fn value_for(schema: &Value) -> BoxedStrategy<Value> {
    let s = schema.as_object().cloned().unwrap_or_default();
    if let Some(Value::Array(vals)) = s.get("enum") {
        return prop::sample::select(vals.clone()).boxed();
    }
    match s.get("type") {
        Some(Value::String(t)) if t == "string" => {
            let min = s.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
            let max = s.get("maxLength").and_then(Value::as_u64).unwrap_or(12) as usize;
            prop::collection::vec(
                prop::sample::select(vec!['a', '"', '\\', '>', '}', '{', 'é', '\n', ' ']),
                min..=max,
            )
            .prop_map(|cs| Value::String(cs.into_iter().collect()))
            .boxed()
        }
        Some(Value::String(t)) if t == "integer" => {
            let min = s.get("minimum").and_then(Value::as_i64).unwrap_or(-1000);
            let max = s.get("maximum").and_then(Value::as_i64).unwrap_or(1000);
            (min..=max).prop_map(Value::from).boxed()
        }
        // Quarters are exact in binary, so the comparison does not depend on float parsing.
        Some(Value::String(t)) if t == "number" => (-4000i32..4000)
            .prop_map(|i| json!(f64::from(i) / 4.0))
            .boxed(),
        Some(Value::String(t)) if t == "boolean" => any::<bool>().prop_map(Value::Bool).boxed(),
        Some(Value::Array(_)) => {
            prop_oneof![Just(Value::Null), "[a-z]{0,4}".prop_map(Value::String)].boxed()
        }
        Some(Value::String(t)) if t == "array" => {
            let item = s.get("items").cloned().unwrap_or(json!({}));
            let min = s.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            prop::collection::vec(value_for(&item), min..min + 3)
                .prop_map(Value::Array)
                .boxed()
        }
        Some(Value::String(t)) if t == "object" => {
            let props = s
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let required: Vec<String> = s
                .get("required")
                .and_then(Value::as_array)
                .map(|r| {
                    r.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let fields: Vec<BoxedStrategy<Option<(String, Value)>>> = props
                .iter()
                .map(|(k, sub)| {
                    let k = k.clone();
                    let v = value_for(sub);
                    if required.contains(&k) {
                        v.prop_map(move |v| Some((k.clone(), v))).boxed()
                    } else {
                        prop::option::of(v)
                            .prop_map(move |v| v.map(|v| (k.clone(), v)))
                            .boxed()
                    }
                })
                .collect();
            fields
                .prop_map(|kvs| Value::Object(kvs.into_iter().flatten().collect()))
                .boxed()
        }
        _ => prop_oneof![Just(json!(1)), Just(json!("x")), Just(json!([true]))].boxed(),
    }
}

fn tools_and_call() -> impl Strategy<Value = (Vec<ToolDef>, String, Value)> {
    tools().prop_flat_map(|ts| {
        let n = ts.len();
        (Just(ts), 0..n).prop_flat_map(|(ts, i)| {
            let schema = ts[i].function.parameters.clone().unwrap();
            let name = ts[i].function.name.clone();
            (Just(ts), Just(name), value_for(&schema))
        })
    })
}

/// Remove field descriptions the encoder drops as redundant, at every depth of `schema`.
fn strip_redundant(schema: &mut Value, tool: &str) {
    if let Some(items) = schema.get_mut("items") {
        strip_redundant(items, tool);
    }
    let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for (field, sub) in props.iter_mut() {
        let m = sub.as_object_mut().unwrap();
        if let Some(Value::String(d)) = m.get("description")
            && is_redundant_description(d, field, tool)
        {
            m.remove("description");
        }
        strip_redundant(sub, tool);
    }
}

fn stream(chunks: &[&str], tools: &[ToolDef]) -> nasiko_tool_compact::Result<Decoded> {
    let mut d = StreamDecoder::new(tools)?;
    let mut out = Decoded::default();
    let mut events = Vec::new();
    for c in chunks {
        events.extend(d.push(c)?);
    }
    events.extend(d.finish()?);
    for e in events {
        match e {
            StreamEvent::Text(t) => out.text.push_str(&t),
            StreamEvent::Call { call, .. } => out.calls.push(call),
        }
    }
    Ok(out)
}

fn split_points(text: &str, cuts: &[usize]) -> Vec<usize> {
    let mut pts: Vec<usize> = cuts
        .iter()
        .map(|c| c % (text.len() + 1))
        .map(|mut c| {
            while !text.is_char_boundary(c) {
                c -= 1;
            }
            c
        })
        .collect();
    pts.push(0);
    pts.push(text.len());
    pts.sort();
    pts.dedup();
    pts
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    /// Every supported schema survives encode → decode_tools unchanged, except descriptions
    /// that only restate their own names (dropped by design).
    #[test]
    fn schemas_survive_the_round_trip(ts in tools()) {
        let compact = encode_tools(&ts).unwrap();
        let mut expected = ts.clone();
        for t in &mut expected {
            let tool = t.function.name.clone();
            strip_redundant(t.function.parameters.as_mut().unwrap(), &tool);
        }
        prop_assert_eq!(decode_tools(&compact).unwrap(), expected, "{}", compact.definitions);
    }

    /// Encoding is a pure function.
    #[test]
    fn encoding_is_deterministic(ts in tools()) {
        prop_assert_eq!(encode_tools(&ts).unwrap(), encode_tools(&ts).unwrap());
    }

    /// Any valid call renders and decodes back to the same tool and arguments.
    #[test]
    fn valid_calls_round_trip((ts, name, args) in tools_and_call(), pre in "[a-z .<>{}]{0,12}", post in "[a-z .]{0,12}") {
        let text = format!("{pre}{}{post}", render_call(&name, &args));
        // Text that itself opens a call (`<<call`, or `<<` + a tool name) is out of scope here.
        prop_assume!(!pre.contains("<<"));
        let out = decode(&text, &ts).unwrap();
        prop_assert_eq!(out.calls.len(), 1);
        prop_assert_eq!(&out.calls[0].name, &name);
        prop_assert_eq!(out.calls[0].arguments_value().unwrap(), args);
        prop_assert_eq!(out.text, format!("{pre}{post}"));
    }

    /// Streaming with arbitrary chunk boundaries equals one-shot decoding.
    #[test]
    fn chunking_never_changes_the_result((ts, name, args) in tools_and_call(), cuts in prop::collection::vec(any::<usize>(), 0..6)) {
        let text = format!("lead <<{}\n{} tail <", render_call(&name, &args), render_call(&name, &args));
        let whole = decode(&text, &ts).unwrap();
        let pts = split_points(&text, &cuts);
        let chunks: Vec<&str> = pts.windows(2).map(|w| &text[w[0]..w[1]]).collect();
        prop_assert_eq!(stream(&chunks, &ts).unwrap(), whole);
    }

    /// Dropping a required field is always caught.
    #[test]
    fn a_missing_required_field_is_an_error((ts, name, args) in tools_and_call()) {
        let schema = ts.iter().find(|t| t.function.name == name).unwrap().function.parameters.clone().unwrap();
        let required = schema.get("required").and_then(Value::as_array).cloned().unwrap_or_default();
        prop_assume!(!required.is_empty());
        let mut args = args;
        args.as_object_mut().unwrap().remove(required[0].as_str().unwrap());
        let e = decode_calls(&render_call(&name, &args), &ts).unwrap_err();
        prop_assert_eq!(e.code(), "invalid_arguments");
    }

    /// An undeclared field is always caught.
    #[test]
    fn an_undeclared_field_is_an_error((ts, name, mut args) in tools_and_call()) {
        args.as_object_mut().unwrap().insert("zz_not_declared".into(), json!(1));
        let e = decode_calls(&render_call(&name, &args), &ts).unwrap_err();
        prop_assert_eq!(e.code(), "invalid_arguments");
    }

    /// Arbitrary output never panics, and every call that comes back is to an offered tool
    /// with arguments that parse.
    #[test]
    fn garbage_never_yields_an_unvalidated_call(ts in tools(), text in "(<<call |[a-z_]{1,6}|[ {}\\[\\]\":,>]|<<|>>|1|true){0,40}") {
        if let Ok(calls) = decode_calls(&text, &ts) {
            for ToolCall { name, arguments } in calls {
                prop_assert!(ts.iter().any(|t| t.function.name == name));
                prop_assert!(serde_json::from_str::<Value>(&arguments).unwrap().is_object());
            }
        }
    }
}
