//! The supported/unsupported matrix: every declined keyword or shape fails the whole catalog with
//! `unsupported_schema` and a path, and nothing is silently dropped.

mod common;

use common::tool;
use nasiko_tool_compact::{ToolCompactError, encode_tools, limits};
use serde_json::{Map, Value, json};

fn prop(schema: Value) -> Value {
    json!({"type": "object", "properties": {"a": schema}})
}

#[test]
fn unsupported_keywords_are_declined_with_their_path() {
    let cases: Vec<(Value, &str)> = vec![
        (prop(json!({"$ref": "#/$defs/x"})), "/properties/a"),
        (json!({"type": "object", "$defs": {"x": {}}}), ""),
        (
            json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object"}),
            "",
        ),
        (
            prop(json!({"allOf": [{"type": "string"}]})),
            "/properties/a",
        ),
        (
            prop(json!({"anyOf": [{"type": "string"}, {"type": "null"}]})),
            "/properties/a",
        ),
        (
            prop(json!({"oneOf": [{"type": "string"}]})),
            "/properties/a",
        ),
        (prop(json!({"not": {"type": "string"}})), "/properties/a"),
        (prop(json!({"type": "string", "if": {}})), "/properties/a"),
        (prop(json!({"const": "x"})), "/properties/a"),
        (
            prop(json!({"type": "string", "pattern": "^a"})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "object", "patternProperties": {}})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "object", "propertyNames": {}})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "number", "multipleOf": 2})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "array", "uniqueItems": true})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "object", "dependencies": {}})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "string", "deprecated": true})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "string", "readOnly": true})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "array", "contains": {}})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "array", "items": [{"type": "string"}]})),
            "/properties/a/items",
        ),
        (
            prop(json!({"type": "object", "additionalProperties": {"type": "string"}})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "string", "enum": ["x"], "minLength": 1})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "string", "enum": ["x"], "format": "email"})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "number", "enum": [1.5]})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "boolean", "enum": [true]})),
            "/properties/a",
        ),
        (prop(json!({"type": "string", "enum": []})), "/properties/a"),
        (
            prop(json!({"type": "string", "enum": [null]})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "string", "enum": ["a", 1]})),
            "/properties/a",
        ),
        (prop(json!({"minimum": 1})), "/properties/a"),
        (
            prop(json!({"type": ["string", "integer"]})),
            "/properties/a",
        ),
        (prop(json!({"type": ["null", "string"]})), "/properties/a"),
        (prop(json!({"type": ["null", "null"]})), "/properties/a"),
        (prop(json!({"type": "date"})), "/properties/a"),
        (
            prop(json!({"type": "string", "description": 5})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "string", "minLength": -1})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "string", "minLength": 1.5})),
            "/properties/a",
        ),
        (
            prop(json!({"type": "integer", "minimum": "1"})),
            "/properties/a",
        ),
        (prop(json!("not a schema")), "/properties/a"),
        (
            json!({"type": "object", "properties": {"has space": {"type": "string"}}}),
            "",
        ),
        (
            json!({"type": "object", "properties": {"1digit": {"type": "string"}}}),
            "",
        ),
        (
            json!({"type": "object", "properties": {"ünïcode": {"type": "string"}}}),
            "",
        ),
        (
            json!({"type": "object", "properties": {"$dollar": {"type": "string"}}}),
            "",
        ),
        (
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["ghost"]}),
            "",
        ),
        (
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a", "a"]}),
            "",
        ),
        (json!({"type": "object", "required": "a"}), ""),
        (json!({"type": "object", "properties": []}), ""),
        (json!({"type": "string"}), ""),
        (json!({"type": ["object", "null"]}), ""),
        (json!([]), ""),
    ];
    for (schema, path) in cases {
        match encode_tools(&[tool("t", schema.clone())]) {
            Err(ToolCompactError::UnsupportedSchema {
                tool: t, path: p, ..
            }) => {
                assert_eq!(t, "t");
                assert_eq!(p, path, "path for {schema}");
            }
            other => panic!("{schema} should be unsupported, got {other:?}"),
        }
    }
}

