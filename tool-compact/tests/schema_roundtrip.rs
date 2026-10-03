//! Schema meaning survives encoding: `decode_tools(encode_tools(T)) == T` on the supported
//! subset, for the public sample tools and for randomly generated schemas.

use nasiko_tool_compact::{CompactTools, ToolDef, decode_tools, encode_tools};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

fn roundtrip(tool: &ToolDef) -> Vec<ToolDef> {
    let encoded = encode_tools(std::slice::from_ref(tool)).expect("encode");
    assert!(encoded.bypassed.is_empty(), "{:?}", encoded.bypassed);
    decode_tools(&encoded).expect("decode_tools")
}

#[test]
fn sample_tools_round_trip() {
    let calendar = ToolDef {
        name: "create_calendar_event".into(),
        description: Some("Create an event in the user's calendar.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        })),
    };
    let email = ToolDef {
        name: "send_email".into(),
        description: Some("Send an email from the user's account.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                "subject": {"type": "string", "description": "Subject line"},
                "body": {"type": "string", "description": "Plain-text body"},
                "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
            },
            "required": ["to", "subject", "body"]
        })),
    };
    for tool in [calendar, email] {
        assert_eq!(roundtrip(&tool), vec![tool]);
    }
}

#[test]
fn decode_tools_rejects_text_it_did_not_produce() {
    for text in [
        "Tools:\nnot a signature",
        "whatever",
        "Tools:\n  note: first",
    ] {
        let bad = CompactTools {
            text: text.into(),
            bypassed: vec![],
        };
        assert!(decode_tools(&bad).is_err(), "{text}");
    }
}

#[test]
fn bypassed_tools_are_not_returned_by_decode_tools() {
    let good = ToolDef {
        name: "good".into(),
        description: None,
        parameters: None,
    };
    let bad = ToolDef {
        name: "bad".into(),
        description: None,
        parameters: Some(json!({"$ref": "#/x"})),
    };
    let encoded = encode_tools(&[good, bad]).unwrap();
    assert_eq!(encoded.bypassed.len(), 1);
    let names: Vec<String> = decode_tools(&encoded)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, vec!["good"]);
}

// ---- generated schemas -------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Node {
    Str,
    DateTime,
    Int,
    Num,
    Bool,
    Enum(Vec<&'static str>),
    Array(Box<Node>),
    Object(Vec<Field>),
}

#[derive(Debug, Clone)]
struct Field {
    node: Node,
    required: bool,
    description: Option<String>,
}

const ENUM_POOL: [&str; 7] = ["public", "private", "a", "b-c", "x.y", "read_only", "Z9"];

fn node() -> impl Strategy<Value = Node> {
    let leaf = prop_oneof![
        Just(Node::Str),
        Just(Node::DateTime),
        Just(Node::Int),
        Just(Node::Num),
        Just(Node::Bool),
        proptest::sample::subsequence(ENUM_POOL.to_vec(), 1..=4).prop_map(Node::Enum),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            inner.clone().prop_map(|n| Node::Array(Box::new(n))),
            proptest::collection::vec(field(inner), 1..=4).prop_map(Node::Object),
        ]
    })
}

fn field(inner: impl Strategy<Value = Node>) -> impl Strategy<Value = Field> {
    (
        inner,
        any::<bool>(),
        proptest::option::of("[A-Za-z0-9][A-Za-z0-9 ,.:()'-]{0,18}[A-Za-z0-9]"),
    )
        .prop_map(|(node, required, description)| Field {
            node,
            required,
            description,
        })
}

fn to_value(node: &Node) -> Value {
    match node {
        Node::Str => json!({"type": "string"}),
        Node::DateTime => json!({"type": "string", "format": "date-time"}),
        Node::Int => json!({"type": "integer"}),
        Node::Num => json!({"type": "number"}),
        Node::Bool => json!({"type": "boolean"}),
        Node::Enum(values) => json!({"type": "string", "enum": values}),
        Node::Array(inner) => json!({"type": "array", "items": to_value(inner)}),
        Node::Object(fields) => object_value(fields),
    }
}

fn object_value(fields: &[Field]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let name = format!("p{i}");
        let mut value = to_value(&f.node);
        if let Some(d) = &f.description {
            value["description"] = json!(d);
        }
        if f.required {
            required.push(json!(name));
        }
        properties.insert(name, value);
    }
    let mut schema = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        schema["required"] = Value::Array(required);
    }
    schema
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn generated_schemas_round_trip(
        fields in proptest::collection::vec(field(node()), 1..=5),
        description in proptest::option::of("[A-Za-z0-9][A-Za-z0-9 ,.'-]{0,30}[A-Za-z0-9]"),
    ) {
        let tool = ToolDef {
            name: "tool_x".into(),
            description,
            parameters: Some(object_value(&fields)),
        };
        prop_assert_eq!(roundtrip(&tool), vec![tool]);
    }

    #[test]
    fn encoding_is_deterministic(fields in proptest::collection::vec(field(node()), 1..=5)) {
        let tool = ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(object_value(&fields)),
        };
        prop_assert_eq!(
            encode_tools(std::slice::from_ref(&tool)).unwrap(),
            encode_tools(std::slice::from_ref(&tool)).unwrap()
        );
    }
}
