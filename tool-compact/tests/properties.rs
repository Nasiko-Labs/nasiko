//! Property tests: invariants over generated schemas, arguments, partitions and mutations.
//! Nothing here depends on the public evaluation examples.

mod common;

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_reply, decode_tools,
    encode_tools, render_calls,
};
use proptest::prelude::*;
use proptest::sample::Index;
use serde_json::{Map, Value, json};

// ── generators ────────────────────────────────────────────────────────────────

fn key() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,7}"
}

fn description() -> impl Strategy<Value = String> {
    // Arbitrary text, control characters and grammar punctuation included.
    prop_oneof![
        any::<String>(),
        "[ -~]{0,30}",
        Just(" # = (x) | \\ \"".to_string())
    ]
}

fn enum_string() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z][a-z0-9_-]{0,6}",
        "[ -~]{0,8}",
        Just("string".to_string())
    ]
}

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<i32>().prop_map(Value::from),
        any::<bool>().prop_map(Value::from),
        enum_string().prop_map(Value::from),
        Just(Value::Null),
        (-1000i32..1000).prop_map(|n| json!(f64::from(n) / 8.0)),
    ]
}

/// A leaf schema the compact grammar supports.
fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"type": "string"})),
        prop_oneof![
            Just("date-time"),
            Just("date"),
            Just("time"),
            Just("email"),
            Just("uuid")
        ]
        .prop_map(|f| json!({"type": "string", "format": f})),
        (0u64..5, 5u64..50)
            .prop_map(|(a, b)| json!({"type": "string", "minLength": a, "maxLength": b})),
        Just(json!({"type": "string", "pattern": "^[a-z]+(\\d{2})?$"})),
        (-50i64..0, 0i64..50)
            .prop_map(|(a, b)| json!({"type": "integer", "minimum": a, "maximum": b})),
        Just(json!({"type": "number", "exclusiveMinimum": 0, "multipleOf": 0.25})),
        Just(json!({"type": "boolean"})),
        Just(json!({"type": ["string", "null"]})),
        Just(json!({})),
        prop::collection::vec(enum_string(), 1..5)
            .prop_map(|v| json!({"type": "string", "enum": v})),
        prop::collection::vec(scalar(), 1..4).prop_filter_map("not all null", |v| {
            (!v.iter().all(Value::is_null)).then(|| json!({ "enum": v }))
        }),
        enum_string().prop_map(|s| json!({ "const": s })),
    ]
}

/// A property schema, possibly nested, without descriptions on `items` (no place for them).
fn schema() -> BoxedStrategy<Value> {
    leaf()
        .prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                (inner.clone(), prop::option::of(1u64..4)).prop_map(|(items, min)| {
                    let mut s = json!({"type": "array", "items": strip_annotations(items)});
                    if let Some(min) = min {
                        s["minItems"] = json!(min);
                    }
                    s
                }),
                object(inner.clone()),
                Just(json!({"type": "object"})),
            ]
        })
        .boxed()
}

fn object(inner: impl Strategy<Value = Value> + Clone) -> impl Strategy<Value = Value> {
    (
        prop::collection::btree_map(key(), property(inner), 0..5),
        any::<Index>(),
        any::<bool>(),
    )
        .prop_map(|(props, pick, closed)| {
            let names: Vec<String> = props.keys().cloned().collect();
            let mut s = json!({"type": "object", "properties": props});
            if !names.is_empty() {
                let n = pick.index(names.len() + 1);
                if n > 0 {
                    s["required"] = json!(names[..n]);
                }
            }
            if closed {
                s["additionalProperties"] = json!(false);
            }
            s
        })
}

/// A schema in property position: may carry a description and a default.
fn property(inner: impl Strategy<Value = Value>) -> impl Strategy<Value = Value> {
    (
        inner,
        prop::option::of(description()),
        prop::option::of(scalar()),
    )
        .prop_map(|(mut s, desc, default)| {
            if let Some(d) = desc {
                s["description"] = json!(d);
            }
            if let Some(d) = default {
                s["default"] = d;
            }
            s
        })
}

fn strip_annotations(mut s: Value) -> Value {
    if let Some(m) = s.as_object_mut() {
        m.remove("description");
        m.remove("default");
    }
    s
}

fn root_params() -> impl Strategy<Value = Value> {
    object(schema())
}

/// Cut `text` at a few random character boundaries.
fn partition(text: &str, cuts: &[Index]) -> Vec<String> {
    let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let mut points: Vec<usize> = cuts
        .iter()
        .filter(|_| !bounds.is_empty())
        .map(|c| bounds[c.index(bounds.len())])
        .collect();
    points.sort_unstable();
    points.dedup();
    let mut parts = Vec::new();
    let mut last = 0;
    for p in points {
        parts.push(text[last..p].to_string());
        last = p;
    }
    parts.push(text[last..].to_string());
    parts
}

fn stream(parts: &[String], tools: &[ToolDef]) -> Result<(String, Vec<ToolCall>), CompactError> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut text = String::new();
    for part in parts {
        text.push_str(&decoder.push(part)?);
    }
    let done = decoder.finish()?;
    text.push_str(&done.text);
    Ok((text, done.calls))
}

fn as_args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

