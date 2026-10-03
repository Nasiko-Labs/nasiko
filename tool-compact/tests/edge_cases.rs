//! Edge cases from code review, pinned through the public API.

use std::time::{Duration, Instant};

use nasiko_tool_compact::{
    CompactError, ToolDef, UnsupportedFeature, decode_calls, decode_tools, encode_tools,
};
use serde_json::{Value, json};

fn tool(params: Value) -> ToolDef {
    ToolDef::new("t", Some("Test tool.".into()), Some(params))
}

fn one(schema: Value) -> ToolDef {
    tool(json!({"type": "object", "properties": {"v": schema}}))
}

fn accepts(tool: &ToolDef, args: &str) -> bool {
    decode_calls(&format!("<<call t {args}>>"), std::slice::from_ref(tool)).is_ok()
}

fn unsupported_feature(tool: ToolDef) -> UnsupportedFeature {
    match encode_tools(&[tool]) {
        Err(CompactError::Unsupported { feature, .. }) => feature,
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn required_on_a_free_form_object_is_bypassed_not_dropped() {
    assert_eq!(
        unsupported_feature(tool(json!({"type": "object", "required": ["a"]}))),
        UnsupportedFeature::InconsistentRequired
    );
}

#[test]
fn const_together_with_enum_is_bypassed_not_half_rendered() {
    assert_eq!(
        unsupported_feature(one(
            json!({"type": "string", "enum": ["a", "b"], "const": "a"})
        )),
        UnsupportedFeature::Keyword("const".into())
    );
}

#[test]
fn a_one_element_type_array_is_bypassed_because_it_would_read_back_as_a_string() {
    assert_eq!(
        unsupported_feature(one(json!({"type": ["string"]}))),
        UnsupportedFeature::Keyword("type".into())
    );
}

#[test]
fn empty_required_and_absent_parameters_read_back_exactly() {
    let tools = vec![
        tool(json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": []})),
        ToolDef::new("ping", None, None),
        ToolDef::new(
            "noop",
            None,
            Some(json!({"type": "object", "properties": {}})),
        ),
    ];
    let back = decode_tools(&encode_tools(&tools).unwrap()).unwrap();
    for (original, decoded) in tools.iter().zip(&back) {
        assert_eq!(decoded.parameters, original.parameters, "{}", original.name);
    }
}

#[test]
fn null_constants_read_back_exactly() {
    let t = tool(json!({"type": "object", "properties": {
        "a": {"const": null}, "b": {"type": "null", "const": null}
    }}));
    let back = decode_tools(&encode_tools(std::slice::from_ref(&t)).unwrap()).unwrap();
    assert_eq!(back[0].parameters, t.parameters);
}

#[test]
fn multiple_of_is_exact_for_large_integers() {
    let t = one(json!({"type": "integer", "multipleOf": 10}));
    assert!(!accepts(&t, r#"{"v":10000000005}"#));
    assert!(accepts(&t, r#"{"v":10000000000}"#));
    let unit = one(json!({"type": "number", "multipleOf": 1}));
    assert!(!accepts(&unit, r#"{"v":1000000000.5}"#));
    let tenth = one(json!({"type": "number", "multipleOf": 0.1}));
    assert!(accepts(&tenth, r#"{"v":0.3}"#));
}

#[test]
fn numbers_compare_by_value_in_enum_const_and_unique_items() {
    let e = one(json!({"enum": [1, 2]}));
    assert!(accepts(&e, r#"{"v":1.0}"#));
    let c = one(json!({"const": 1}));
    assert!(accepts(&c, r#"{"v":1.0}"#));
    let u = one(json!({"type": "array", "uniqueItems": true}));
    assert!(!accepts(&u, r#"{"v":[1,1.0]}"#));
    assert!(accepts(&u, r#"{"v":[1,2,"1"]}"#));
}

#[test]
fn integers_too_large_to_represent_are_rejected_not_rounded() {
    let t = one(json!({"type": "integer"}));
    assert!(!accepts(&t, r#"{"v":123456789012345678901234567890}"#));
    assert!(
        accepts(&t, r#"{"v":18446744073709551615}"#),
        "u64::MAX is exact"
    );
}

#[test]
fn unique_items_is_fast_on_large_arrays() {
    let t = one(json!({"type": "array", "uniqueItems": true}));
    let items: Vec<String> = (0..60_000).map(|i| i.to_string()).collect();
    let args = format!(r#"{{"v":[{}]}}"#, items.join(","));
    let started = Instant::now();
    assert!(accepts(&t, &args));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn diamond_refs_are_bounded_not_exponential() {
    let mut defs = serde_json::Map::new();
    for i in 0..30 {
        defs.insert(
            format!("D{i}"),
            json!({"type": "object", "properties": {
                "a": {"$ref": format!("#/$defs/D{}", i + 1)},
                "b": {"$ref": format!("#/$defs/D{}", i + 1)}
            }}),
        );
    }
    defs.insert("D30".into(), json!({"type": "string"}));
    let t = tool(
        json!({"type": "object", "properties": {"root": {"$ref": "#/$defs/D0"}}, "$defs": defs}),
    );
    let started = Instant::now();
    assert!(encode_tools(std::slice::from_ref(&t)).is_err());
    let _ = decode_calls(r#"<<call t {"root":{}}>>"#, std::slice::from_ref(&t));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn not_if_then_else_and_dependent_required_are_enforced() {
    let not = one(json!({"type": "string", "not": {"const": "forbidden"}}));
    assert!(!accepts(&not, r#"{"v":"forbidden"}"#));
    assert!(accepts(&not, r#"{"v":"fine"}"#));

    let cond = tool(json!({
        "type": "object",
        "properties": {"kind": {"type": "string"}, "card": {"type": "string"}},
        "if": {"properties": {"kind": {"const": "card"}}, "required": ["kind"]},
        "then": {"required": ["card"]}
    }));
    assert!(!accepts(&cond, r#"{"kind":"card"}"#));
    assert!(accepts(&cond, r#"{"kind":"card","card":"4242"}"#));
    assert!(accepts(&cond, r#"{"kind":"cash"}"#));

    let deps = tool(json!({
        "type": "object",
        "properties": {"start": {"type": "string"}, "end": {"type": "string"}},
        "dependentRequired": {"start": ["end"]}
    }));
    assert!(!accepts(&deps, r#"{"start":"a"}"#));
    assert!(accepts(&deps, r#"{"start":"a","end":"b"}"#));
}

#[test]
fn property_names_and_contains_are_enforced() {
    let names = one(json!({"type": "object", "propertyNames": {"pattern": "^[a-z]+$"}}));
    assert!(!accepts(&names, r#"{"v":{"Bad":1}}"#));
    assert!(accepts(&names, r#"{"v":{"good":1}}"#));

    let contains = one(json!({"type": "array", "contains": {"type": "integer"}, "maxContains": 2}));
    assert!(!accepts(&contains, r#"{"v":["a"]}"#));
    assert!(accepts(&contains, r#"{"v":["a",1]}"#));
    assert!(!accepts(&contains, r#"{"v":[1,2,3]}"#));
}

#[test]
fn defaults_are_never_injected_into_a_call() {
    let t = one(json!({"type": "integer", "default": 10}));
    let calls = decode_calls("<<call t {}>>", std::slice::from_ref(&t)).unwrap();
    assert!(
        calls[0].arguments.is_empty(),
        "the default was invented: {calls:?}"
    );
}

#[test]
fn many_tools_keep_every_name_and_order() {
    let tools: Vec<ToolDef> = (0..25)
        .map(|i| {
            ToolDef::new(
                format!("tool_{i:02}-x"),
                Some(format!("Tool number {i}.")),
                Some(json!({"type": "object", "properties": {"n": {"type": "integer"}}})),
            )
        })
        .collect();
    let back = decode_tools(&encode_tools(&tools).unwrap()).unwrap();
    let names: Vec<&str> = back.iter().map(|t| t.name.as_str()).collect();
    let expected: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, expected);
    assert!(decode_calls(r#"<<call tool_17-x {"n":1}>>"#, &tools).is_ok());
}
