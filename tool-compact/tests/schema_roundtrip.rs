use nasiko_tool_compact::{CompactError, ToolDef, decode_tools, encode_tools, is_schema_supported};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::collections::HashSet;

fn assert_schemas_equivalent(actual: &Option<Value>, expected: &Option<Value>) {
    match (actual, expected) {
        (None, None) => {}
        (Some(a), Some(e)) => assert_value_equivalent(a, e),
        _ => panic!("Schema mismatch: actual={actual:?}, expected={expected:?}"),
    }
}

fn assert_value_equivalent(a: &Value, b: &Value) {
    match (a, b) {
        (Value::Object(map_a), Value::Object(map_b)) => {
            let keys_a: HashSet<&String> = map_a.keys().collect();
            let keys_b: HashSet<&String> = map_b.keys().collect();
            assert_eq!(
                keys_a, keys_b,
                "Object keys do not match: {map_a:?} vs {map_b:?}"
            );

            for (k, val_a) in map_a {
                let val_b = &map_b[k];
                if k == "required" {
                    let req_a: HashSet<String> = val_a
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    let req_b: HashSet<String> = val_b
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    assert_eq!(req_a, req_b, "Required fields do not match");
                } else {
                    assert_value_equivalent(val_a, val_b);
                }
            }
        }
        (Value::Array(arr_a), Value::Array(arr_b)) => {
            assert_eq!(arr_a.len(), arr_b.len(), "Array lengths differ");
            for (item_a, item_b) in arr_a.iter().zip(arr_b.iter()) {
                assert_value_equivalent(item_a, item_b);
            }
        }
        _ => assert_eq!(a, b, "Values differ"),
    }
}

#[test]
fn test_single_tool_roundtrip() {
    let tools = vec![ToolDef {
        name: "get_weather".to_string(),
        description: Some("Fetch current weather for a city.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" },
                "units": { "type": "string", "enum": ["metric", "imperial"] }
            },
            "required": ["city"]
        })),
    }];

    let compact = encode_tools(&tools).expect("encode failed");
    let decoded = decode_tools(&compact).expect("decode failed");

    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].name, "get_weather");
    assert_eq!(decoded[0].description, tools[0].description);
    assert_schemas_equivalent(&decoded[0].parameters, &tools[0].parameters);
}

#[test]
fn test_primitives_roundtrip() {
    let tools = vec![ToolDef {
        name: "test_primitives".to_string(),
        description: Some("Test all primitive types.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "a_str": { "type": "string" },
                "a_int": { "type": "integer" },
                "a_num": { "type": "number" },
                "a_bool": { "type": "boolean" },
                "a_dt": { "type": "string", "format": "date-time" }
            },
            "required": ["a_str", "a_int", "a_num", "a_bool", "a_dt"]
        })),
    }];

    let compact = encode_tools(&tools).expect("encode failed");
    let decoded = decode_tools(&compact).expect("decode failed");

    assert_schemas_equivalent(&decoded[0].parameters, &tools[0].parameters);
}

#[test]
fn test_nested_objects_and_arrays() {
    let tools = vec![ToolDef {
        name: "complex_tool".to_string(),
        description: Some("Tool with nested structures.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string", "description": "City name" },
                        "zip": { "type": "integer" }
                    },
                    "required": ["city"]
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "matrix": {
                    "type": "array",
                    "items": {
                        "type": "array",
                        "items": { "type": "integer" }
                    }
                }
            },
            "required": ["address"]
        })),
    }];

    let compact = encode_tools(&tools).expect("encode failed");
    let decoded = decode_tools(&compact).expect("decode failed");

    assert_schemas_equivalent(&decoded[0].parameters, &tools[0].parameters);
}

#[test]
fn test_empty_tool_parameters() {
    let tools = vec![
        ToolDef {
            name: "ping".to_string(),
            description: Some("Health check.".to_string()),
            parameters: None,
        },
        ToolDef {
            name: "no_desc".to_string(),
            description: None,
            parameters: None,
        },
    ];

    let compact = encode_tools(&tools).expect("encode failed");
    let decoded = decode_tools(&compact).expect("decode failed");

    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].name, "ping");
    assert_eq!(decoded[0].description, Some("Health check.".to_string()));
    assert_eq!(decoded[1].name, "no_desc");
    assert_eq!(decoded[1].description, None);
}

