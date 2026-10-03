use nasiko_tool_compact::{DecodeError, ToolDef, decode_calls, strip_call_markers};
use serde_json::json;

fn sample_calendar_tool() -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "create_calendar_event",
            "description": "Create an event in the user's calendar.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Event title" },
                    "start": { "type": "string", "format": "date-time", "description": "Start time" },
                    "duration_min": { "type": "integer", "description": "Duration in minutes" },
                    "attendees": { "type": "array", "items": { "type": "string" } },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"]
            }
        }
    }))
    .unwrap()
}

fn sample_email_tool() -> ToolDef {
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
fn test_dc_001_valid_single_call() {
    let tools = vec![sample_calendar_tool()];
    let input = "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";

    let calls = decode_calls(input, &tools).expect("should decode valid call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "Design review");
    assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
}

#[test]
fn test_dc_003_delimiter_inside_string() {
    let tools = vec![sample_email_tool()];
    let input =
        "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>";

    let calls = decode_calls(input, &tools).expect("should handle >> inside string");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "send_email");

    let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["subject"], "a >> b");
    assert_eq!(args["to"], json!(["sam@example.com"]));
    assert_eq!(args["body"], "x");
}

#[test]
fn test_dc_004_unknown_tool_rejection() {
    let tools = vec![sample_calendar_tool()];
    let input = "<<call delete_everything {}>>";

    let err = decode_calls(input, &tools).expect_err("should reject unknown tool");
    assert_eq!(err.category(), "unknown_tool");
    match err {
        DecodeError::UnknownTool(name) => assert_eq!(name, "delete_everything"),
        _ => panic!("unexpected error type"),
    }
}

#[test]
fn test_dc_005_missing_required_and_bad_enum() {
    let tools = vec![sample_calendar_tool()];
    let input = "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>";

    let err = decode_calls(input, &tools).expect_err("should fail validation");
    assert_eq!(err.category(), "invalid_arguments");
}

#[test]
fn test_integer_vs_number_and_boolean() {
    let tools = vec![sample_calendar_tool()];

    // Float passed to integer field duration_min
    let input_float = "<<call create_calendar_event {\"title\":\"Meet\",\"start\":\"2026-10-05T15:00:00Z\",\"duration_min\":30.5}>>";
    let err = decode_calls(input_float, &tools).expect_err("should reject float for integer");
    assert_eq!(err.category(), "invalid_arguments");

    // Boolean passed to integer field duration_min
    let input_bool = "<<call create_calendar_event {\"title\":\"Meet\",\"start\":\"2026-10-05T15:00:00Z\",\"duration_min\":true}>>";
    let err = decode_calls(input_bool, &tools).expect_err("should reject boolean for integer");
    assert_eq!(err.category(), "invalid_arguments");
}

#[test]
fn test_null_value_rejected() {
    let tools = vec![sample_calendar_tool()];

    // Explicit null for required field title
    let input_null_req =
        "<<call create_calendar_event {\"title\":null,\"start\":\"2026-10-05T15:00:00Z\"}>>";
    let err = decode_calls(input_null_req, &tools).expect_err("should reject null for required");
    assert_eq!(err.category(), "invalid_arguments");

    // Explicit null for non-nullable optional field visibility
    let input_null_opt = "<<call create_calendar_event {\"title\":\"Meet\",\"start\":\"2026-10-05T15:00:00Z\",\"visibility\":null}>>";
    let err = decode_calls(input_null_opt, &tools)
        .expect_err("should reject null for non-nullable optional");
    assert_eq!(err.category(), "invalid_arguments");
}

#[test]
fn test_multiple_calls_sequential() {
    let tools = vec![sample_calendar_tool(), sample_email_tool()];
    let input = r#"
Sure, scheduling that and sending the email!
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>
All done!
"#;

    let calls = decode_calls(input, &tools).expect("should decode both calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].function.name, "create_calendar_event");

    let stripped = strip_call_markers(input);
    assert!(stripped.contains("Sure, scheduling that"));
    assert!(stripped.contains("All done!"));
    assert!(!stripped.contains("<<call"));
}

#[test]
fn test_no_calls_conversational() {
    let tools = vec![sample_calendar_tool()];
    let input = "The weather today is sunny with a high of 72 degrees.";

    let calls =
        decode_calls(input, &tools).expect("conversational response should yield empty calls");
    assert!(calls.is_empty());
}
