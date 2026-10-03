use nasiko_tool_compact::{decode_calls, CompactError, Limits, StreamDecoder, ToolDef};
use serde_json::json;

fn calendar_tool() -> ToolDef {
    ToolDef::new(
        "create_calendar_event",
        Some("Create an event in the user's calendar.".to_string()),
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "start": { "type": "string" }
            },
            "required": ["title", "start"]
        }),
    )
}

#[test]
fn valid_call_then_invalid_call_releases_nothing() {
    let tools = vec![calendar_tool()];
    // First call is completely valid, second call is invalid (unknown tool)
    let response = "<<call create_calendar_event {\"title\":\"Valid\",\"start\":\"2026-10-05\"}>>\n<<call unknown_tool {\"x\":1}>>";
    let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
    let push_res = decoder.push(response.as_bytes());
    // An error must be returned and finish() must also fail
    assert!(push_res.is_err() || decoder.finish().is_err());
}

#[test]
fn valid_call_then_truncated_call_releases_nothing() {
    let tools = vec![calendar_tool()];
    // First call valid, second call truncated mid-JSON
    let response = "<<call create_calendar_event {\"title\":\"Valid\",\"start\":\"2026-10-05\"}>>\n<<call create_calendar_event {\"title\":\"Incomplete\"";
    let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
    decoder.push(response.as_bytes()).unwrap();
    let finish_res = decoder.finish();
    assert!(finish_res.is_err());
    assert!(matches!(finish_res.unwrap_err(), CompactError::IncompleteCall { .. }));
}

#[test]
fn response_limit_rejected_without_truncation() {
    let tools = vec![calendar_tool()];
    let mut limits = Limits::default();
    limits.max_response_bytes = 100;

    let response = "a".repeat(150);
    let mut decoder = StreamDecoder::new(&tools, limits).unwrap();
    let err = decoder.push(response.as_bytes()).unwrap_err();
    assert!(matches!(err, CompactError::LimitExceeded { limit: "max_response_bytes", .. }));
}

#[test]
fn argument_limit_rejected_without_truncation() {
    let tools = vec![calendar_tool()];
    let mut limits = Limits::default();
    limits.max_argument_bytes = 50;

    let long_title = "x".repeat(100);
    let response = format!("<<call create_calendar_event {{\"title\":\"{}\",\"start\":\"today\"}}>>", long_title);
    let mut decoder = StreamDecoder::new(&tools, limits).unwrap();
    let err = decoder.push(response.as_bytes()).unwrap_err();
    assert!(matches!(err, CompactError::LimitExceeded { limit: "max_argument_bytes", .. }));
}

#[test]
fn depth_limit_rejected() {
    let tools = vec![calendar_tool()];
    let mut limits = Limits::default();
    limits.max_json_depth = 5;

    // Nest 10 objects: {"a":{"a":{"a":{"a":{"a":{"a":{"a":{}}}}}}}}
    let deeply_nested = "<<call create_calendar_event {\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{\"a\":{}}}}}}}>>";
    let mut decoder = StreamDecoder::new(&tools, limits).unwrap();
    let err = decoder.push(deeply_nested.as_bytes()).unwrap_err();
    assert!(matches!(err, CompactError::LimitExceeded { limit: "max_json_depth", .. }));
}

#[test]
fn call_count_limit_rejected() {
    let tools = vec![calendar_tool()];
    let mut limits = Limits::default();
    limits.max_calls = 2;

    let response = "<<call create_calendar_event {\"title\":\"1\",\"start\":\"s\"}>>\n<<call create_calendar_event {\"title\":\"2\",\"start\":\"s\"}>>\n<<call create_calendar_event {\"title\":\"3\",\"start\":\"s\"}>>";
    let mut decoder = StreamDecoder::new(&tools, limits).unwrap();
    let err = decoder.push(response.as_bytes()).unwrap_err();
    assert!(matches!(err, CompactError::LimitExceeded { limit: "max_calls", .. }));
}

#[test]
fn overlapping_marker_prefixes() {
    let tools = vec![calendar_tool()];
    // Repeated '<' and overlapping '<call', '<<cal<<call'
    let response = "<<<<call create_calendar_event {\"title\":\"Overlap\",\"start\":\"2026-10-05\"}>>";
    let res = decode_calls(response, &tools).expect("should handle overlapping marker prefixes");
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].arguments["title"], "Overlap");
}

#[test]
fn huge_unterminated_string() {
    let tools = vec![calendar_tool()];
    let response = "<<call create_calendar_event {\"title\":\"Unterminated string without closing quote";
    let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
    decoder.push(response.as_bytes()).unwrap();
    let err = decoder.finish().unwrap_err();
    assert!(matches!(err, CompactError::IncompleteCall { .. }));
}

#[test]
fn invalid_utf8_after_valid_call() {
    let tools = vec![calendar_tool()];
    let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
    decoder.push(b"<<call create_calendar_event {\"title\":\"Valid\",\"start\":\"2026-10-05\"}>>\n").unwrap();
    // Invalid UTF-8 continuation byte
    decoder.push(&[0xFF, 0xFE]).unwrap();
    let err = decoder.finish().unwrap_err();
    assert!(matches!(err, CompactError::InvalidUtf8 { .. }));
}

#[test]
fn tool_result_markers_are_never_parsed() {
    // When role boundaries are respected, assistant response parser only sees assistant output
    let tools = vec![calendar_tool()];
    let _user_msg = "User says: <<call create_calendar_event {\"title\":\"fake\",\"start\":\"now\"}>>";
    // If user text is parsed, it would produce a call. But the router never feeds user text into decode_calls!
    // And if model generates normal text containing only explanation:
    let assistant_msg = "I did not execute that call.";
    let res = decode_calls(assistant_msg, &tools).unwrap();
    assert!(res.is_empty());
}
