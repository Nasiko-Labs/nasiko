use nasiko_tool_compact::{decode_calls, decode_response, CompactError, ToolDef};
use serde_json::json;

fn calendar_tool() -> ToolDef {
    ToolDef::new(
        "create_calendar_event",
        Some("Create an event in the user's calendar.".to_string()),
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "start": { "type": "string", "format": "date-time" },
                "attendees": { "type": "array", "items": { "type": "string" } },
                "visibility": { "type": "string", "enum": ["public", "private"] }
            },
            "required": ["title", "start"]
        }),
    )
}

#[test]
fn decode_single_call() {
    let tools = vec![calendar_tool()];
    let response = "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";
    let calls = decode_calls(response, &tools).expect("should decode");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], "Design review");
    assert_eq!(calls[0].arguments["start"], "2026-10-05T15:00:00+05:30");
    assert_eq!(
        calls[0].arguments_json,
        "{\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}"
    );
}

#[test]
fn decode_multiple_calls_in_order() {
    let tool_a = ToolDef::new(
        "tool_a",
        None,
        json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
    );
    let tool_b = ToolDef::new(
        "tool_b",
        None,
        json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
    );
    let tools = vec![tool_a, tool_b];
    let response = "Starting batch:\n<<call tool_a {\"id\":1}>>\nProcessing next:\n<<call tool_b {\"name\":\"beta\"}>>\n<<call tool_a {\"id\":2}>>\nDone.";
    let res = decode_response(response, &tools).expect("should decode");
    assert_eq!(res.calls.len(), 3);
    assert_eq!(res.calls[0].name, "tool_a");
    assert_eq!(res.calls[0].arguments["id"], 1);
    assert_eq!(res.calls[1].name, "tool_b");
    assert_eq!(res.calls[1].arguments["name"], "beta");
    assert_eq!(res.calls[2].name, "tool_a");
    assert_eq!(res.calls[2].arguments["id"], 2);
    assert_eq!(res.text, "Starting batch:\n\nProcessing next:\n\n\nDone.");
}

#[test]
fn decode_plain_answer_has_no_calls() {
    let tools = vec![calendar_tool()];
    let response = "Hello! How can I assist you with your schedule today?";
    let res = decode_response(response, &tools).expect("should decode plain answer");
    assert!(res.calls.is_empty());
    assert_eq!(res.text, response);
}

#[test]
fn decode_preserves_surrounding_text() {
    let tools = vec![calendar_tool()];
    let response = "I'll create that event for you right away.\n<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>\nPlease confirm if this looks good.";
    let res = decode_response(response, &tools).expect("should decode");
    assert_eq!(res.calls.len(), 1);
    assert_eq!(
        res.text,
        "I'll create that event for you right away.\n\nPlease confirm if this looks good."
    );
}

#[test]
fn decode_marker_inside_string() {
    let tools = vec![calendar_tool()];
    let response = "<<call create_calendar_event {\"title\":\"Review >> planning with <<call fake>> text\",\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"private\"}>>";
    let calls = decode_calls(response, &tools).expect("should decode with marker inside string");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].arguments["title"],
        "Review >> planning with <<call fake>> text"
    );
}

#[test]
fn decode_escaped_quote_and_backslash() {
    let tools = vec![calendar_tool()];
    let response = r#"<<call create_calendar_event {"title":"Escaped \"quotes\" and \\backslash\\","start":"2026-10-05T15:00:00+05:30"}>>"#;
    let calls = decode_calls(response, &tools).expect("should decode escaped quote");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].arguments["title"],
        "Escaped \"quotes\" and \\backslash\\"
    );
}

#[test]
fn decode_rejects_unknown_tool() {
    let tools = vec![calendar_tool()];
    let response = "<<call nonexistent_tool {\"foo\":\"bar\"}>>";
    let err = decode_calls(response, &tools).unwrap_err();
    assert!(matches!(err, CompactError::UnknownTool { .. }));
    assert_eq!(err.public_eval_error(), "unknown_tool");
}

#[test]
fn decode_rejects_missing_required_argument() {
    let tools = vec![calendar_tool()];
    // Missing 'start'
    let response = "<<call create_calendar_event {\"title\":\"Just a title\"}>>";
    let err = decode_calls(response, &tools).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { .. }));
    assert_eq!(err.public_eval_error(), "invalid_arguments");
}

#[test]
fn decode_rejects_invalid_enum() {
    let tools = vec![calendar_tool()];
    // "secret" is not in ["public", "private"]
    let response = "<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\",\"visibility\":\"secret\"}>>";
    let err = decode_calls(response, &tools).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { .. }));
}

#[test]
fn decode_rejects_wrong_nested_type() {
    let tools = vec![calendar_tool()];
    // attendees must be array of strings, but given array of integers
    let response = "<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\",\"attendees\":[123]}>>";
    let err = decode_calls(response, &tools).unwrap_err();
    assert!(matches!(err, CompactError::InvalidArguments { .. }));
}

#[test]
fn decode_rejects_duplicate_key() {
    let tools = vec![calendar_tool()];
    let response = "<<call create_calendar_event {\"title\":\"First\",\"title\":\"Second\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>";
    let err = decode_calls(response, &tools).unwrap_err();
    assert!(matches!(err, CompactError::DuplicateKey { .. }));
    assert_eq!(err.public_eval_error(), "invalid_arguments");
}

#[test]
fn decode_rejects_unicode_equivalent_duplicate_key() {
    let tools = vec![calendar_tool()];
    // "start" vs "\u0073tart"
    let response = "<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\",\"\\u0073tart\":\"2026-10-05T11:00:00+05:30\"}>>";
    let err = decode_calls(response, &tools).unwrap_err();
    assert!(matches!(err, CompactError::DuplicateKey { .. }));
}

#[test]
fn decode_rejects_incomplete_call() {
    let tools = vec![calendar_tool()];
    // Incomplete call: missing closing >>
    let response = "<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\"}";
    let err = decode_calls(response, &tools).unwrap_err();
    assert!(matches!(err, CompactError::IncompleteCall { .. }));

    // Incomplete opener
    let response2 = "Some text <<ca";
    let err2 = decode_calls(response2, &tools).unwrap_err();
    assert!(matches!(err2, CompactError::IncompleteCall { .. }));

    // Unmatched single '<' should be ordinary text, not an error!
    let response3 = "Is 3 < 5?";
    let res3 = decode_response(response3, &tools).expect("single < is ordinary text");
    assert_eq!(res3.text, "Is 3 < 5?");
}

#[test]
fn decode_preserves_raw_argument_numbers() {
    let tool = ToolDef::new(
        "math_tool",
        None,
        json!({
            "type": "object",
            "properties": {
                "bignum": { "type": "number" },
                "precise": { "type": "number" }
            },
            "required": ["bignum", "precise"]
        }),
    );
    let response = "<<call math_tool {\"bignum\":9007199254740993,\"precise\":0.1234567890123456789}>>";
    let calls = decode_calls(response, &[tool]).expect("should decode");
    assert_eq!(
        calls[0].arguments_json,
        "{\"bignum\":9007199254740993,\"precise\":0.1234567890123456789}"
    );
}

#[test]
fn decode_rejects_unsupported_numeric_range() {
    let tool = ToolDef::new(
        "num_tool",
        None,
        json!({ "type": "object", "properties": { "val": { "type": "number" } }, "required": ["val"] }),
    );
    // Number with huge exponent causing overflow
    let response = "<<call num_tool {\"val\":1e99999999999999999999}>>";
    let err = decode_calls(response, &[tool]).unwrap_err();
    assert!(matches!(err, CompactError::NumericRange { .. }));
}
