use nasiko_tool_compact::{
    decode_calls,
    encode_tools,
    FunctionDef,
    ToolDef,
};
use serde_json::{json, Map, Value};

fn tool(name: &str, description: &str, parameters: Value) -> ToolDef {
    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: name.to_string(),
            description: Some(description.to_string()),
            parameters: Some(parameters),
        },
        extra: Map::new(),
    }
}

#[test]
fn encodes_required_and_optional_fields() {
    let tools = vec![tool(
        "create_event",
        "Create an event",
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "duration_min": {"type": "integer"}
            },
            "required": ["title"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();

    assert_eq!(
        compact.text,
        "create_event(title:str, duration_min?:int) - Create an event"
    );
}

#[test]
fn encodes_enum() {
    let tools = vec![tool(
        "set_visibility",
        "Set visibility",
        json!({
            "type": "object",
            "properties": {
                "visibility": {
                    "type": "string",
                    "enum": ["public", "private"]
                }
            },
            "required": ["visibility"]
        }),
    )];

    let compact = encode_tools(&tools).unwrap();

    assert_eq!(
        compact.text,
        "set_visibility(visibility:public|private) - Set visibility"
    );
}

#[test]
fn decodes_simple_call() {
    let tools = vec![tool(
        "create_event",
        "Create an event",
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"}
            },
            "required": ["title"]
        }),
    )];

    let response = r#"Sure! <<call create_event {"title":"Hackathon"}>>"#;

    let calls = decode_calls(response, &tools).unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_event");
    assert_eq!(
        calls[0].function.arguments,
        r#"{"title":"Hackathon"}"#
    );
}

#[test]
fn rejects_unknown_tool() {
    let tools = vec![tool(
        "create_event",
        "Create an event",
        json!({
            "type": "object",
            "properties": {}
        }),
    )];

    let response = r#"<<call delete_everything {"x":1}>>"#;

    let result = decode_calls(response, &tools);

    assert!(result.is_err());
}

#[test]
fn handles_terminator_inside_json_string() {
    let tools = vec![tool(
        "send_message",
        "Send a message",
        json!({
            "type": "object",
            "properties": {
                "message": {"type": "string"}
            },
            "required": ["message"]
        }),
    )];

    let response = r#"<<call send_message {"message":"hello >> world"}>>"#;

    let calls = decode_calls(response, &tools).unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "send_message");
}

#[test]
fn decodes_multiple_calls() {
    let tools = vec![
        tool(
            "first",
            "First tool",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
        tool(
            "second",
            "Second tool",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
    ];

    let response = r#"<<call first {}>> text <<call second {}>>"#;

    let calls = decode_calls(response, &tools).unwrap();

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "first");
    assert_eq!(calls[1].function.name, "second");
}
#[test]
fn rejects_missing_required_field() {
    let tools = vec![tool(
        "create_event",
        "Create an event",
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "duration_min": {"type": "integer"}
            },
            "required": ["title"]
        }),
    )];

    let response = r#"<<call create_event {"duration_min":30}>>"#;

    let result = decode_calls(response, &tools);

    assert!(result.is_err());
}

#[test]
fn rejects_invalid_enum_value() {
    let tools = vec![tool(
        "set_visibility",
        "Set visibility",
        json!({
            "type": "object",
            "properties": {
                "visibility": {
                    "type": "string",
                    "enum": ["public", "private"]
                }
            },
            "required": ["visibility"]
        }),
    )];

    let response = r#"<<call set_visibility {"visibility":"secret"}>>"#;

    let result = decode_calls(response, &tools);

    assert!(result.is_err());
}

#[test]
fn rejects_invalid_argument_type() {
    let tools = vec![tool(
        "create_event",
        "Create an event",
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"}
            },
            "required": ["title"]
        }),
    )];

    let response = r#"<<call create_event {"title":123}>>"#;

    let result = decode_calls(response, &tools);

    assert!(result.is_err());
}
#[test]
fn stream_decoder_handles_split_marker() {
    use nasiko_tool_compact::StreamDecoder;

    let tools = vec![tool(
        "create_event",
        "Create an event",
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"}
            },
            "required": ["title"]
        }),
    )];

    let mut decoder = StreamDecoder::new();

    let first = decoder
        .push("Some text <<cal", &tools)
        .unwrap();

    assert!(first.is_empty());

    let second = decoder
        .push(r#"l create_event {"title":"Hackathon"}>>"#, &tools)
        .unwrap();

    assert_eq!(second.len(), 1);
    assert_eq!(second[0].function.name, "create_event");
}
