use nasiko_tool_compact::{CompactStreamDecoder, ParameterSchema, StreamEvent, ToolRegistry, ToolSchema, ValueType};

#[test]
fn test_split_chunks_streaming() {
    let mut registry = ToolRegistry::new();
    registry.register(
        ToolSchema::new("get_weather", "Weather tool")
            .with_parameter(ParameterSchema::new("location", ValueType::String, true))
            .with_parameter(ParameterSchema::new("units", ValueType::String, false)),
    );

    let mut decoder = CompactStreamDecoder::with_registry(registry);

    // Split input across multiple tiny chunks
    let chunks = vec![
        "get_",
        "weather",
        "(location=",
        "\"New York\"",
        ", units=",
        "\"celsius\")",
    ];

    let mut started = false;
    let mut completed = false;

    for chunk in chunks {
        let events = decoder.feed_chunk(chunk);
        for event in events {
            match event {
                StreamEvent::ToolStarted { name } => {
                    assert_eq!(name, "get_weather");
                    started = true;
                }
                StreamEvent::CallComplete { name, args } => {
                    assert_eq!(name, "get_weather");
                    assert_eq!(args["location"], "New York");
                    assert_eq!(args["units"], "celsius");
                    completed = true;
                }
                StreamEvent::Error(err) => {
                    panic!("Unexpected streaming error: {:?}", err);
                }
            }
        }
    }

    assert!(started, "Should have emitted ToolStarted");
    assert!(completed, "Should have emitted CallComplete");
}

#[test]
fn test_single_byte_chunk_streaming() {
    let input = r#"get_weather(location="London")"#;
    let mut registry = ToolRegistry::new();
    registry.register(
        ToolSchema::new("get_weather", "Weather tool")
            .with_parameter(ParameterSchema::new("location", ValueType::String, true)),
    );

    let mut decoder = CompactStreamDecoder::with_registry(registry);
    let mut completed = false;

    for c in input.chars() {
        let buf = c.to_string();
        let events = decoder.feed_chunk(&buf);
        for event in events {
            if let StreamEvent::CallComplete { name, args } = event {
                assert_eq!(name, "get_weather");
                assert_eq!(args["location"], "London");
                completed = true;
            }
        }
    }

    assert!(completed, "Single byte streaming should complete successfully");
}
