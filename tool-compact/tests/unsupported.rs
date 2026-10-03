use nasiko_tool_compact::{CompactError, SchemaKind, ToolDef, analyze_tools};
use serde_json::{Value, json};

fn tool(schema: Value) -> ToolDef {
    serde_json::from_value(json!({"function":{"name":"test","parameters":schema}})).unwrap()
}

#[test]
fn unknown_keywords_never_disappear_at_any_depth() {
    for keyword in [
        "$ref",
        "$defs",
        "definitions",
        "oneOf",
        "anyOf",
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "const",
        "nullable",
        "dependentSchemas",
        "dependentRequired",
        "patternProperties",
        "contains",
        "prefixItems",
        "unevaluatedProperties",
        "unevaluatedItems",
        "propertyNames",
        "minimum",
        "maximum",
        "pattern",
        "minItems",
        "title",
        "default",
        "examples",
        "deprecated",
        "$comment",
        "vendorExtension",
    ] {
        let mut child = json!({"type":"string"});
        child[keyword] = json!(true);
        let schema = json!({"type":"object","properties":{"x":child}});
        assert!(
            matches!(analyze_tools(&[tool(schema)]), Err(CompactError::UnsupportedSchema{keyword: found, path, ..}) if found == keyword && path.ends_with(".x")),
            "{keyword}"
        );
    }
}

#[test]
fn nested_semantics_and_metadata_are_canonical() {
    let tools = analyze_tools(&[tool(json!({"type":"object","description":"root","required":["z"],"properties":{"z":{"type":"array","items":{"type":"array","items":{"type":"string","format":"date-time","enum":["later"],"description":"leaf"}}},"a":{"type":"object","additionalProperties":false}}}))]).unwrap();
    if let Some(node) = &tools[0].parameters {
        if let SchemaKind::Object {
            properties,
            additional_properties,
        } = &node.kind
        {
            assert!(*additional_properties);
            assert_eq!(properties.keys().collect::<Vec<_>>(), vec!["a", "z"]);
            assert!(properties["z"].required);
            assert!(!properties["a"].required);
        } else {
            panic!("expected object")
        }
    } else {
        panic!("expected schema")
    }
}

#[test]
fn malformed_schemas_and_duplicate_names_fail() {
    for schema in [
        json!({}),
        json!({"type":["string","null"]}),
        json!({"type":"object","properties":[]}),
        json!({"type":"object","required":["missing"]}),
        json!({"type":"object","required":false}),
        json!({"type":"array"}),
        json!({"type":"object","additionalProperties":{"type":"string"}}),
        json!({"type":"object","properties":{"x":{"type":"integer","enum":["1"]}}}),
    ] {
        assert!(analyze_tools(&[tool(schema)]).is_err());
    }
    let t = tool(json!({"type":"object"}));
    assert!(analyze_tools(&[t.clone(), t]).is_err());
}

#[test]
fn all_scalar_enums_and_zero_arguments_are_supported() {
    for (kind, values) in [
        ("string", json!(["a"])),
        ("integer", json!([1, 2])),
        ("number", json!([1, 1.5])),
        ("boolean", json!([true, false])),
    ] {
        assert!(
            analyze_tools(&[tool(
                json!({"type":"object","properties":{"x":{"type":kind,"enum":values}}})
            )])
            .is_ok()
        );
    }
    assert!(
        analyze_tools(&[tool(Value::Null)]).unwrap()[0]
            .parameters
            .is_none()
    );
}

#[test]
fn schemas_are_depth_bounded() {
    let mut schema = json!({"type":"string"});
    for _ in 0..70 {
        schema = json!({"type":"array","items":schema});
    }
    assert!(matches!(
        analyze_tools(&[tool(json!({"type":"object","properties":{"deep":schema}}))]),
        Err(CompactError::LimitExceeded(_))
    ));
}
