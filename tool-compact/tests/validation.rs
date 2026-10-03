//! Argument validation through the public API (`validate_call` and `decode_calls`).

mod common;

use common::{Lcg, MUTATORS, gen_instance, gen_object, tool};
use nasiko_tool_compact::{ToolCompactError, validate_call};
use serde_json::{Value, json};

fn check(schema: Value, args: Value) -> Result<(), ToolCompactError> {
    let tools = [tool("t", schema)];
    validate_call("t", &args.to_string(), &tools).map(|_| ())
}

fn invalid_at(schema: Value, args: Value, path: &str) {
    match check(schema, args.clone()) {
        Err(ToolCompactError::InvalidArguments { path: p, .. }) => {
            assert_eq!(p, path, "for {args}")
        }
        other => panic!("{args} should be invalid at {path}, got {other:?}"),
    }
}

#[test]
fn required_optional_and_nullable_matrix() {
    let schema = json!({"type": "object", "properties": {
        "req": {"type": "string"},
        "opt": {"type": "string"},
        "nul": {"type": ["string", "null"]},
        "req_nul": {"type": ["integer", "null"]}
    }, "required": ["req", "req_nul"]});
    assert!(check(schema.clone(), json!({"req": "a", "req_nul": null})).is_ok());
    assert!(
        check(
            schema.clone(),
            json!({"req": "a", "req_nul": 1, "opt": "b", "nul": null})
        )
        .is_ok()
    );
    invalid_at(schema.clone(), json!({"req_nul": 1}), "/");
    invalid_at(schema.clone(), json!({"req": "a"}), "/");
    invalid_at(schema.clone(), json!({"req": null, "req_nul": 1}), "/req");
    invalid_at(
        schema.clone(),
        json!({"req": "a", "req_nul": 1, "opt": null}),
        "/opt",
    );
    invalid_at(schema, json!({"req": "a", "req_nul": 1.5}), "/req_nul");
}

#[test]
fn integer_semantics_and_no_coercion() {
    let schema = json!({"type": "object", "properties": {
        "i": {"type": "integer"}, "n": {"type": "number"}, "b": {"type": "boolean"}, "s": {"type": "string"}, "z": {"type": "null"}
    }});
    assert!(
        check(
            schema.clone(),
            json!({"i": 1.0, "n": 1, "b": true, "s": "1", "z": null})
        )
        .is_ok()
    );
    invalid_at(schema.clone(), json!({"i": 1.5}), "/i");
    invalid_at(schema.clone(), json!({"i": "1"}), "/i");
    invalid_at(schema.clone(), json!({"n": "1.5"}), "/n");
    invalid_at(schema.clone(), json!({"b": "true"}), "/b");
    invalid_at(schema.clone(), json!({"b": 1}), "/b");
    invalid_at(schema.clone(), json!({"s": 1}), "/s");
    invalid_at(schema, json!({"z": 0}), "/z");
}

#[test]
fn bounds_are_exact_including_exclusive_and_two_to_the_fifty_three() {
    let schema = json!({"type": "object", "properties": {
        "a": {"type": "integer", "minimum": 1, "maximum": 10},
        "b": {"type": "number", "exclusiveMinimum": 0, "exclusiveMaximum": 1},
        "c": {"type": "integer", "minimum": 9007199254740993_i64},
        "d": {"type": "integer", "maximum": -9007199254740993_i64},
        "e": {"type": "number", "minimum": 0.1, "maximum": 0.3},
        "f": {"type": "integer", "maximum": u64::MAX},
        "g": {"type": "integer", "minimum": i64::MIN}
    }});
    assert!(check(schema.clone(), json!({"a": 1, "b": 0.5})).is_ok());
    assert!(check(schema.clone(), json!({"a": 10.0})).is_ok());
    invalid_at(schema.clone(), json!({"a": 0}), "/a");
    invalid_at(schema.clone(), json!({"a": 11}), "/a");
    invalid_at(schema.clone(), json!({"b": 0}), "/b");
    invalid_at(schema.clone(), json!({"b": 1}), "/b");
    invalid_at(schema.clone(), json!({"b": 1.0}), "/b");
    // 2^53 is below the minimum 2^53+1; an f64 conversion would make them equal.
    invalid_at(schema.clone(), json!({"c": 9007199254740992.0}), "/c");
    invalid_at(schema.clone(), json!({"c": 9007199254740992_i64}), "/c");
    assert!(check(schema.clone(), json!({"c": 9007199254740993_i64})).is_ok());
    assert!(check(schema.clone(), json!({"c": 9007199254740994_i64})).is_ok());
    assert!(check(schema.clone(), json!({"d": -9007199254740993_i64})).is_ok());
    invalid_at(schema.clone(), json!({"d": -9007199254740992.0}), "/d");
    assert!(check(schema.clone(), json!({"e": 0.3})).is_ok());
    invalid_at(schema.clone(), json!({"e": 0.30000000000000004}), "/e");
    assert!(check(schema.clone(), json!({"f": u64::MAX})).is_ok());
    assert!(check(schema.clone(), json!({"f": 1e300})).is_err());
    assert!(check(schema.clone(), json!({"g": i64::MIN})).is_ok());
    invalid_at(schema, json!({"g": -1e300}), "/g");
}