#[test]
fn test_unsupported_keywords_trigger_bypass() {
    let unsupported_schemas = vec![
        json!({"type": "object", "$ref": "#/defs/Foo"}),
        json!({"type": "object", "oneOf": [{"type": "string"}]}),
        json!({"type": "object", "anyOf": [{"type": "string"}]}),
        json!({"type": "object", "allOf": [{"type": "string"}]}),
        json!({"type": "object", "properties": {"code": {"type": "string", "pattern": "^[A-Z]+$"}}}),
        json!({"type": "object", "properties": {"count": {"type": "integer", "minimum": 0}}}),
        json!({"type": "object", "properties": {"name": {"type": "string", "default": "unnamed"}}}),
        json!({"type": "object", "properties": {"role": {"type": "string", "const": "admin"}}}),
        json!({"type": "object", "properties": {"email": {"type": "string", "format": "email"}}}),
        json!({"type": "object", "properties": {"val": {"type": ["string", "null"]}}}),
    ];

    for schema in unsupported_schemas {
        assert!(is_schema_supported(&schema).is_err());

        let tool = ToolDef {
            name: "unsupported".to_string(),
            description: None,
            parameters: Some(schema),
        };

        let res = encode_tools(&[tool]);
        assert!(
            matches!(res, Err(CompactError::UnsupportedSchema { .. })),
            "Expected UnsupportedSchema error for unsupported keyword"
        );
    }
}

// ─── Property-Based Testing ───────────────────────────────────────────────────

fn arb_identifier() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,8}".prop_filter("not empty", |s| !s.is_empty())
}

fn arb_primitive_type() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!({"type": "string"})),
        Just(json!({"type": "string", "format": "date-time"})),
        Just(json!({"type": "integer"})),
        Just(json!({"type": "number"})),
        Just(json!({"type": "boolean"})),
        proptest::collection::vec("[a-z]{1,4}", 2..3).prop_map(|variants| {
            json!({
                "type": "string",
                "enum": variants
            })
        }),
    ]
}

fn arb_prop_schema(depth: u32) -> impl Strategy<Value = Value> {
    if depth == 0 {
        arb_primitive_type().boxed()
    } else {
        prop_oneof![
            arb_primitive_type(),
            arb_prop_schema(depth - 1).prop_map(|items| {
                json!({
                    "type": "array",
                    "items": items
                })
            }),
            proptest::collection::vec((arb_identifier(), arb_prop_schema(depth - 1)), 1..3)
                .prop_map(|props| {
                    let mut prop_map = serde_json::Map::new();
                    let mut required = Vec::new();
                    for (i, (name, schema)) in props.into_iter().enumerate() {
                        prop_map.insert(name.clone(), schema);
                        if i % 2 == 0 {
                            required.push(Value::String(name));
                        }
                    }
                    let mut obj = serde_json::Map::new();
                    obj.insert("type".to_string(), Value::String("object".to_string()));
                    obj.insert("properties".to_string(), Value::Object(prop_map));
                    if !required.is_empty() {
                        obj.insert("required".to_string(), Value::Array(required));
                    }
                    Value::Object(obj)
                }),
        ]
        .boxed()
    }
}

fn arb_tool_parameters() -> impl Strategy<Value = Value> {
    proptest::collection::vec((arb_identifier(), arb_prop_schema(1)), 1..4).prop_map(|props| {
        let mut prop_map = serde_json::Map::new();
        let mut required = Vec::new();
        for (i, (name, schema)) in props.into_iter().enumerate() {
            prop_map.insert(name.clone(), schema);
            if i % 2 == 0 {
                required.push(Value::String(name));
            }
        }
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert("properties".to_string(), Value::Object(prop_map));
        if !required.is_empty() {
            obj.insert("required".to_string(), Value::Array(required));
        }
        Value::Object(obj)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn test_property_schema_roundtrip(
        name in arb_identifier(),
        parameters in arb_tool_parameters(),
    ) {
        let tool = ToolDef {
            name,
            description: Some("Auto-generated tool".to_string()),
            parameters: Some(parameters),
        };

        let compact = encode_tools(std::slice::from_ref(&tool)).expect("encoding valid schema should succeed");
        let decoded = decode_tools(&compact).expect("decoding should succeed");

        prop_assert_eq!(decoded.len(), 1);
        prop_assert_eq!(&decoded[0].name, &tool.name);
        prop_assert_eq!(&decoded[0].description, &tool.description);
        assert_schemas_equivalent(&decoded[0].parameters, &tool.parameters);
    }
}