// ── properties ────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// I15: a supported schema survives encode → decode_tools exactly.
    #[test]
    fn generated_schemas_roundtrip_exactly(params in root_params(), desc in prop::option::of(description())) {
        let tool = ToolDef::new("gen_tool", desc, Some(params));
        let compact = encode_tools(std::slice::from_ref(&tool))
            .map_err(|e| TestCaseError::fail(format!("{e}")))?;
        let back = decode_tools(&compact)
            .map_err(|e| TestCaseError::fail(format!("{e}\n{}", compact.definitions())))?;
        prop_assert_eq!(back.len(), 1);
        prop_assert_eq!(&back[0].description, &tool.description);
        prop_assert_eq!(&back[0].parameters, &tool.parameters, "definitions:\n{}", compact.definitions());
    }

    /// Whatever the encoder accepts, the parser reads back, and re-encoding is a fixpoint.
    #[test]
    fn encoder_output_always_parses_and_reencodes_identically(params in any_json(3)) {
        let tool = ToolDef::new("any_tool", None, Some(params));
        if let Ok(compact) = encode_tools(std::slice::from_ref(&tool)) {
            let back = decode_tools(&compact)
                .map_err(|e| TestCaseError::fail(format!("{e}\n{}", compact.definitions())))?;
            let again = encode_tools(&back).map_err(|e| TestCaseError::fail(format!("{e}")))?;
            prop_assert_eq!(again, compact);
        }
    }

    /// I9 + I12: arbitrary argument text — markers, quotes, backslashes, unicode — rendered as a
    /// call decodes back exactly, however the reply is chunked.
    #[test]
    fn rendered_calls_survive_random_partitions(
        subject in any::<String>(),
        body in any::<String>(),
        to in prop::collection::vec(any::<String>(), 1..3),
        prefix in "[ -~]{0,10}",
        cuts in prop::collection::vec(any::<Index>(), 0..8),
    ) {
        let call = ToolCall {
            name: "send_email".into(),
            arguments: as_args(json!({"to": to, "subject": subject, "body": body})),
        };
        // Prose must not itself open a call.
        let prefix = prefix.replace('<', "");
        let text = format!("{prefix}{}", render_calls(std::slice::from_ref(&call)));
        let tools = common::tools();
        let (prose, calls) = stream(&partition(&text, &cuts), &tools)
            .map_err(|e| TestCaseError::fail(format!("{e}: {text:?}")))?;
        prop_assert_eq!(calls, vec![call]);
        prop_assert_eq!(prose, prefix);
    }

    /// I6: a value outside an enum is never accepted, whatever it looks like.
    #[test]
    fn values_outside_an_enum_are_always_rejected(value in any::<String>()) {
        prop_assume!(value != "public" && value != "private");
        let text = render_calls(&[ToolCall {
            name: "create_calendar_event".into(),
            arguments: as_args(json!({"title": "t", "start": "2026-10-05T15:00:00+05:30", "visibility": value})),
        }]);
        prop_assert_eq!(decode_calls(&text, &common::tools()).unwrap_err().code(), "invalid_arguments");
    }

    /// I5: a non-integer is never accepted for an integer argument.
    #[test]
    fn non_integers_are_always_rejected_for_integer_arguments(value in prop_oneof![
        any::<String>().prop_map(Value::from),
        any::<bool>().prop_map(Value::from),
        (1i32..1000).prop_map(|n| json!(f64::from(n) + 0.5)),
        Just(json!([1])),
        Just(json!({"n": 1})),
    ]) {
        let text = render_calls(&[ToolCall {
            name: "create_calendar_event".into(),
            arguments: as_args(json!({"title": "t", "start": "2026-10-05T15:00:00+05:30", "duration_min": value})),
        }]);
        prop_assert_eq!(decode_calls(&text, &common::tools()).unwrap_err().code(), "invalid_arguments");
    }

    /// I9 + I13: a corrupted reply decodes identically in one chunk or many, never panics, and
    /// anything it does accept is a fixpoint of render → decode (no invented values).
    #[test]
    fn mutated_replies_are_chunking_invariant_and_never_invent_values(
        edits in prop::collection::vec((any::<Index>(), prop::sample::select(vec!['{', '}', '"', '\\', '<', '>', ',', ':', ' ', 'x', '1'])), 1..4),
        delete in any::<bool>(),
        cuts in prop::collection::vec(any::<Index>(), 0..6),
    ) {
        let mut text: Vec<char> = r#"<<call tracker.create_ticket {"title":"t","priority":"high","labels":["a"]}>> ok"#.chars().collect();
        for (at, c) in &edits {
            let i = at.index(text.len());
            if delete { text.remove(i); } else { text.insert(i, *c); }
        }
        let text: String = text.into_iter().collect();
        let tools = common::tools();
        let whole = decode_reply(&text, &tools).map(|d| (d.text, d.calls));
        let parts = stream(&partition(&text, &cuts), &tools);
        prop_assert_eq!(&parts, &whole);
        if let Ok((_, calls)) = whole {
            let again = decode_calls(&render_calls(&calls), &tools)
                .map_err(|e| TestCaseError::fail(format!("{e}")))?;
            prop_assert_eq!(again, calls);
        }
    }
}

/// Arbitrary JSON, for the "never panics, always parses back" property.
fn any_json(depth: u32) -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "[ -~]{0,6}".prop_map(Value::from),
        prop_oneof![
            Just("type"),
            Just("string"),
            Just("object"),
            Just("properties"),
            Just("enum"),
            Just("required")
        ]
        .prop_map(Value::from),
    ];
    leaf.prop_recursive(depth, 32, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(
                prop_oneof![
                    Just("type".to_string()),
                    Just("properties".to_string()),
                    Just("items".to_string()),
                    Just("enum".to_string()),
                    Just("required".to_string()),
                    Just("description".to_string()),
                    key()
                ],
                inner,
                0..4
            )
            .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}
