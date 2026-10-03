use nasiko_tool_compact::{ToolCall, ToolDef, decode_calls, render_calls};
use serde_json::{Map, Value};

#[test]
fn decode_rendered_single_call_roundtrips() {
    let tools = vec![ToolDef {
        name: "create_calendar_event".to_string(),
        description: Some("Create an event in the user's calendar.".to_string()),
        parameters: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "start": { "type": "string", "format": "date-time" },
                "attendees": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["title", "start"]
        })),
    }];

    let arguments = Map::from_iter([
        (
            "title".to_string(),
            Value::String("Design review".to_string()),
        ),
        (
            "start".to_string(),
            Value::String("2026-10-05T15:00:00+05:30".to_string()),
        ),
        (
            "attendees".to_string(),
            Value::Array(vec![Value::String("riya@example.com".to_string())]),
        ),
    ]);
    let calls = vec![ToolCall {
        name: "create_calendar_event".to_string(),
        arguments,
    }];

    let rendered = render_calls(&calls);
    let decoded = decode_calls(&rendered, &tools).unwrap();
    assert_eq!(decoded, calls);
}

#[test]
fn decode_rendered_multiple_calls_roundtrips() {
    let tools = vec![
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "start": { "type": "string", "format": "date-time" },
                    "duration_min": { "type": "integer" },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"]
            })),
        },
        ToolDef {
            name: "send_email".to_string(),
            description: Some("Send an email from the user's account.".to_string()),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "to": { "type": "array", "items": { "type": "string" } },
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["to", "subject", "body"]
            })),
        },
    ];

    let rendered = "<<call send_email {\"body\":\"The build is green.\",\"subject\":\"Build status\",\"to\":[\"sam@example.com\"]}>>\n<<call create_calendar_event {\"duration_min\":30,\"start\":\"2026-10-04T10:00:00+05:30\",\"title\":\"Retro\",\"visibility\":\"private\"}>>";
    let decoded = decode_calls(rendered, &tools).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].name, "send_email");
    assert_eq!(decoded[1].name, "create_calendar_event");
}

#[test]
fn stream_decoder_preserves_call_marker_split_across_chunks() {
    use nasiko_tool_compact::{StreamDecoder, StreamEvent};

    let tools = vec![ToolDef {
        name: "create_calendar_event".to_string(),
        description: None,
        parameters: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "start": { "type": "string", "format": "date-time" }
            },
            "required": ["title", "start"]
        })),
    }];
    let chunks = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let mut decoder = StreamDecoder::new(&tools).unwrap();
    let mut calls = Vec::new();
    for chunk in chunks {
        for event in decoder.push(chunk).unwrap() {
            if let StreamEvent::Call { call, .. } = event {
                calls.push(call);
            }
        }
    }
    for event in decoder.finish().unwrap() {
        if let StreamEvent::Call { call, .. } = event {
            calls.push(call);
        }
    }

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], "Retro");
}
