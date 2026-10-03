//! Comprehensive tests for the streaming decoder edge cases.
//!
//! These tests verify that the StreamDecoder handles markers split at all
//! possible boundary positions without losing or misinterpreting data.

use nasiko_tool_compact::{encode_tools, StreamDecoder, ToolDef};
use nasiko_tool_compact::types::FunctionDef;
use serde_json::json;

fn make_tool(name: &str, desc: &str, params: serde_json::Value) -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: name.into(),
            description: Some(desc.into()),
            parameters: Some(params),
        },
        extra: serde_json::Map::new(),
    }
}

#[test]
fn single_call_one_chunk() {
    let tools = vec![make_tool(
        "search",
        "Search the web.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    let text = r#"Let me search for that. <<call search {"query":"rust"}>>"#;
    decoder.push(text);

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "search");
    
    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["query"], "rust");
}

#[test]
fn single_call_split_across_two_chunks() {
    let tools = vec![make_tool(
        "search",
        "Search the web.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // Split in the middle of the marker
    decoder.push(r#"Let me search. <<call search {"qu"#);
    decoder.push(r#"ery":"rust"}>>"#);

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "search");
}

#[test]
fn single_call_split_byte_by_byte() {
    let tools = vec![make_tool(
        "ping",
        "Ping server.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    let text = r#"<<call ping {}>>"#;
    
    // Feed one character at a time
    for ch in text.chars() {
        decoder.push(&ch.to_string());
    }

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "ping");
}

#[test]
fn multiple_calls_in_one_chunk() {
    let tools = vec![
        make_tool(
            "search",
            "Search.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        ),
        make_tool(
            "translate",
            "Translate.",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "lang": { "type": "string" }
                },
                "required": ["text", "lang"]
            }),
        ),
    ];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    let text = r#"<<call search {"query":"hello"}>> and <<call translate {"text":"hi","lang":"fr"}>>"#;
    decoder.push(text);

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "search");
    assert_eq!(calls[1].function.name, "translate");
}

#[test]
fn multiple_calls_each_split_differently() {
    let tools = vec![
        make_tool(
            "search",
            "Search.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        ),
        make_tool(
            "calculate",
            "Calculate.",
            json!({
                "type": "object",
                "properties": {
                    "expr": { "type": "string" }
                },
                "required": ["expr"]
            }),
        ),
    ];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // First call split mid-marker, second call complete
    decoder.push(r#"First: <<call search {"q"#);
    decoder.push(r#"uery":"test"}>> Second: <<call calculate {"expr":"2+2"}>>"#);

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "search");
    assert_eq!(calls[1].function.name, "calculate");
}

#[test]
fn incomplete_marker_at_end_is_error() {
    let tools = vec![make_tool(
        "search",
        "Search.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // Incomplete marker (no closing >>)
    decoder.push(r#"Starting a call: <<call search {"query":"test"#);

    let result = decoder.finish();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("incomplete") || err_msg.contains("Incomplete"));
}

#[test]
fn text_with_no_markers() {
    let tools = vec![make_tool(
        "search",
        "Search.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    decoder.push("Just regular text without any tool calls.");
    decoder.push(" More text here.");

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 0);
}

#[test]
fn marker_split_at_open_boundary() {
    let tools = vec![make_tool(
        "ping",
        "Ping.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // Split at "<<c" - the most fragile boundary for "<<call"
    decoder.push("Starting now <<c");
    decoder.push("all ping {}>>");

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "ping");
}

#[test]
fn marker_split_at_close_boundary() {
    let tools = vec![make_tool(
        "ping",
        "Ping.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // Split at ">>" - the closing marker
    decoder.push(r#"<<call ping {}>"#);
    decoder.push(">");

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "ping");
}

#[test]
fn json_with_double_angle_in_string_values() {
    let tools = vec![make_tool(
        "display",
        "Display text.",
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // The >> inside the JSON string value should be escaped as \u003e\u003e
    let text = r#"<<call display {"text":"Use \\u003e\\u003e for angle brackets"}>>"#;
    decoder.push(text);

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "display");
}

#[test]
fn multiple_incomplete_markers_are_buffered() {
    let tools = vec![make_tool(
        "search",
        "Search.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // Send partial marker that looks like it might be starting
    decoder.push("Text with < and <");
    decoder.push("< but then actual <<call search {\"query\":\"test\"}>>");

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "search");
}

#[test]
fn flush_returns_completed_calls_incrementally() {
    let tools = vec![make_tool(
        "ping",
        "Ping.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // First complete call
    decoder.push("<<call ping {}>>");
    let first_batch = decoder.flush();
    assert_eq!(first_batch.len(), 1);

    // Second complete call
    decoder.push(" then <<call ping {}>>");
    let second_batch = decoder.flush();
    assert_eq!(second_batch.len(), 1);

    // No more calls
    let remaining = decoder.finish().unwrap();
    assert_eq!(remaining.len(), 0);
}

#[test]
fn split_in_tool_name() {
    let tools = vec![make_tool(
        "search_web",
        "Search.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // Split in the middle of the tool name
    decoder.push(r#"<<call search_w"#);
    decoder.push(r#"eb {"query":"test"}>>"#);

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "search_web");
}

#[test]
fn split_exactly_after_call_keyword() {
    let tools = vec![make_tool(
        "ping",
        "Ping.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    // Split right after "call "
    decoder.push("<<call ");
    decoder.push("ping {}>>");

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "ping");
}

#[test]
fn empty_chunks_do_not_affect_decoding() {
    let tools = vec![make_tool(
        "ping",
        "Ping.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )];

    let compact = encode_tools(&tools).unwrap();
    let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

    decoder.push("");
    decoder.push("<<call ping ");
    decoder.push("");
    decoder.push("{}>>");
    decoder.push("");

    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "ping");
}
