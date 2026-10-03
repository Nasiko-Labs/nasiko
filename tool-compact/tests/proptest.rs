//! Property tests over random schemas in the supported subset, with arbitrary descriptions and
//! argument strings (quotes, backslashes, newlines, `>>`, non-ASCII).

use std::collections::BTreeMap;

use nasiko_tool_compact::{
    BypassReason, MAX_DEPTH, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools,
    encode_tools, render_call,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
enum T {
    Str(Option<String>),
    Int,
    Num,
    Bool,
    DateTime,
    Date,
    Enum(Vec<String>),
    Arr(Box<N>),
    Obj(BTreeMap<String, (bool, N)>, bool),
}

#[derive(Debug, Clone)]
struct N {
    t: T,
    /// Extra limit keywords merged into the schema (min/max/length/items).
    cons: Map<String, Value>,
    desc: Option<String>,
}

fn name() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_-]{0,7}"
}

fn short_num() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<i64>().prop_map(|i| json!(i)),
        (-1_000_000i64..1_000_000).prop_map(|i| json!(i as f64 / 100.0)),
    ]
}

/// Random limit keywords that fit type `t`; empty unless `with_cons` (call values are not
/// generated to satisfy limits, so the call round-trip runs without them).
fn cons_for(t: &T, with_cons: bool) -> BoxedStrategy<Map<String, Value>> {
    if !with_cons {
        return Just(Map::new()).boxed();
    }
    let opt = |k: &'static str, s: BoxedStrategy<Value>| {
        prop::option::of(s).prop_map(move |v| v.map(|v| (k.to_string(), v)))
    };
    let count = || (0u64..50).prop_map(|n| json!(n)).boxed();
    let parts: Vec<BoxedStrategy<Option<(String, Value)>>> = match t {
        T::Str(_) | T::DateTime | T::Date => vec![
            opt("minLength", count()).boxed(),
            opt("maxLength", count()).boxed(),
        ],
        T::Int | T::Num => vec![
            opt("minimum", short_num().boxed()).boxed(),
            opt("maximum", short_num().boxed()).boxed(),
            opt("exclusiveMinimum", short_num().boxed()).boxed(),
            opt("exclusiveMaximum", short_num().boxed()).boxed(),
        ],
        T::Arr(_) => vec![
            opt("minItems", count()).boxed(),
            opt("maxItems", count()).boxed(),
            opt("uniqueItems", Just(json!(true)).boxed()).boxed(),
        ],
        _ => vec![],
    };
    parts
        .prop_map(|kv| kv.into_iter().flatten().collect())
        .boxed()
}

fn node(with_cons: bool) -> impl Strategy<Value = N> {
    let leaf = prop_oneof![
        prop::option::of("[a-z][a-z0-9-]{0,6}").prop_map(T::Str),
        Just(T::Int),
        Just(T::Num),
        Just(T::Bool),
        Just(T::DateTime),
        Just(T::Date),
        // Every character class the enum grammar allows, including non-ASCII alphanumerics.
        prop::collection::btree_set("[a-zA-Z0-9_.:/+@é日٣-]{1,6}", 1..4)
            .prop_map(|s| T::Enum(s.into_iter().collect())),
    ];
    let with_extras = move |t: T| {
        let cons = cons_for(&t, with_cons);
        (Just(t), cons, prop::option::of(any::<String>())).prop_map(|(t, cons, desc)| N {
            t,
            cons,
            desc,
        })
    };
    leaf.prop_flat_map(with_extras)
        // Top-level fields are depth 1, so MAX_DEPTH - 1 levels of recursion reach MAX_DEPTH.
        .prop_recursive((MAX_DEPTH - 1) as u32, 32, 2, move |inner| {
            prop_oneof![
                inner.clone().prop_map(|n| T::Arr(Box::new(n))),
                (
                    prop::collection::btree_map(name(), (any::<bool>(), inner), 0..4),
                    any::<bool>()
                )
                    .prop_map(|(f, closed)| T::Obj(f, closed)),
            ]
            .prop_flat_map(with_extras)
        })
}

