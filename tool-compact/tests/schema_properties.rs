//! Property-oriented and round-trip verification tests for nasiko-tool-compact.

use nasiko_tool_compact::schema::ToolSchema;
use nasiko_tool_compact::validate::validate_arguments;
use nasiko_tool_compact::{
    FunctionDef, ToolDef, decode_calls, decode_tools, encode_tools, render_call,
};
use serde_json::{Map, Value, json};

/// Deterministic Pseudo-Random Number Generator (SplitMix64)
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    fn next_range(&mut self, min: usize, max: usize) -> usize {
        if min >= max {
            return min;
        }
        let span = (max - min) as u64;
        min + (self.next_u64() % span) as usize
    }

    fn next_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }

    fn choose<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        let idx = self.next_range(0, slice.len());
        &slice[idx]
    }
}

/// Helper to generate arbitrary supported schemas deterministically.
fn generate_random_tool(rng: &mut SimpleRng, id: usize, depth: usize) -> ToolDef {
    let tool_name = format!("tool_{id}");
    let has_desc = rng.next_bool();
    let description = if has_desc {
        Some(format!("Deterministic generated tool {id} description."))
    } else {
        None
    };

    let params = generate_random_object_schema(rng, depth);

    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: tool_name,
            description,
            parameters: Some(params),
        },
    }
}

fn generate_random_object_schema(rng: &mut SimpleRng, max_depth: usize) -> Value {
    let prop_count = rng.next_range(1, 6);
    let mut props = Map::new();
    let mut required = Vec::new();

    for i in 0..prop_count {
        let fname = format!("f_{i}");
        let is_req = rng.next_bool();
        if is_req {
            required.push(Value::String(fname.clone()));
        }

        let fschema = generate_random_field_schema(rng, max_depth);
        props.insert(fname, fschema);
    }

    let mut obj = json!({
        "type": "object",
        "properties": props,
        "additionalProperties": false
    });

    if !required.is_empty() {
        obj["required"] = Value::Array(required);
    }

    obj
}

fn generate_random_field_schema(rng: &mut SimpleRng, max_depth: usize) -> Value {
    let type_choice = if max_depth > 0 {
        rng.next_range(0, 9)
    } else {
        rng.next_range(0, 6)
    };

    let desc = if rng.next_bool() {
        Some("A critical field with non-redundant description semantics")
    } else {
        None
    };

    let mut schema = match type_choice {
        0 => json!({"type": "string"}),
        1 => json!({"type": "integer"}),
        2 => json!({"type": "number"}),
        3 => json!({"type": "boolean"}),
        4 => {
            let formats = ["date-time", "date", "time", "email", "uri", "uuid"];
            let fmt = rng.choose(&formats);
            json!({"type": "string", "format": fmt})
        }
        5 => {
            let variants = vec![
                Value::String("alpha".into()),
                Value::String("beta".into()),
                Value::String("gamma".into()),
            ];
            json!({"type": "string", "enum": variants})
        }
        6 => {
            let item_schema = generate_random_field_schema(rng, max_depth.saturating_sub(1));
            json!({"type": "array", "items": item_schema})
        }
        7 => generate_random_object_schema(rng, max_depth.saturating_sub(1)),
        _ => json!({"type": "string"}),
    };

    if let Some(d) = desc
        && let Value::Object(ref mut map) = schema
    {
        map.insert("description".to_string(), Value::String(d.to_string()));
    }

    schema
}

fn generate_valid_args(schema_val: &Value) -> Value {
    let mut args = Map::new();
    let obj = match schema_val.as_object() {
        Some(o) => o,
        None => return Value::Object(args),
    };

    let req_keys: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for (k, s) in properties {
        let is_req = req_keys.contains(&k.as_str());
        if is_req {
            args.insert(k, sample_value_for_schema(&s));
        }
    }

    Value::Object(args)
}

fn sample_value_for_schema(s: &Value) -> Value {
    if let Some(enum_vals) = s.get("enum").and_then(Value::as_array)
        && let Some(first) = enum_vals.first()
    {
        return first.clone();
    }

    if let Some(fmt) = s.get("format").and_then(Value::as_str) {
        return match fmt {
            "date-time" => Value::String("2026-10-05T15:00:00Z".into()),
            "date" => Value::String("2026-10-05".into()),
            "time" => Value::String("15:00:00".into()),
            "email" => Value::String("dev@nasiko.org".into()),
            "uri" => Value::String("https://nasiko.org".into()),
            "uuid" => Value::String("123e4567-e89b-12d3-a456-426614174000".into()),
            _ => Value::String("sample".into()),
        };
    }

    match s.get("type").and_then(Value::as_str).unwrap_or("string") {
        "string" => Value::String("test_string".into()),
        "integer" => Value::Number(42.into()),
        "number" => Value::Number(serde_json::Number::from_f64(3.5).unwrap()),
        "boolean" => Value::Bool(true),
        "array" => {
            let item_s = s.get("items").unwrap_or(&Value::Null);
            Value::Array(vec![sample_value_for_schema(item_s)])
        }
        "object" => generate_valid_args(s),
        _ => Value::String("value".into()),
    }
}

