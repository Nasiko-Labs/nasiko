use nasiko_tool_compact::{
    CALL_INSTRUCTION, FunctionDef, ToolCompactError, ToolDef, decode_tools, encode_tools,
};
use serde_json::json;

fn calendar_tool() -> ToolDef {
    ToolDef::new(FunctionDef {
        name: "create_calendar_event".into(),
        description: Some("Create an event in the user's calendar.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Event title" },
                "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
                "duration_min": { "type": "integer", "description": "Duration in minutes" },
                "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
                "visibility": { "type": "string", "enum": ["public", "private"] }
            },
            "required": ["title", "start"]
        })),
    })
}

fn email_tool() -> ToolDef {
    ToolDef::new(FunctionDef {
        name: "send_email".into(),
        description: Some("Send an email to recipients.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": { "type": "array", "items": { "type": "string" } },
                "subject": { "type": "string" },
                "body": { "type": "string" }
            },
            "required": ["to", "subject", "body"]
        })),
    })
}

#[test]
fn test_simple_tool_encoding() {
    let tool = ToolDef::new(FunctionDef {
        name: "ping".into(),
        description: Some("Ping the server".into()),
        parameters: None,
    });

    let compact = encode_tools(&[tool]).unwrap();
    assert_eq!(
        compact.prompt_text,
        format!("ping() - Ping the server\n{CALL_INSTRUCTION}")
    );
    assert_eq!(compact.tools.len(), 1);
}

#[test]
fn test_multiple_tools_encoding() {
    let tools = vec![calendar_tool(), email_tool()];
    let compact = encode_tools(&tools).unwrap();

    assert_eq!(compact.tools.len(), 2);
    let lines: Vec<&str> = compact.prompt_text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("create_calendar_event("));
    assert!(lines[1].starts_with("send_email("));
    assert_eq!(lines[2], CALL_INSTRUCTION);
}

#[test]
fn test_nested_objects_and_arrays() {
    let tool = ToolDef::new(FunctionDef {
        name: "deploy_service".into(),
        description: Some("Deploy a microservice.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "service": { "type": "string" },
                "config": {
                    "type": "object",
                    "properties": {
                        "replicas": { "type": "integer" },
                        "env": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["replicas"]
                }
            },
            "required": ["service", "config"]
        })),
    });

    let compact = encode_tools(&[tool]).unwrap();
    assert!(compact.prompt_text.contains("deploy_service(config:{env?:[str], replicas:int}, service:str) - Deploy a microservice."));
}

#[test]
fn test_deterministic_output() {
    let tools = vec![calendar_tool(), email_tool()];

    let run1 = encode_tools(&tools).unwrap();
    let run2 = encode_tools(&tools).unwrap();

    assert_eq!(run1.prompt_text, run2.prompt_text);
    assert_eq!(run1.compact_bytes, run2.compact_bytes);
    assert_eq!(run1.tools, run2.tools);
}

#[test]
fn test_invalid_parameters_non_object() {
    let tool = ToolDef::new(FunctionDef {
        name: "bad_tool".into(),
        description: None,
        parameters: Some(json!(42)),
    });

    let err = encode_tools(&[tool]).unwrap_err();
    match err {
        ToolCompactError::SchemaError(msg) => {
            assert!(msg.contains("must be a JSON object"));
        }
        other => panic!("expected SchemaError, got {:?}", other),
    }
}

#[test]
fn test_invalid_parameters_wrong_type() {
    let tool = ToolDef::new(FunctionDef {
        name: "bad_tool".into(),
        description: None,
        parameters: Some(json!({
            "type": "array"
        })),
    });

    let err = encode_tools(&[tool]).unwrap_err();
    match err {
        ToolCompactError::SchemaError(msg) => {
            assert!(msg.contains("must have type 'object'"));
        }
        other => panic!("expected SchemaError, got {:?}", other),
    }
}

#[test]
fn test_empty_tool_name_error() {
    let tool = ToolDef::new(FunctionDef {
        name: "   ".into(),
        description: None,
        parameters: None,
    });

    let err = encode_tools(&[tool]).unwrap_err();
    match err {
        ToolCompactError::SchemaError(msg) => {
            assert!(msg.contains("cannot be empty"));
        }
        other => panic!("expected SchemaError, got {:?}", other),
    }
}

#[test]
fn test_schema_preservation_for_decoder() {
    let tools = vec![calendar_tool(), email_tool()];
    let compact = encode_tools(&tools).unwrap();

    // Verify all tools survived intact in compact.tools
    let decoded = decode_tools(&compact).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].function.name, "create_calendar_event");
    assert_eq!(decoded[1].function.name, "send_email");

    // Verify the schemas are byte-identical to original parameters
    assert_eq!(
        decoded[0].function.parameters,
        tools[0].function.parameters
    );
    assert_eq!(
        decoded[1].function.parameters,
        tools[1].function.parameters
    );
}
