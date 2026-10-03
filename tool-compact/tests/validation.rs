use nasiko_tool_compact::{ToolCall, ToolDef, validate_call};
use serde_json::{Value, json};

fn tool(schema: Value) -> ToolDef {
    serde_json::from_value(json!({"function":{"name":"foo","parameters":schema}})).unwrap()
}
fn validate(schema: Value, arguments: Value) -> bool {
    validate_call(
        &ToolCall {
            name: "foo".into(),
            arguments,
        },
        &[tool(schema)],
    )
    .is_ok()
}

#[test]
fn recursive_validation_has_no_coercion() {
    let schema = json!({"type":"object","additionalProperties":false,"required":["count","flag","nested"],"properties":{"count":{"type":"integer","enum":[30]},"flag":{"type":"boolean"},"nested":{"type":"array","items":{"type":"object","required":["name"],"properties":{"name":{"type":"string","enum":["yes"]}}}}}});
    let valid = json!({"count":30,"flag":true,"nested":[{"name":"yes"}]});
    assert!(validate(schema.clone(), valid.clone()));
    for (key, value) in [
        ("count", json!("30")),
        ("count", json!(30.5)),
        ("flag", json!("true")),
        ("nested", json!([{"name":"no"}])),
        ("nested", json!([{}])),
        ("nested", json!({})),
        ("extra", json!(1)),
    ] {
        let mut arguments = valid.clone();
        arguments[key] = value;
        assert!(!validate(schema.clone(), arguments), "{key}");
    }
    assert!(!validate(schema.clone(), json!({})));
    assert!(!validate(schema, json!([])));
}

#[test]
fn additional_properties_default_true_false_and_zero_arguments() {
    for schema in [
        json!({"type":"object"}),
        json!({"type":"object","additionalProperties":true}),
    ] {
        assert!(validate(schema, json!({"extra":[1]})));
    }
    assert!(!validate(
        json!({"type":"object","additionalProperties":false}),
        json!({"extra":1})
    ));
    assert!(validate(Value::Null, json!({})));
    assert!(!validate(Value::Null, json!({"extra":1})));
}

#[test]
fn numeric_semantics_and_enums() {
    for value in [json!(30), json!(30.0)] {
        assert!(validate(
            json!({"type":"object","properties":{"x":{"type":"integer","enum":[30]}}}),
            json!({"x":value})
        ));
    }
    for value in [json!(1), json!(1.5)] {
        assert!(validate(
            json!({"type":"object","properties":{"x":{"type":"number"}}}),
            json!({"x":value})
        ));
    }
    assert!(!validate(
        json!({"type":"object","properties":{"x":{"type":"integer","enum":[9007199254740992u64]}}}),
        json!({"x":9007199254740993u64})
    ));
}

#[test]
fn unknown_tool_cannot_validate() {
    assert!(
        validate_call(
            &ToolCall {
                name: "unknown".into(),
                arguments: json!({})
            },
            &[tool(Value::Null)]
        )
        .is_err()
    );
}
