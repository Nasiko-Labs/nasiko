use nasiko_tool_compact::{CompactError, ToolDef, encode_tools};
use serde_json::json;

fn calendar_tool() -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "create_calendar_event",
            "description": "Create an event in the user's calendar.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Event title" },
                    "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
                    "duration_min": { "type": "integer", "description": "Duration in minutes" },
                    "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"]
            }
        }
    }))
    .unwrap()
}

fn email_tool() -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "send_email",
            "description": "Send an email from the user's account.",
            "parameters": {
                "type": "object",
                "properties": {
                    "to": { "type": "array", "items": { "type": "string" } },
                    "subject": { "type": "string" },
                    "body": { "type": "string" },
                    "cc": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["to", "subject", "body"]
            }
        }
    }))
    .unwrap()
}

#[test]
fn test_encode_tools_public_dataset() {
    let tools = vec![calendar_tool(), email_tool()];
    let compact = encode_tools(&tools).expect("should encode successfully");

    assert_eq!(
        compact.tool_names,
        vec!["create_calendar_event", "send_email"]
    );
    assert!(compact.schema_dsl.contains("create_calendar_event("));
    assert!(compact.schema_dsl.contains("title:str"));
    assert!(compact.schema_dsl.contains("start:datetime"));
    assert!(compact.schema_dsl.contains("visibility?:public|private"));
    assert!(compact.schema_dsl.contains("attendees?:[str]"));
    assert!(compact.schema_dsl.contains("send_email("));
    assert!(compact.schema_dsl.contains("to:[str]"));
    assert!(compact.instruction.contains("<<call name {json args}>>"));
    assert!(compact.prompt.contains(&compact.schema_dsl));
    assert!(compact.prompt.contains(&compact.instruction));
}

#[test]
fn test_unsupported_schema_one_of() {
    let tool: ToolDef = serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "one_of_tool",
            "parameters": {
                "type": "object",
                "oneOf": [{ "type": "string" }, { "type": "integer" }]
            }
        }
    }))
    .unwrap();

    let err = encode_tools(&[tool]).expect_err("should reject oneOf");
    match err {
        CompactError::UnsupportedSchema(kw) => assert_eq!(kw, "oneOf"),
        _ => panic!("expected UnsupportedSchema(oneOf)"),
    }
}

#[test]
fn test_unsupported_schema_any_of() {
    let tool: ToolDef = serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "any_of_tool",
            "parameters": {
                "type": "object",
                "properties": {
                    "field": {
                        "anyOf": [{ "type": "string" }, { "type": "integer" }]
                    }
                }
            }
        }
    }))
    .unwrap();

    let err = encode_tools(&[tool]).expect_err("should reject anyOf");
    match err {
        CompactError::UnsupportedSchema(kw) => assert_eq!(kw, "anyOf"),
        _ => panic!("expected UnsupportedSchema(anyOf)"),
    }
}

#[test]
fn test_unsupported_schema_all_of() {
    let tool: ToolDef = serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "all_of_tool",
            "parameters": {
                "type": "object",
                "allOf": [{ "type": "object" }]
            }
        }
    }))
    .unwrap();

    let err = encode_tools(&[tool]).expect_err("should reject allOf");
    match err {
        CompactError::UnsupportedSchema(kw) => assert_eq!(kw, "allOf"),
        _ => panic!("expected UnsupportedSchema(allOf)"),
    }
}

#[test]
fn test_unsupported_schema_ref() {
    let tool: ToolDef = serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "ref_tool",
            "parameters": {
                "type": "object",
                "$ref": "#/definitions/CustomType"
            }
        }
    }))
    .unwrap();

    let err = encode_tools(&[tool]).expect_err("should reject $ref");
    match err {
        CompactError::UnsupportedSchema(kw) => assert_eq!(kw, "$ref"),
        _ => panic!("expected UnsupportedSchema($ref)"),
    }
}

#[test]
fn test_unsupported_schema_pattern_properties() {
    let tool: ToolDef = serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "pattern_tool",
            "parameters": {
                "type": "object",
                "patternProperties": {
                    "^[a-z]+$": { "type": "string" }
                }
            }
        }
    }))
    .unwrap();

    let err = encode_tools(&[tool]).expect_err("should reject patternProperties");
    match err {
        CompactError::UnsupportedSchema(kw) => assert_eq!(kw, "patternProperties"),
        _ => panic!("expected UnsupportedSchema(patternProperties)"),
    }
}

#[test]
fn test_empty_tools() {
    let err = encode_tools(&[]).expect_err("should reject empty tools");
    assert_eq!(err, CompactError::EmptyTools);
}
