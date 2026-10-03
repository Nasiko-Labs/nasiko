use nasiko_tool_compact::{FunctionDef, ToolCompactError, ToolDef, decode_calls};
use serde_json::{Value, json};

fn sample_tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new(FunctionDef {
            name: "create_calendar_event".into(),
            description: Some("Create a calendar event".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "duration_min": { "type": "integer" },
                    "attendees": { "type": "array", "items": { "type": "string" } },
                    "visibility": { "type": "string", "enum": ["public", "private"] },
                    "details": {
                        "type": "object",
                        "properties": {
                            "location": { "type": "string" }
                        },
                        "required": ["location"]
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            })),
        }),
        ToolDef::new(FunctionDef {
            name: "send_email".into(),
            description: Some("Send an email".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": { "type": "array", "items": { "type": "string" } },
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["to", "subject"]
            })),
        }),
        ToolDef::new(FunctionDef {
            name: "ping".into(),
            description: Some("Ping check".into()),
            parameters: None,
        }),
    ]
}

// 1. Single tool call
#[test]
fn test_decode_single_tool_call() {
    let tools = sample_tools();
    let text = r#"Certainly! <<call create_calendar_event {"title": "Design Review", "duration_min": 45}>> Done."#;
    let calls = decode_calls(text, &tools).unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].kind, "function");
    assert_eq!(calls[0].function.name, "create_calendar_event");

    let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "Design Review");
    assert_eq!(args["duration_min"], 45);
}

// 2. Multiple tool calls
#[test]
fn test_decode_multiple_tool_calls() {
    let tools = sample_tools();
    let text = r#"
    I will send an email and book the event:
    <<call send_email {"to": ["alice@example.com"], "subject": "Meeting"}>>
    And now the event:
    <<call create_calendar_event {"title": "Team Sync", "duration_min": 30}>>
    "#;
    let calls = decode_calls(text, &tools).unwrap();

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].id, "call_2");
    assert_eq!(calls[1].function.name, "create_calendar_event");
}

// 3. Unknown tool
#[test]
fn test_decode_unknown_tool() {
    let tools = sample_tools();
    let text = r#"<<call delete_database {"force": true}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.as_code(), "unknown_tool");

    match err {
        ToolCompactError::UnknownTool(name) => {
            assert_eq!(name, "delete_database");
        }
        other => panic!("expected UnknownTool, got {:?}", other),
    }
}

// 4. Missing required argument
#[test]
fn test_decode_missing_required_argument() {
    let tools = sample_tools();
    // 'title' is required for create_calendar_event
    let text = r#"<<call create_calendar_event {"duration_min": 30}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.as_code(), "invalid_arguments");

    match err {
        ToolCompactError::MissingRequiredField { tool, field } => {
            assert_eq!(tool, "create_calendar_event");
            assert_eq!(field, "title");
        }
        other => panic!("expected MissingRequiredField, got {:?}", other),
    }
}

// 5. Invalid enum value
#[test]
fn test_decode_invalid_enum_value() {
    let tools = sample_tools();
    // 'visibility' must be "public" or "private"
    let text = r#"<<call create_calendar_event {"title": "Retro", "visibility": "confidential"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.as_code(), "invalid_arguments");

    match err {
        ToolCompactError::InvalidEnumValue {
            tool,
            field,
            value,
            allowed,
        } => {
            assert_eq!(tool, "create_calendar_event");
            assert_eq!(field, "visibility");
            assert_eq!(value, "confidential");
            assert_eq!(allowed, vec!["public", "private"]);
        }
        other => panic!("expected InvalidEnumValue, got {:?}", other),
    }
}

