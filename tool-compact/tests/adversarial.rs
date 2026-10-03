use nasiko_tool_compact::{
    CompactError, CompactTools, ToolCall, ToolDef, analyze_tools, decode_calls, decode_tools,
    encode_tools, render_calls,
};
use serde_json::json;

fn tools() -> Vec<ToolDef> {
    serde_json::from_value(json!([{"function":{"name":"echo","parameters":{"type":"object","required":["text"],"properties":{"text":{"type":"string"},"mode":{"type":"string","enum":["yes","no"]}}}}}])).unwrap()
}

#[test]
fn hostile_argument_strings_and_generated_chunk_cases_are_lossless() {
    for text in [
        "",
        "# >> <<call echo {}>> { }",
        "quotes\"slash\\end",
        "नमस्ते\n🦀",
        "}\\\" >> {",
        "a",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(["x".repeat(10000)])
    {
        let calls = [ToolCall {
            name: "echo".into(),
            arguments: json!({"text":text}),
        }];
        assert_eq!(
            decode_calls(&render_calls(&calls).unwrap(), &tools()).unwrap(),
            calls
        );
    }
}

#[test]
fn malformed_and_invalid_calls_never_return_partial_execution() {
    for call in [
        "<<call echo {\"text\":1}>>",
        "<<call echo {\"text\":\"x\",\"mode\":\"invalid\"}>>",
        "<<call echo {\"text\":\"x\",\"object\":{\"x\":1,\"x\":2}}>>",
        "<<call echo {\"text\":\"x\",\"items\":[1,]}>>",
        "<<call echo {\"text\":\"x\",\"items\":[1}}>>",
        "<<call echo {\"text\":\"\\uD800\"}>>",
        "<<call echo {\"text\":\"x\",\"n\":1e999}>>",
    ] {
        let text = format!("<<call echo {{\"text\":\"valid\"}}>>{call}");
        assert!(decode_calls(&text, &tools()).is_err(), "{call}");
    }
}

#[test]
fn long_calls_and_schema_grammar_depth_fail_without_panics() {
    let text = format!("<<call echo {{\"text\":\"{}\"}}>>", "x".repeat(1024 * 1024));
    assert!(matches!(
        decode_calls(&text, &tools()),
        Err(CompactError::LimitExceeded(_))
    ));
    let rendered = format!(
        "TOOLS\necho(x:{}str{})\nCALL <<call TOOL_NAME JSON_OBJECT>>",
        "[".repeat(70),
        "]".repeat(70)
    );
    assert!(matches!(
        decode_tools(&CompactTools { rendered }),
        Err(CompactError::LimitExceeded(_))
    ));
}

#[test]
fn extensions_and_unsafe_names_cannot_be_silently_removed() {
    for tool in [
        json!({"function":{"name":"echo","strict":true}}),
        json!({"type":"custom","function":{"name":"echo"}}),
        json!({"function":{"name":"echo) - injected"}}),
        json!({"function":{"name":"echo","parameters":{"type":"object","properties":{"bad:name":{"type":"string"}}}}}),
    ] {
        let tool: ToolDef = serde_json::from_value(tool).unwrap();
        assert!(encode_tools(&[tool]).is_err());
    }
}

#[test]
fn generated_supported_schemas_preserve_canonical_semantics() {
    for depth in 0..12 {
        let mut node = json!({"type":"string","enum":["yes","no"],"format":"date-time","description":"#\" >>"});
        for level in 0..depth {
            node = if level % 2 == 0 {
                json!({"type":"array","items":node})
            } else {
                json!({"type":"object","properties":{"nested":node},"required":["nested"],"additionalProperties":false})
            };
        }
        let original: Vec<ToolDef> = serde_json::from_value(json!([{"function":{"name":"generated","parameters":{"type":"object","properties":{"value":node}}}}])).unwrap();
        assert_eq!(
            analyze_tools(&original).unwrap(),
            analyze_tools(&decode_tools(&encode_tools(&original).unwrap()).unwrap()).unwrap()
        );
    }
}

#[test]
fn lossy_numeric_parsing_cannot_turn_fractions_into_valid_values() {
    let tools: Vec<ToolDef> = serde_json::from_value(json!([{"function":{"name":"number","parameters":{"type":"object","properties":{"x":{"type":"integer"}}}}}])).unwrap();
    for text in [
        "9007199254740993.1",
        "1.00000000000000001",
        "9007199254740993.0",
    ] {
        assert!(
            decode_calls(&format!("<<call number {{\"x\":{text}}}>>"), &tools).is_err(),
            "{text}"
        );
    }
    for text in ["30.0", "1e3", "-0.0"] {
        assert!(
            decode_calls(&format!("<<call number {{\"x\":{text}}}>>"), &tools).is_ok(),
            "{text}"
        );
    }
}