// =========================================================================
// PROPERTY 1: Supported schema round-trip preserves semantic meaning
// =========================================================================

#[test]
fn test_property_1_roundtrip_semantic_preservation() {
    let mut rng = SimpleRng::new(0xDEADBEEF);
    const NUM_CASES: usize = 100;

    for i in 0..NUM_CASES {
        let tool = generate_random_tool(&mut rng, i, 2);

        // A. JSON Schema -> CanonicalSchema -> JSON Schema
        let canonical = ToolSchema::from_tool_def(&tool)
            .unwrap_or_else(|e| panic!("Failed to parse valid schema {i}: {e}"));
        let roundtrip_tool = canonical.to_tool_def();

        assert!(
            ToolSchema::semantically_equal(&tool, &roundtrip_tool),
            "Schema roundtrip semantic mismatch for case {i}"
        );

        // B. ToolDef -> encode_tools -> decode_tools -> ToolDef'
        let compact = encode_tools(std::slice::from_ref(&tool))
            .unwrap_or_else(|e| panic!("Failed to encode tool {i}: {e}"));
        let decoded_tools = decode_tools(&compact)
            .unwrap_or_else(|e| panic!("Failed to decode compact tools {i}: {e}"));

        assert_eq!(decoded_tools.len(), 1);
        assert!(
            ToolSchema::semantically_equal(&tool, &decoded_tools[0]),
            "Compact roundtrip semantic mismatch for case {i}"
        );
    }
}

// =========================================================================
// PROPERTY 2 & 3: Argument Validation Invariance (Valid accepted, Invalid rejected)
// =========================================================================

#[test]
fn test_property_2_and_3_argument_validation_invariance() {
    let mut rng = SimpleRng::new(0xCAFEBABE);
    const NUM_CASES: usize = 50;

    for i in 0..NUM_CASES {
        let tool = generate_random_tool(&mut rng, i, 1);
        let canonical = ToolSchema::from_tool_def(&tool).unwrap();
        let rt_tool = canonical.to_tool_def();

        let orig_param_map = tool
            .function
            .parameters
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap();
        let rt_param_map = rt_tool
            .function
            .parameters
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap();

        // Property 2: Valid arguments accepted by orig are accepted by round-trip
        let valid_args = generate_valid_args(tool.function.parameters.as_ref().unwrap());
        let valid_map = valid_args.as_object().unwrap();

        let orig_valid_res = validate_arguments(&tool.function.name, valid_map, orig_param_map);
        let rt_valid_res = validate_arguments(&rt_tool.function.name, valid_map, rt_param_map);

        assert!(
            orig_valid_res.is_ok(),
            "Original failed on valid args for case {i}: {:?}",
            orig_valid_res
        );
        assert!(
            rt_valid_res.is_ok(),
            "Roundtrip failed on valid args for case {i}: {:?}",
            rt_valid_res
        );

        // Property 3: Invalid arguments rejected by orig are rejected by round-trip
        let mut invalid_map = valid_map.clone();
        invalid_map.insert(
            "unexpected_rogue_key".to_string(),
            Value::String("bad".into()),
        );

        let orig_invalid_res =
            validate_arguments(&tool.function.name, &invalid_map, orig_param_map);
        let rt_invalid_res = validate_arguments(&rt_tool.function.name, &invalid_map, rt_param_map);

        assert!(
            orig_invalid_res.is_err(),
            "Original must reject rogue key for case {i}"
        );
        assert!(
            rt_invalid_res.is_err(),
            "Roundtrip must reject rogue key for case {i}"
        );
        assert_eq!(
            orig_invalid_res.unwrap_err().code(),
            rt_invalid_res.unwrap_err().code()
        );
    }
}

// =========================================================================
// PROPERTY 4: Determinism of Compact Encoding
// =========================================================================

#[test]
fn test_property_4_encoding_determinism() {
    let mut rng = SimpleRng::new(0x12345678);
    const NUM_CASES: usize = 20;

    for i in 0..NUM_CASES {
        let tool = generate_random_tool(&mut rng, i, 2);

        let baseline = encode_tools(std::slice::from_ref(&tool))
            .unwrap()
            .definitions;
        for _ in 0..50 {
            let test_run = encode_tools(std::slice::from_ref(&tool))
                .unwrap()
                .definitions;
            assert_eq!(baseline, test_run, "Non-deterministic encoding in case {i}");
        }
    }
}

