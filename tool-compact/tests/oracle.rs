//! Differential tests against an independent JSON Schema validator (`jsonschema`, Draft
//! 2020-12, formats not asserted, no reference resolution).
//!
//! For every schema the crate accepts, the hand-written validator must agree with the oracle on
//! every generated valid instance and every mutation. Hand-written conformance cases in
//! `tests/fixtures/conformance.json` pin the regressions reviewers asked for.

mod common;

use common::{Lcg, MUTATORS, gen_instance, gen_object, tool};
use jsonschema::Draft;
use nasiko_tool_compact::{ToolCompactError, encode_tools, validate_call};
use serde_json::{Value, json};

fn oracle(schema: &Value) -> jsonschema::Validator {
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .should_validate_formats(false)
        .build(schema)
        .expect("oracle compiles the schema")
}

fn ours(schema: &Value, instance: &Value) -> bool {
    let tools = [tool("t", schema.clone())];
    match validate_call("t", &instance.to_string(), &tools) {
        Ok(_) => true,
        Err(ToolCompactError::InvalidArguments { .. }) => false,
        Err(e) => panic!("unexpected error kind {}: {e}", e.kind()),
    }
}

#[test]
fn generated_instances_and_mutations_agree_with_the_oracle() {
    let mut rng = Lcg::new(4242);
    let mut compared = 0usize;
    for round in 0..600 {
        let schema = gen_object(&mut rng, 0, false);
        let validator = oracle(&schema);
        let instance = gen_instance(&mut rng, &schema);
        let mut candidates = vec![("valid", instance.clone())];
        for (name, mutate) in MUTATORS {
            if let Some(bad) = mutate(&schema, &instance) {
                candidates.push((name, bad));
            }
        }
        for (label, candidate) in candidates {
            let expected = validator.is_valid(&candidate);
            let got = ours(&schema, &candidate);
            compared += 1;
            assert_eq!(
                got, expected,
                "round {round} ({label}): oracle={expected} ours={got}\nschema {schema}\ninstance {candidate}"
            );
        }
    }
    assert!(compared > 1500, "too few comparisons: {compared}");
}

#[test]
fn conformance_fixtures_agree_with_the_oracle_and_pin_expected_verdicts() {
    let text = include_str!("fixtures/conformance.json");
    let cases: Vec<Value> = serde_json::from_str(text).expect("fixture parses");
    assert!(cases.len() >= 20);
    for case in cases {
        let note = case["note"].as_str().unwrap_or("");
        let schema = &case["schema"];
        let instance = &case["instance"];
        let expected = case["valid"].as_bool().expect("valid flag");
        let supported = case["supported"].as_bool().unwrap_or(true);
        let oracle_says = oracle(schema).is_valid(instance);
        assert_eq!(
            oracle_says, expected,
            "oracle disagrees with fixture: {note}"
        );
        let tools = [tool("t", schema.clone())];
        match encode_tools(&tools) {
            Ok(_) => {
                assert!(supported, "{note}: expected the schema to be declined");
                assert_eq!(ours(schema, instance), expected, "{note}");
            }
            Err(e) => {
                assert!(!supported, "{note}: unexpectedly declined: {e}");
                assert_eq!(e.kind(), "unsupported_schema", "{note}");
                // Declined schemas never validate anything, in either direction.
                assert_eq!(
                    validate_call("t", &instance.to_string(), &tools)
                        .unwrap_err()
                        .kind(),
                    "unsupported_schema",
                    "{note}"
                );
            }
        }
    }
}

#[test]
fn a_nullable_enum_without_null_never_admits_null_through_this_crate() {
    let schema = json!({"type": "object", "properties": {"v": {"type": ["string", "null"], "enum": ["public", "private"]}}});
    assert!(!oracle(&schema).is_valid(&json!({"v": null})));
    // The crate declines the schema, so there is no code path that could accept null for it.
    let err = validate_call("t", r#"{"v": null}"#, &[tool("t", schema)]).unwrap_err();
    assert_eq!(err.kind(), "unsupported_schema");
}
