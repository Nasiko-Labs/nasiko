//! Integration tests for encode → decode round-trip correctness.

use nasiko_tool_compact::{decode_calls, decode_tools, encode_tools};
use nasiko_tool_compact::types::{FunctionDef, ToolDef};
use serde_json::json;

fn make_tool(name: &str, desc: Option<&str>, params: Option<serde_json::Value>) -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: name.into(),
            description: desc.map(String::from),
            parameters: params,
        },
        extra: serde_json::Map::new(),
    }
}

#[test]
fn roundtrip_simple_tool() {
    let tools = vec![make_tool(
        "get_weather",
        Some("Get current weather."),
        Some(json!({
            "type": "object",
            "properties": {
                "location": { "type": "string" }
            },
            "required": ["location"]
        })),
    )];

    let compact = encode_tools(&tools).unwrap();
    assert!(compact.text.contains("get_weather(location:str)"));

    // Simulate model output.
    let model_output = r#"<<call get_weather {"location":"San Francisco"}>>"#;
    let calls = decode_calls(model_output, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "get_weather");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["location"], "San Francisco");
}

#[test]
fn roundtrip_multi_field_tool() {
    let tools = vec![make_tool(
        "create_event",
        Some("Create a calendar event."),
        Some(json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "date": { "type": "string" },
                "duration_minutes": { "type": "integer" },
                "recurring": { "type": "boolean" },
                "priority": { "type": "string", "enum": ["low", "medium", "high"] }
            },
            "required": ["title", "date"]
        })),
    )];

    let compact = encode_tools(&tools).unwrap();
    assert!(compact.text.contains("title:str"));
    assert!(compact.text.contains("date:str"));
    assert!(compact.text.contains("priority?:low|medium|high"));

    let model_output =
        r#"<<call create_event {"title":"Meeting","date":"2024-01-15","priority":"high"}>>"#;
    let calls = decode_calls(model_output, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_event");
}

#[test]
fn roundtrip_multiple_tools() {
    let tools = vec![
        make_tool(
            "search",
            Some("Search."),
            Some(json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            })),
        ),
        make_tool(
            "calculate",
            Some("Calculate."),
            Some(json!({
                "type": "object",
                "properties": { "expression": { "type": "string" } },
                "required": ["expression"]
            })),
        ),
    ];

    let compact = encode_tools(&tools).unwrap();
    assert!(compact.text.contains("search("));
    assert!(compact.text.contains("calculate("));

    let model_output = r#"<<call search {"query":"pi value"}>> and <<call calculate {"expression":"3.14159 * 2"}>>"#;
    let calls = decode_calls(model_output, &tools).unwrap();
    assert_eq!(calls.len(), 2);
}

#[test]
fn roundtrip_tool_with_array() {
    let tools = vec![make_tool(
        "batch_process",
        Some("Process items."),
        Some(json!({
            "type": "object",
            "properties": {
                "items": { "type": "array", "items": { "type": "string" } },
                "parallel": { "type": "boolean" }
            },
            "required": ["items"]
        })),
    )];

    let compact = encode_tools(&tools).unwrap();
    assert!(compact.text.contains("items:[str]"));

    let model_output = r#"<<call batch_process {"items":["a","b","c"]}>>"#;
    let calls = decode_calls(model_output, &tools).unwrap();
    assert_eq!(calls.len(), 1);
}

#[test]
fn decode_tools_roundtrip_preserves_schema() {
    let tools = vec![
        make_tool(
            "search",
            Some("Search the web."),
            Some(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            })),
        ),
        make_tool(
            "translate",
            Some("Translate text."),
            Some(json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "lang": { "type": "string", "enum": ["en", "fr", "de"] }
                },
                "required": ["text", "lang"]
            })),
        ),
    ];

    let compact = encode_tools(&tools).unwrap();
    let reconstructed = decode_tools(&compact.schemas).unwrap();

    assert_eq!(reconstructed.len(), tools.len());
    for (orig, recon) in tools.iter().zip(reconstructed.iter()) {
        assert_eq!(orig.function.name, recon.function.name);
        assert_eq!(orig.function.description, recon.function.description);
        assert_eq!(orig.function.parameters, recon.function.parameters);
    }
}

#[test]
fn fail_closed_unknown_tool() {
    let tools = vec![make_tool(
        "known",
        None,
        Some(json!({ "type": "object", "properties": {} })),
    )];
    let output = r#"<<call unknown {"x":"y"}>>"#;
    let err = decode_calls(output, &tools).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unknown tool"), "got: {msg}");
}

#[test]
fn fail_closed_missing_required() {
    let tools = vec![make_tool(
        "search",
        None,
        Some(json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        })),
    )];
    let output = r#"<<call search {"wrong_field":"value"}>>"#;
    let err = decode_calls(output, &tools).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("missing required field"), "got: {msg}");
}

#[test]
fn fail_closed_invalid_enum() {
    let tools = vec![make_tool(
        "set_mode",
        None,
        Some(json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "enum": ["fast", "safe"] }
            },
            "required": ["mode"]
        })),
    )];
    let output = r#"<<call set_mode {"mode":"dangerous"}>>"#;
    let err = decode_calls(output, &tools).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("invalid enum value"), "got: {msg}");
}

#[test]
fn bypass_tool_still_decodable() {
    let tools = vec![make_tool(
        "complex",
        Some("A complex tool."),
        Some(json!({
            "type": "object",
            "properties": {
                "input": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "integer" }
                    ]
                }
            }
        })),
    )];

    let compact = encode_tools(&tools).unwrap();
    assert!(compact.schemas[0].bypassed);

    // decode_tools should still reconstruct it.
    let reconstructed = decode_tools(&compact.schemas).unwrap();
    assert_eq!(reconstructed[0].function.parameters, tools[0].function.parameters);
}

#[test]
fn empty_tools_roundtrip() {
    let tools: Vec<ToolDef> = vec![];
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.schemas.is_empty());
    let reconstructed = decode_tools(&compact.schemas).unwrap();
    assert!(reconstructed.is_empty());
}

#[test]
fn tool_with_no_params_roundtrip() {
    let tools = vec![make_tool("ping", Some("Ping server."), None)];
    let compact = encode_tools(&tools).unwrap();
    assert!(compact.text.contains("ping()"));

    // A call with empty args should work.
    let output = r#"<<call ping {}>>"#;
    let calls = decode_calls(output, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "ping");
}