fn object_schema(fields: &BTreeMap<String, (bool, N)>, closed: bool) -> Map<String, Value> {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (k, (req, n)) in fields {
        props.insert(k.clone(), schema(n));
        if *req {
            required.push(json!(k));
        }
    }
    let mut obj = Map::new();
    obj.insert("type".into(), json!("object"));
    obj.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        obj.insert("required".into(), Value::Array(required));
    }
    if closed {
        obj.insert("additionalProperties".into(), json!(false));
    }
    obj
}

fn schema(n: &N) -> Value {
    let mut obj = match &n.t {
        T::Str(None) => json!({"type":"string"}),
        T::Str(Some(f)) => json!({"type":"string","format":f}),
        T::Int => json!({"type":"integer"}),
        T::Num => json!({"type":"number"}),
        T::Bool => json!({"type":"boolean"}),
        T::DateTime => json!({"type":"string","format":"date-time"}),
        T::Date => json!({"type":"string","format":"date"}),
        T::Enum(v) => json!({"type":"string","enum":v}),
        T::Arr(inner) => json!({"type":"array","items":schema(inner)}),
        T::Obj(fields, closed) => Value::Object(object_schema(fields, *closed)),
    };
    if let Some(o) = obj.as_object_mut() {
        o.extend(n.cons.clone());
    }
    if let (Some(d), Some(o)) = (&n.desc, obj.as_object_mut()) {
        o.insert("description".into(), json!(d));
    }
    obj
}

fn value(n: &N) -> BoxedStrategy<Value> {
    match &n.t {
        T::Str(_) => any::<String>().prop_map(Value::String).boxed(),
        T::Int => any::<i64>().prop_map(|i| json!(i)).boxed(),
        T::Num => prop_oneof![
            any::<i64>().prop_map(|i| json!(i)),
            // Short decimals: serde_json's default (non-`float_roundtrip`) parser can be off by an
            // ulp on 17-digit floats, which is a serde_json property, not this crate's.
            (-1_000_000_000i64..1_000_000_000).prop_map(|i| json!(i as f64 / 1000.0)),
        ]
        .boxed(),
        T::Bool => any::<bool>().prop_map(Value::Bool).boxed(),
        T::DateTime => (
            2000u32..2100,
            1u32..13,
            1u32..29,
            0u32..24,
            0u32..60,
            0u32..60,
        )
            .prop_map(|(y, mo, d, h, mi, s)| {
                json!(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}+05:30"))
            })
            .boxed(),
        T::Date => (2000u32..2100, 1u32..13, 1u32..29)
            .prop_map(|(y, mo, d)| json!(format!("{y:04}-{mo:02}-{d:02}")))
            .boxed(),
        T::Enum(v) => prop::sample::select(v.clone())
            .prop_map(Value::String)
            .boxed(),
        T::Arr(inner) => prop::collection::vec(value(inner), 0..3)
            .prop_map(Value::Array)
            .boxed(),
        T::Obj(fields, _) => object_value(fields).prop_map(Value::Object).boxed(),
    }
}

fn object_value(fields: &BTreeMap<String, (bool, N)>) -> BoxedStrategy<Map<String, Value>> {
    let parts: Vec<BoxedStrategy<Option<(String, Value)>>> = fields
        .iter()
        .map(|(k, (req, n))| {
            let k = k.clone();
            if *req {
                value(n).prop_map(move |v| Some((k.clone(), v))).boxed()
            } else {
                prop::option::of(value(n))
                    .prop_map(move |v| v.map(|v| (k.clone(), v)))
                    .boxed()
            }
        })
        .collect();
    parts
        .prop_map(|kvs| kvs.into_iter().flatten().collect())
        .boxed()
}

type Fields = BTreeMap<String, (bool, N)>;

#[derive(Debug, Clone)]
enum Corruption {
    WrongType(String, Value),
    DropRequired(String),
    BadEnum(String),
}

/// A value of the wrong JSON type for `t`.
fn wrong_type(t: &T) -> Value {
    match t {
        T::Str(_) | T::Enum(_) | T::DateTime | T::Date => json!(12345),
        T::Int | T::Num | T::Bool | T::Arr(_) | T::Obj(..) => json!("x"),
    }
}

