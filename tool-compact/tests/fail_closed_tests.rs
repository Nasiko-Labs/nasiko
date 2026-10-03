use nasiko_tool_compact::{
    decode_call, validate_call, CompactStreamDecoder, ParameterSchema, StreamEvent,
    ToolCompactError, ToolRegistry, ToolSchema, ValueType,
};
use serde_json::json;

fn setup_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(
        ToolSchema::new("search", "Search database")
            .with_parameter(ParameterSchema::new("query", ValueType::String, true))
            .with_parameter(ParameterSchema::new("limit", ValueType::Integer, false).with_default(json!(10))),
    );
    reg
}

#[test]
fn test_unknown_tool_fail_closed() {
    let registry = setup_registry();
    let res = validate_call("nonexistent_tool", &json!({"query": "test"}), &registry);

    match res {
        Err(ToolCompactError::UnknownTool(name)) => {
            assert_eq!(name, "nonexistent_tool");
        }
        other => panic!("Expected UnknownTool error, got {:?}", other),
    }
}

#[test]
fn test_unknown_tool_stream_fail_closed() {
    let registry = setup_registry();
    let mut decoder = CompactStreamDecoder::with_registry(registry);

    let events = decoder.feed_chunk("unknown_tool(arg=");
    assert_eq!(events.len(), 1);
    match &events[0] {
        StreamEvent::Error(ToolCompactError::UnknownTool(name)) => {
            assert_eq!(name, "unknown_tool");
        }
        other => panic!("Expected stream UnknownTool error, got {:?}", other),
    }
}

#[test]
fn test_missing_required_argument_fail_closed() {
    let registry = setup_registry();
    let (tool_name, args) = decode_call("search(limit=5)").unwrap();
    let res = validate_call(&tool_name, &args, &registry);

    match res {
        Err(ToolCompactError::InvalidArguments { tool, reasoning }) => {
            assert_eq!(tool, "search");
            assert!(reasoning.contains("Missing required parameter 'query'"));
        }
        other => panic!("Expected InvalidArguments error, got {:?}", other),
    }
}

#[test]
fn test_type_mismatch_argument_fail_closed() {
    let registry = setup_registry();
    let (tool_name, args) = decode_call("search(query=12345)").unwrap();
    let res = validate_call(&tool_name, &args, &registry);

    match res {
        Err(ToolCompactError::InvalidArguments { tool, reasoning }) => {
            assert_eq!(tool, "search");
            assert!(reasoning.contains("expected type string"));
        }
        other => panic!("Expected InvalidArguments error, got {:?}", other),
    }
}

#[test]
fn test_malformed_syntax_fail_closed() {
    let res = decode_call("search(query=\"unclosed string)");
    match res {
        Err(ToolCompactError::Malformed(msg)) => {
            assert!(msg.contains("Unclosed string"));
        }
        other => panic!("Expected Malformed error, got {:?}", other),
    }
}