// =========================================================================
// PROPERTY 5: Determinism of Decoding
// =========================================================================

#[test]
fn test_property_5_decoding_determinism() {
    let mut rng = SimpleRng::new(0x87654321);
    const NUM_CASES: usize = 20;

    for i in 0..NUM_CASES {
        let tool = generate_random_tool(&mut rng, i, 1);
        let valid_args = generate_valid_args(tool.function.parameters.as_ref().unwrap());
        let call_text = render_call(&tool.function.name, &valid_args);

        let baseline = decode_calls(&call_text, std::slice::from_ref(&tool)).unwrap();
        for _ in 0..50 {
            let test_run = decode_calls(&call_text, std::slice::from_ref(&tool)).unwrap();
            assert_eq!(baseline, test_run, "Non-deterministic decoding in case {i}");
        }
    }
}

// =========================================================================
// PROPERTY 6: Unsupported Schemas Fail-Closed (Never Silently Accepted)
// =========================================================================

#[test]
fn test_property_6_unsupported_schemas_never_silently_accepted() {
    let unsupported_cases: Vec<(&str, Value)> = vec![
        (
            "anyOf root",
            json!({"type": "object", "anyOf": [{"properties": {"a": {"type": "string"}}}]}),
        ),
        (
            "oneOf root",
            json!({"type": "object", "oneOf": [{"properties": {"a": {"type": "string"}}}]}),
        ),
        (
            "allOf root",
            json!({"type": "object", "allOf": [{"properties": {"a": {"type": "string"}}}]}),
        ),
        (
            "$ref root",
            json!({"type": "object", "$ref": "#/definitions/CustomType"}),
        ),
        ("$defs root", json!({"type": "object", "$defs": {}})),
        (
            "not root",
            json!({"type": "object", "not": {"properties": {"a": {"type": "string"}}}}),
        ),
        (
            "patternProperties",
            json!({"type": "object", "patternProperties": {"^S_": {"type": "string"}}}),
        ),
        (
            "additionalProperties true",
            json!({"type": "object", "additionalProperties": true}),
        ),
        (
            "additionalProperties schema",
            json!({"type": "object", "additionalProperties": {"type": "string"}}),
        ),
        (
            "pattern constraint",
            json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "^[0-9]+$"}}}),
        ),
        (
            "minimum constraint",
            json!({"type": "object", "properties": {"a": {"type": "integer", "minimum": 1}}}),
        ),
        (
            "maximum constraint",
            json!({"type": "object", "properties": {"a": {"type": "number", "maximum": 100}}}),
        ),
        (
            "default value",
            json!({"type": "object", "properties": {"a": {"type": "string", "default": "fallback"}}}),
        ),
        (
            "minLength",
            json!({"type": "object", "properties": {"a": {"type": "string", "minLength": 5}}}),
        ),
        (
            "maxLength",
            json!({"type": "object", "properties": {"a": {"type": "string", "maxLength": 10}}}),
        ),
        (
            "minItems",
            json!({"type": "object", "properties": {"a": {"type": "array", "items": {"type": "string"}, "minItems": 1}}}),
        ),
        (
            "maxItems",
            json!({"type": "object", "properties": {"a": {"type": "array", "items": {"type": "string"}, "maxItems": 5}}}),
        ),
        (
            "uniqueItems",
            json!({"type": "object", "properties": {"a": {"type": "array", "items": {"type": "string"}, "uniqueItems": true}}}),
        ),
        (
            "unsupported string format ipv4",
            json!({"type": "object", "properties": {"a": {"type": "string", "format": "ipv4"}}}),
        ),
        (
            "unsupported string format hostname",
            json!({"type": "object", "properties": {"a": {"type": "string", "format": "hostname"}}}),
        ),
        (
            "unsupported string format regex",
            json!({"type": "object", "properties": {"a": {"type": "string", "format": "regex"}}}),
        ),
        (
            "invalid tool name",
            json!({"type": "object", "properties": {"bad-field-$-name": {"type": "string"}}}),
        ),
    ];

    for (label, param_schema) in unsupported_cases {
        let tool = ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "unsupported_tool".to_string(),
                description: Some(format!("Test for {label}")),
                parameters: Some(param_schema),
            },
        };

        // Canonical parse MUST fail
        let canonical_res = ToolSchema::from_tool_def(&tool);
        assert!(
            canonical_res.is_err(),
            "Expected UnsupportedSchema for '{label}', but got: Ok"
        );

        // Encoder MUST fail
        let encode_res = encode_tools(&[tool]);
        assert!(
            encode_res.is_err(),
            "Expected encode_tools to reject '{label}', but got: Ok"
        );
    }
}