#[test]
fn enum_membership_uses_numeric_equality_and_ignores_nullability() {
    let schema = json!({"type": "object", "properties": {
        "s": {"type": "string", "enum": ["public", "private"]},
        "n": {"type": ["string", "null"], "enum": ["a", null]},
        "i": {"type": "integer", "enum": [1, 2, 9007199254740993_i64]}
    }});
    assert!(check(schema.clone(), json!({"s": "public", "n": null, "i": 1.0})).is_ok());
    invalid_at(schema.clone(), json!({"s": "secret"}), "/s");
    invalid_at(schema.clone(), json!({"s": null}), "/s");
    invalid_at(schema.clone(), json!({"n": "b"}), "/n");
    invalid_at(schema.clone(), json!({"i": 3}), "/i");
    invalid_at(schema.clone(), json!({"i": 9007199254740992.0}), "/i");
    invalid_at(schema, json!({"i": "1"}), "/i");
}

#[test]
fn lengths_count_code_points() {
    let schema = json!({"type": "object", "properties": {
        "s": {"type": "string", "minLength": 2, "maxLength": 3},
        "a": {"type": "array", "items": {"type": "integer"}, "minItems": 1, "maxItems": 2}
    }});
    assert!(check(schema.clone(), json!({"s": "éé", "a": [1]})).is_ok());
    assert!(check(schema.clone(), json!({"s": "日本語"})).is_ok());
    invalid_at(schema.clone(), json!({"s": "é"}), "/s");
    invalid_at(schema.clone(), json!({"s": "日本語で"}), "/s");
    invalid_at(schema.clone(), json!({"a": []}), "/a");
    invalid_at(schema.clone(), json!({"a": [1, 2, 3]}), "/a");
    invalid_at(schema, json!({"a": [1, "2"]}), "/a/1");
}

#[test]
fn extras_follow_additional_properties_and_nested_paths_are_reported() {
    let open = json!({"type": "object", "properties": {"a": {"type": "string"}}});
    assert!(check(open.clone(), json!({"a": "x", "extra": 1})).is_ok());
    let explicit_open = json!({"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": true});
    assert!(check(explicit_open, json!({"extra": 1})).is_ok());
    let closed = json!({"type": "object", "properties": {"a": {"type": "string"}}, "additionalProperties": false});
    invalid_at(closed, json!({"a": "x", "extra": 1}), "/extra");

    let nested = json!({"type": "object", "properties": {
        "pos": {"type": "object", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}}, "required": ["x", "y"], "additionalProperties": false},
        "tags": {"type": "array", "items": {"type": "object", "properties": {"k": {"type": "string"}}, "required": ["k"]}}
    }, "required": ["pos"]});
    assert!(
        check(
            nested.clone(),
            json!({"pos": {"x": 1, "y": 2}, "tags": [{"k": "a"}]})
        )
        .is_ok()
    );
    invalid_at(nested.clone(), json!({"pos": {"x": 1}}), "/pos");
    invalid_at(
        nested.clone(),
        json!({"pos": {"x": 1, "y": 2, "z": 3}}),
        "/pos/z",
    );
    invalid_at(nested.clone(), json!({"pos": {"x": 1, "y": "2"}}), "/pos/y");
    invalid_at(
        nested,
        json!({"pos": {"x": 1, "y": 2}, "tags": [{"k": "a"}, {}]}),
        "/tags/1",
    );
}

#[test]
fn checks_compose_and_annotations_do_not_apply_defaults() {
    let schema = json!({"type": "object", "properties": {
        "n": {"type": "integer", "minimum": 0, "maximum": 10, "default": 5},
        "s": {"type": "string", "format": "date-time", "minLength": 4, "title": "When"}
    }, "required": ["n"]});
    assert!(check(schema.clone(), json!({"n": 3, "s": "2026"})).is_ok());
    invalid_at(schema.clone(), json!({}), "/");
    invalid_at(schema.clone(), json!({"n": 3.5}), "/n");
    invalid_at(schema.clone(), json!({"n": 11}), "/n");
    invalid_at(schema.clone(), json!({"n": 3, "s": "abc"}), "/s");
    // `format` is an annotation: an arbitrary string of the right length passes.
    assert!(check(schema.clone(), json!({"n": 3, "s": "not a date"})).is_ok());
    // Defaults are never applied to returned arguments.
    let call = validate_call("t", r#"{"n": 1}"#, &[tool("t", schema)]).unwrap();
    assert_eq!(call.arguments, json!({"n": 1}));
}

#[test]
fn the_any_schema_accepts_everything_and_absent_parameters_accept_any_object() {
    assert!(check(json!({}), json!({"anything": [1, {"goes": null}]})).is_ok());
    let no_params = nasiko_tool_compact::ToolDef {
        name: "t".into(),
        description: None,
        parameters: None,
    };
    assert!(validate_call("t", r#"{"x": 1}"#, std::slice::from_ref(&no_params)).is_ok());
    assert_eq!(
        validate_call("t", "[]", &[no_params]).unwrap_err().kind(),
        "malformed_call"
    );
}

#[test]
fn generated_valid_instances_pass_and_every_mutation_fails() {
    let mut rng = Lcg::new(77);
    let mut mutations = 0;
    for round in 0..500 {
        let schema = gen_object(&mut rng, 0, false);
        let instance = gen_instance(&mut rng, &schema);
        let tools = [tool("t", schema.clone())];
        if let Err(e) = validate_call("t", &instance.to_string(), &tools) {
            panic!(
                "round {round}: valid instance rejected: {e}\nschema {schema}\ninstance {instance}"
            );
        }
        for (name, mutate) in MUTATORS {
            if let Some(bad) = mutate(&schema, &instance) {
                mutations += 1;
                match validate_call("t", &bad.to_string(), &tools) {
                    Err(ToolCompactError::InvalidArguments { .. }) => {}
                    other => panic!(
                        "round {round}: mutation {name} should be invalid, got {other:?}\nschema {schema}\nbad {bad}"
                    ),
                }
            }
        }
    }
    assert!(mutations > 500, "mutators barely applied: {mutations}");
}