// 6. Incorrect argument type
#[test]
fn test_decode_incorrect_argument_type() {
    let tools = sample_tools();
    // 'duration_min' must be an integer, not a string
    let text = r#"<<call create_calendar_event {"title": "Retro", "duration_min": "30 mins"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.as_code(), "invalid_arguments");

    match err {
        ToolCompactError::InvalidArguments { tool, details } => {
            assert_eq!(tool, "create_calendar_event");
            assert!(details.contains("expected type 'integer'"));
        }
        other => panic!("expected InvalidArguments, got {:?}", other),
    }
}

// 7. Malformed JSON
#[test]
fn test_decode_malformed_json() {
    let tools = sample_tools();
    // JSON syntax error inside call arguments
    let text = r#"<<call create_calendar_event {"title": "Retro", "duration_min": }>>"#;
    let err = decode_calls(text, &tools).unwrap_err();

    match err {
        ToolCompactError::InvalidArguments { tool, details } => {
            assert_eq!(tool, "create_calendar_event");
            assert!(details.contains("malformed JSON arguments"));
        }
        other => panic!("expected InvalidArguments, got {:?}", other),
    }
}

// 8. Malformed call markers
#[test]
fn test_decode_malformed_call_markers() {
    let tools = sample_tools();

    // Case A: Missing closing '>>'
    let unclosed_marker = r#"<<call create_calendar_event {"title": "Retro"}"#;
    assert!(decode_calls(unclosed_marker, &tools).is_err());

    // Case B: Missing opening '{'
    let missing_brace = r#"<<call create_calendar_event "title": "Retro">>"#;
    assert!(decode_calls(missing_brace, &tools).is_err());

    // Case C: Missing tool name
    let missing_name = r#"<<call {"title": "Retro"}>>"#;
    assert!(decode_calls(missing_name, &tools).is_err());

    // Case D: Unclosed JSON brace
    let unclosed_brace = r#"<<call create_calendar_event {"title": "Retro">> "#;
    assert!(decode_calls(unclosed_brace, &tools).is_err());
}

// 9. Empty arguments
#[test]
fn test_decode_empty_arguments() {
    let tools = sample_tools();
    // 'ping' has no parameters
    let text = r#"<<call ping {}>>"#;
    let calls = decode_calls(text, &tools).unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "ping");
    assert_eq!(calls[0].function.arguments, "{}");
}

// 10. Unknown additional properties
#[test]
fn test_decode_disallowed_additional_properties() {
    let tools = sample_tools();
    // 'create_calendar_event' has "additionalProperties": false
    let text = r#"<<call create_calendar_event {"title": "Planning", "random_extra_param": 123}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();

    match err {
        ToolCompactError::InvalidArguments { tool, details } => {
            assert_eq!(tool, "create_calendar_event");
            assert!(details.contains("unexpected argument 'random_extra_param'"));
        }
        other => panic!("expected InvalidArguments, got {:?}", other),
    }
}

// 11. Escaped '>>' inside string argument
#[test]
fn test_decode_escaped_delimiters_inside_string() {
    let tools = sample_tools();
    let text = r#"<<call create_calendar_event {"title": "A >> B discussion with \"quotes\" inside"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();

    assert_eq!(calls.len(), 1);
    let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "A >> B discussion with \"quotes\" inside");
}

// 12. Nested object validation
#[test]
fn test_decode_nested_object_validation() {
    let tools = sample_tools();
    // 'details.location' is required when 'details' is provided
    let text_missing_nested = r#"<<call create_calendar_event {"title": "Planning", "details": {}}>>"#;
    let err = decode_calls(text_missing_nested, &tools).unwrap_err();
    match err {
        ToolCompactError::MissingRequiredField { field, .. } => {
            assert_eq!(field, "location");
        }
        other => panic!("expected MissingRequiredField, got {:?}", other),
    }

    // Valid nested call
    let text_valid_nested = r#"<<call create_calendar_event {"title": "Planning", "details": {"location": "Room 402"}}>>"#;
    let calls = decode_calls(text_valid_nested, &tools).unwrap();
    assert_eq!(calls.len(), 1);
}
