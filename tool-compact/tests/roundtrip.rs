//! Property-style tests: for a broad family of schemas, encoding then decoding
//! must preserve the schema semantics (required vs optional, types, enums,
//! nesting), and any tool call built from valid values must validate.

use nasiko_tool_compact::{ToolCall, ToolDef, decode_calls, decode_tools, encode_tools};
use serde_json::{Map, Value, json};

fn def(params: Value) -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {"name": "tool", "description": "d", "parameters": params}
    }))
    .unwrap()
}

fn schema_cases() -> Vec<Value> {
    vec![
        json!({"type": "object", "properties": {}, "required": []}),
        json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}),
        json!({"type": "object", "properties": {
            "n": {"type": "integer"}, "f": {"type": "number"}, "b": {"type": "boolean"},
            "s": {"type": "string", "format": "date-time"},
            "l": {"type": "string", "enum": ["x", "y", "z"]},
            "items": {"type": "array", "items": {"type": "string"}},
            "nested": {"type": "object", "properties": {"inner": {"type": "integer"}}, "required": ["inner"]}
        }, "required": ["n", "items", "nested"]}),
        json!({"type": "object", "properties": {
            "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}},
            "opt": {"type": "array", "items": {"type": "object", "properties": {"k": {"type": "string"}}}}
        }}),
    ]
}

#[test]
fn encode_decode_preserves_semantics_for_all_shapes() {
    for params in schema_cases() {
        let tools = vec![def(params.clone())];
        let compact = encode_tools(&tools).unwrap();
        let back = decode_tools(&compact).unwrap();
        assert_eq!(back.len(), 1);
        let got = back[0].function.parameters.as_ref().unwrap();

        let orig_props = params["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let got_props = got["properties"].as_object().cloned().unwrap_or_default();
        assert_eq!(orig_props.len(), got_props.len());

        let orig_req: Vec<&str> = params["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let got_req: Vec<&str> = got["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(orig_req.len(), got_req.len());
        for r in &orig_req {
            assert!(got_req.contains(r));
        }
    }
}

#[test]
fn every_valid_value_roundtrips_through_decode() {
    // Build a call with valid values for the rich schema; it must validate.
    let params = schema_cases().remove(2);
    let tools = vec![def(params)];
    let call = ToolCall {
        name: "tool".into(),
        arguments: json!({
            "n": 3, "f": 1.5, "b": true, "s": "2026-10-02T10:00:00+05:30",
            "l": "y", "items": ["a", "b"], "nested": {"inner": 7}
        }),
    };
    let text = format!("<<call tool {}>>", call.arguments);
    let calls = decode_calls(&text, &tools).unwrap();
    assert_eq!(calls, vec![call]);
}

#[test]
fn invalid_values_never_produce_a_call() {
    let params = schema_cases().remove(2);
    let tools = vec![def(params)];
    let bad: Vec<Value> = vec![
        json!({"n": "three", "items": [], "nested": {"inner": 1}}), // wrong type
        json!({"items": [], "nested": {"inner": 1}}),               // missing n
        json!({"n": 1, "items": ["a", 2], "nested": {"inner": 1}}), // bad item type
        json!({"n": 1, "items": [], "nested": {}, "l": "q"}),       // missing inner + bad enum
        json!({"n": 1, "items": [], "nested": {"inner": 1}, "l": "q"}), // bad enum
    ];
    for args in bad {
        let text = format!("<<call tool {args}>>");
        assert!(decode_calls(&text, &tools).is_err(), "must reject {args}");
    }
}

#[test]
fn extra_fields_do_not_crash_validation() {
    let tools = vec![def(schema_cases().remove(1))];
    let text = r#"<<call tool {"a":"x","unknown":42}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls[0].arguments["unknown"], json!(42));
}

#[test]
fn compact_text_has_no_trailing_whitespace_lines() {
    let tools = vec![def(schema_cases().pop().unwrap())];
    let compact = encode_tools(&tools).unwrap();
    for line in compact.text.lines() {
        assert_eq!(line, line.trim_end());
    }
    let _ = Map::<String, Value>::new();
}
