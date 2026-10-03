//! Golden lines for every construct of the notation.
//!
//! These strings are the contract documented in README.md; a change here is a grammar change.
//! The same goldens are asserted from `nasiko-llm-router`'s tests (where `serde_json` may have
//! `preserve_order` enabled) to prove the encoder's output does not depend on map iteration.

mod common;

use common::tool;
use nasiko_tool_compact::{HEADER, INSTRUCTIONS, ToolDef, encode_tools};
use serde_json::{Value, json};

fn line(parameters: Value) -> String {
    encode_tools(&[tool("t", parameters)])
        .expect("supported")
        .definitions()
        .to_owned()
}

#[test]
fn scalar_types_and_constraints() {
    assert_eq!(
        line(json!({"type": "object", "properties": {
            "s": {"type": "string"},
            "i": {"type": "integer"},
            "n": {"type": "number"},
            "b": {"type": "boolean"},
            "z": {"type": "null"},
            "any": {}
        }})),
        "t(any?:*, b?:bool, i?:int, n?:num, s?:str, z?:null) - test tool"
    );
    assert_eq!(
        line(json!({"type": "object", "properties": {
            "a": {"type": "string", "minLength": 1, "maxLength": 200},
            "b": {"type": "integer", "minimum": 1, "maximum": 100},
            "c": {"type": "integer", "minimum": 0},
            "d": {"type": "number", "maximum": 1.5},
            "e": {"type": "number", "exclusiveMinimum": 0, "maximum": 1},
            "f": {"type": "integer", "minimum": 0, "exclusiveMinimum": 1, "maximum": 9, "exclusiveMaximum": 10}
        }})),
        "t(a?:str(min=1,max=200), b?:int(1..100), c?:int(0..), d?:num(..1.5), e?:num(gt=0,le=1), f?:int(ge=0,gt=1,le=9,lt=10)) - test tool"
    );
}

#[test]
fn nullable_arrays_objects_and_enums() {
    assert_eq!(
        line(json!({"type": "object", "properties": {
            "a": {"type": ["string", "null"]},
            "b": {"type": "array", "items": {"type": "string"}},
            "c": {"type": "array"},
            "d": {"type": ["array", "null"], "items": {"type": ["integer", "null"]}, "minItems": 1, "maxItems": 5},
            "e": {"type": "object", "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}}, "required": ["x", "y"], "additionalProperties": false},
            "f": {"type": ["object", "null"]},
            "g": {"type": "object", "properties": {}, "additionalProperties": true},
            "h": {"type": "string", "enum": ["public", "private"]},
            "i": {"type": ["string", "null"], "enum": ["a", null]},
            "j": {"type": "integer", "enum": [1, 2, 3]}
        }, "required": ["e"]})),
        "t(e:{x:int, y:int}!, a?:str|null, b?:[str], c?:[...], d?:[int|null](1..5)|null, f?:{...}|null, g?:{}+, h?:public|private, i?:a|null, j?:1|2|3) - test tool"
    );
}

#[test]
fn descriptions_are_quoted_on_properties_and_raw_or_quoted_on_tools() {
    assert_eq!(
        encode_tools(&[ToolDef {
            name: "t".into(),
            description: Some("Plain. Has \"quotes\" too".into()),
            parameters: Some(
                json!({"type": "object", "description": "root \"d\"", "properties": {
                    "a": {"type": "string", "description": "Event title"}
                }})
            )
        }])
        .unwrap()
        .definitions(),
        "t(a?:str 'Event title') 'root \"d\"' - Plain. Has \"quotes\" too"
    );
    for (desc, expected) in [
        ("two\nlines", "t - \"two\\nlines\""),
        (" leading", "t - \" leading\""),
        ("trailing ", "t - \"trailing \""),
        ("", "t - \"\""),
        ("\"quoted start", "t - \"\\\"quoted start\""),
        ("日本語 🎉", "t - 日本語 🎉"),
    ] {
        let compact = encode_tools(&[ToolDef {
            name: "t".into(),
            description: Some(desc.into()),
            parameters: None,
        }])
        .unwrap();
        assert_eq!(compact.definitions(), expected, "for {desc:?}");
    }
}

#[test]
fn reserved_words_and_digit_strings_are_quoted_in_enums() {
    assert_eq!(
        line(json!({"type": "object", "properties": {
            "a": {"type": "string", "enum": ["str", "int", "null", "true", "2", "-1", "a b", "ok_word", "zh-Hans", "日本"]}
        }})),
        "t(a?:\"str\"|\"int\"|\"null\"|\"true\"|\"2\"|\"-1\"|\"a b\"|ok_word|zh-Hans|\"日本\") - test tool"
    );
}

#[test]
fn the_prompt_is_header_definitions_then_instructions() {
    let compact = encode_tools(&[tool("t", json!({"type": "object"}))]).unwrap();
    assert_eq!(compact.instructions(), INSTRUCTIONS);
    assert_eq!(compact.header(), HEADER);
    assert_eq!(
        compact.prompt(),
        format!("{HEADER}\nt(...) - test tool\n{INSTRUCTIONS}")
    );
    assert!(INSTRUCTIONS.contains("<<call tool_name {\"arg\": \"value\"}>>"));
    assert!(
        !INSTRUCTIONS.to_lowercase().contains("validat")
            && !HEADER.to_lowercase().contains("validat"),
        "format aliases must not be advertised as validation"
    );
}

#[test]
fn per_tool_lines_match_the_joined_definitions() {
    let tools = vec![
        tool("a", json!({"type": "object"})),
        tool(
            "b",
            json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        ),
    ];
    let compact = encode_tools(&tools).unwrap();
    let joined: Vec<&str> = compact.tools().iter().map(|t| t.line.as_str()).collect();
    assert_eq!(compact.definitions(), joined.join("\n"));
    assert_eq!(compact.tools()[1].name, "b");
    assert!(encode_tools(&[]).unwrap().definitions().is_empty());
}