#[test]
fn a_nullable_enum_without_a_null_member_is_declined_not_approximated() {
    // JSON Schema: `null` is NOT valid here (enum membership wins). The notation cannot carry
    // "nullable type but null not allowed" faithfully, so the schema is declined outright. The
    // oracle test pins that no accepted encoding ever admits null for this shape.
    let schema = prop(json!({"type": ["string", "null"], "enum": ["public", "private"]}));
    assert!(matches!(
        encode_tools(&[tool("t", schema)]),
        Err(ToolCompactError::UnsupportedSchema { .. })
    ));
    // The faithful variant, with null listed, is supported.
    let ok = prop(json!({"type": ["string", "null"], "enum": ["public", "private", null]}));
    assert!(encode_tools(&[tool("t", ok)]).is_ok());
}

#[test]
fn bypass_is_all_or_nothing() {
    let good = tool("good", json!({"type": "object"}));
    let bad = tool("bad", prop(json!({"type": "string", "pattern": "x"})));
    let err = encode_tools(&[good, bad]).unwrap_err();
    assert_eq!(err.kind(), "unsupported_schema");
    assert!(err.to_string().contains("tool 'bad'"));
}

#[test]
fn schema_depth_limit_is_exact() {
    fn nested(depth: usize) -> Value {
        let mut v = json!({"type": "string"});
        for _ in 0..depth {
            v = json!({"type": "array", "items": v});
        }
        json!({"type": "object", "properties": {"a": v}})
    }
    // Root is depth 0, `a` is 1, each array level adds one.
    assert!(encode_tools(&[tool("t", nested(limits::MAX_SCHEMA_DEPTH - 1))]).is_ok());
    assert!(matches!(
        encode_tools(&[tool("t", nested(limits::MAX_SCHEMA_DEPTH))]),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_SCHEMA_DEPTH",
            ..
        })
    ));
}

#[test]
fn schema_node_limit_is_exact_across_the_catalog() {
    fn wide(props: usize) -> Value {
        let mut m = Map::new();
        for i in 0..props {
            m.insert(format!("p{i}"), json!({"type": "string"}));
        }
        json!({"type": "object", "properties": m})
    }
    // 16 tools × (1 root + 255 properties) = 4096 nodes exactly.
    let per_tool = limits::MAX_PROPERTIES - 1;
    let count = limits::MAX_SCHEMA_NODES / (per_tool + 1);
    assert_eq!(count * (per_tool + 1), limits::MAX_SCHEMA_NODES);
    let mut tools: Vec<_> = (0..count)
        .map(|i| tool(&format!("t{i}"), wide(per_tool)))
        .collect();
    assert!(encode_tools(&tools).is_ok());
    tools.push(tool("one_more", json!({"type": "object"})));
    assert!(matches!(
        encode_tools(&tools),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_SCHEMA_NODES",
            ..
        })
    ));
}

#[test]
fn property_enum_and_description_limits_are_exact() {
    let mut m = Map::new();
    for i in 0..limits::MAX_PROPERTIES {
        m.insert(format!("p{i}"), json!({"type": "string"}));
    }
    assert!(
        encode_tools(&[tool(
            "t",
            json!({"type": "object", "properties": m.clone()})
        )])
        .is_ok()
    );
    m.insert("one_more".into(), json!({"type": "string"}));
    assert!(matches!(
        encode_tools(&[tool("t", json!({"type": "object", "properties": m}))]),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_PROPERTIES",
            ..
        })
    ));

    let members: Vec<Value> = (0..limits::MAX_ENUM_MEMBERS).map(|i| json!(i)).collect();
    assert!(encode_tools(&[tool("t", prop(json!({"type": "integer", "enum": members})))]).is_ok());
    let members: Vec<Value> = (0..=limits::MAX_ENUM_MEMBERS).map(|i| json!(i)).collect();
    assert!(matches!(
        encode_tools(&[tool("t", prop(json!({"type": "integer", "enum": members})))]),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_ENUM_MEMBERS",
            ..
        })
    ));

    let ok = "d".repeat(limits::MAX_DESCRIPTION_BYTES);
    assert!(
        encode_tools(&[tool(
            "t",
            prop(json!({"type": "string", "description": ok}))
        )])
        .is_ok()
    );
    let long = "d".repeat(limits::MAX_DESCRIPTION_BYTES + 1);
    assert!(matches!(
        encode_tools(&[tool(
            "t",
            prop(json!({"type": "string", "description": long}))
        )]),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_DESCRIPTION_BYTES",
            ..
        })
    ));
    let long_default = "d".repeat(limits::MAX_DESCRIPTION_BYTES);
    assert!(matches!(
        encode_tools(&[tool(
            "t",
            prop(json!({"type": "string", "default": long_default}))
        )]),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_DESCRIPTION_BYTES",
            ..
        })
    ));
}