fn tool(with_cons: bool) -> impl Strategy<Value = (ToolDef, Fields)> {
    (
        "[a-z][a-z0-9_]{0,10}",
        prop::option::of(any::<String>()),
        prop::collection::btree_map(name(), (any::<bool>(), node(with_cons)), 0..5),
        any::<bool>(),
    )
        .prop_map(|(name, description, fields, closed)| {
            let t = ToolDef {
                name,
                description,
                parameters: Some(Value::Object(object_schema(&fields, closed))),
            };
            (t, fields)
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn encode_then_decode_tools_is_identity(
        tools in prop::collection::btree_map("[a-z][a-z0-9_]{0,10}", tool(true), 1..4)
    ) {
        // Re-key so tool names are unique.
        let tools: Vec<ToolDef> = tools
            .into_iter()
            .map(|(name, (mut t, _))| { t.name = name; t })
            .collect();
        let compact = encode_tools(&tools).unwrap();
        // Every generated schema is supported; the only allowed bypass is the size guard, which
        // fires for tiny tool lists where the fixed instructions outweigh the native JSON.
        prop_assert!(
            matches!(compact.bypass_reason(), None | Some(BypassReason::NotSmaller { .. })),
            "bypassed: {:?}", compact.bypass_reason()
        );
        prop_assert_eq!(decode_tools(&compact).unwrap(), tools);
    }

    #[test]
    fn render_then_decode_calls_is_identity(
        (t, args, before, after, split) in tool(false).prop_flat_map(|(t, fields)| {
            (
                Just(t),
                object_value(&fields),
                any::<String>(),
                any::<String>(),
                any::<prop::sample::Index>(),
            )
        })
    ) {
        // Prose must not itself contain a marker, or fuse with ours.
        prop_assume!(!before.contains("<<") && !after.contains("<<") && !before.ends_with('<'));
        let call = ToolCall { name: t.name.clone(), arguments: args };
        let text = format!("{before}{}{after}", render_call(&call));
        let tools = vec![t];
        let decoded = decode_calls(&text, &tools).unwrap();
        prop_assert_eq!(&decoded.calls, &vec![call.clone()]);
        prop_assert_eq!(&decoded.text, &format!("{before}{after}"));

        let chars: Vec<char> = text.chars().collect();
        let at = split.index(chars.len() + 1);
        let mut d = StreamDecoder::new(&tools);
        d.push(&chars.iter().take(at).collect::<String>()).unwrap();
        d.push(&chars.iter().skip(at).collect::<String>()).unwrap();
        prop_assert_eq!(d.finish().unwrap(), decoded);
    }

    #[test]
    fn corrupted_arguments_are_always_rejected(
        (t, fields, args, pick) in tool(false).prop_flat_map(|(t, fields)| {
            let args = object_value(&fields);
            (Just(t), Just(fields), args, any::<prop::sample::Index>())
        })
    ) {
        let options: Vec<Corruption> = fields
            .iter()
            .flat_map(|(k, (req, n))| {
                let mut v = vec![Corruption::WrongType(k.clone(), wrong_type(&n.t))];
                if *req {
                    v.push(Corruption::DropRequired(k.clone()));
                }
                if matches!(n.t, T::Enum(_)) {
                    v.push(Corruption::BadEnum(k.clone()));
                }
                v
            })
            .collect();
        prop_assume!(!options.is_empty());
        let mut args = args;
        match &options[pick.index(options.len())] {
            Corruption::WrongType(k, v) => { args.insert(k.clone(), v.clone()); }
            Corruption::DropRequired(k) => { args.remove(k); }
            // '!' is not an enum character, so this can never be a member.
            Corruption::BadEnum(k) => { args.insert(k.clone(), json!("!not-in-enum!")); }
        }
        let text = render_call(&ToolCall { name: t.name.clone(), arguments: args });
        let err = decode_calls(&text, &[t]).unwrap_err();
        prop_assert_eq!(err.code(), "invalid_arguments");
    }
}
