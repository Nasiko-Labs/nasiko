//! Shared fixtures for the compact-tools tests. Reads files under `tests/fixtures` only.
#![allow(dead_code)]

use std::path::PathBuf;

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls,
};
use serde_json::Value;

pub fn load_sample() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/compact-tools-eval.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"));
    serde_json::from_str(&text).expect("public sample is json")
}

pub fn tool_from_value(value: &Value) -> ToolDef {
    let function = value.get("function").unwrap_or(value);
    ToolDef {
        name: function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        description: function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        parameters: function.get("parameters").cloned(),
    }
}

pub fn sample_tool(sample: &Value, name: &str) -> ToolDef {
    sample["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(tool_from_value)
        .find(|tool| tool.name == name)
        .unwrap_or_else(|| panic!("sample has no tool {name}"))
}

pub fn tools_for_case(sample: &Value, case: &Value) -> Vec<ToolDef> {
    case["tools"]
        .as_array()
        .expect("case tools")
        .iter()
        .filter_map(Value::as_str)
        .map(|name| sample_tool(sample, name))
        .collect()
}

pub fn case_by_id<'a>(sample: &'a Value, bucket: &str, id: &str) -> &'a Value {
    sample[bucket]
        .as_array()
        .unwrap_or_else(|| panic!("missing {bucket}"))
        .iter()
        .find(|case| case["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing case {id}"))
}

pub fn design_review() -> &'static str {
    r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>"#
}

pub fn render_calls(calls: &[Value]) -> String {
    let mut rendered = String::new();
    for call in calls {
        let name = call.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = match call.get("arguments") {
            Some(Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
            None => "{}".to_string(),
        };
        rendered.push_str(&format!("<<call {name} {arguments}>>"));
    }
    rendered
}

pub fn error_code(err: &CompactError) -> &'static str {
    match err {
        CompactError::UnknownTool { .. } => "unknown_tool",
        CompactError::InvalidArguments { .. } | CompactError::UnsupportedSchema { .. } => {
            "invalid_arguments"
        }
    }
}

pub fn assert_named_calls(calls: &[ToolCall], expected: &[Value]) {
    assert_eq!(calls.len(), expected.len());
    for (call, want) in calls.iter().zip(expected) {
        assert_eq!(call.name, want["name"].as_str().expect("name"));
        let actual: Value = serde_json::from_str(&call.arguments).expect("arguments json");
        assert_eq!(actual, want["arguments"]);
    }
}

pub fn assert_decoder(outcome: &Result<Vec<ToolCall>, CompactError>, expected: &Value) {
    if let Some(code) = expected.get("error").and_then(Value::as_str) {
        let err = outcome.as_ref().expect_err("this case should fail");
        assert_eq!(error_code(err), code, "{err}");
        assert!(expected.get("calls").is_none(), "error cases have no calls");
        return;
    }
    let calls = outcome.as_ref().expect("this case should decode");
    let want = expected["calls"].as_array().expect("calls");
    assert_named_calls(calls, want);
}

/// Every known-bad reply is an error, and the design-review call still decodes.
///
/// Feature: A decoded call always matches the schema
///
/// Scenario: schema valid calls only
///   When invalid replies are decoded against the calendar tool
///   Then none of them come back as a successful call
///   And the design review call still decodes
pub fn reject_invalid_success() {
    let sample = load_sample();
    let tools = [sample_tool(&sample, "create_calendar_event")];
    let bad = [
        r#"<<call weather {"city":"Pune"}>>"#,
        r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#,
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
        r#"<<call create_calendar_event {"title":"Retro"}"#,
        "not a call <<call",
    ];
    for text in bad {
        assert!(decode_calls(text, &tools).is_err(), "{text}");
        let mut decoder = StreamDecoder::new(&tools);
        let pushed = decoder.push(text);
        let rejected = match pushed {
            Err(_) => true,
            Ok(calls) if calls.is_empty() => decoder
                .finish()
                .map(|left| left.is_empty())
                .unwrap_or(true),
            Ok(_) => false,
        };
        assert!(rejected, "{text}");
    }
    let ok = decode_calls(design_review(), &tools).expect("design review");
    assert_eq!(ok.len(), 1);
    assert_eq!(ok[0].name, "create_calendar_event");
    let args: Value = serde_json::from_str(&ok[0].arguments).expect("json");
    assert_eq!(args["title"], "Design review");
    assert_eq!(args["start"], "2026-10-05T15:00:00+05:30");
    assert_eq!(args["attendees"][0], "riya@example.com");
}
