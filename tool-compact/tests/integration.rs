//! Integration tests for encode / decode / stream / round-trip.

use nasiko_tool_compact::{
    StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools, render_call,
    render_calls, split_like,
};
use serde_json::json;

fn calendar() -> ToolDef {
    ToolDef {
        name: "create_calendar_event".into(),
        description: Some("Create an event in the user's calendar.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Event title" },
                "start": { "type": "string", "format": "date-time" },
                "duration_min": { "type": "integer" },
                "attendees": { "type": "array", "items": { "type": "string" } },
                "visibility": { "type": "string", "enum": ["public", "private"] }
            },
            "required": ["title", "start"]
        })),
    }
}

fn email() -> ToolDef {
    ToolDef {
        name: "send_email".into(),
        description: Some("Send an email from the user's account.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": { "type": "array", "items": { "type": "string" } },
                "subject": { "type": "string" },
                "body": { "type": "string" },
                "cc": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["to", "subject", "body"]
        })),
    }
}

#[test]
fn multiple_tools_encode() {
    let c = encode_tools(&[calendar(), email()]).unwrap();
    assert!(c.prompt.contains("create_calendar_event"));
    assert!(c.prompt.contains("send_email"));
    assert!(c.prompt.contains("<<call TOOL_NAME JSON_ARGUMENTS>>"));
}

#[test]
fn nested_object_and_array() {
    let tool = ToolDef {
        name: "ship".into(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "dest": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" },
                        "zip": { "type": "string" }
                    },
                    "required": ["city"]
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sku": { "type": "string" },
                            "qty": { "type": "integer" }
                        },
                        "required": ["sku", "qty"]
                    }
                }
            },
            "required": ["dest", "items"]
        })),
    };
    let c = encode_tools(std::slice::from_ref(&tool)).unwrap();
    assert!(c.prompt.contains("dest:{"));
    assert!(c.prompt.contains("items:[{"));
    let call = ToolCall {
        name: "ship".into(),
        arguments: json!({
            "dest": { "city": "Hyd", "zip": "500032" },
            "items": [{ "sku": "A1", "qty": 2 }]
        }),
    };
    let rendered = render_call(&call).unwrap();
    let decoded = decode_calls(&rendered, std::slice::from_ref(&tool)).unwrap();
    assert_eq!(decoded[0].arguments["items"][0]["qty"], json!(2));
}

#[test]
fn multiple_calls() {
    let tools = vec![calendar(), email()];
    let text = r#"
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>
"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "send_email");
    assert_eq!(calls[1].name, "create_calendar_event");
}

#[test]
fn unicode_and_escapes() {
    let tools = vec![email()];
    let text = r#"<<call send_email {"to":["a@b.com"],"subject":"café \"quotes\"","body":"line1\nline2\\path"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert!(
        calls[0].arguments["subject"]
            .as_str()
            .unwrap()
            .contains("café")
    );
}

#[test]
fn invalid_type() {
    let tools = vec![calendar()];
    let text = r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":"thirty"}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.label(), "invalid_arguments");
}

#[test]
fn extra_fields_fail_closed() {
    let tools = vec![calendar()];
    let text = r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","surprise":true}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.label(), "invalid_arguments");
}

#[test]
fn malformed_json() {
    let tools = vec![calendar()];
    let text = r#"<<call create_calendar_event {"title":}>>"#;
    let err = decode_calls(text, &tools).unwrap_err();
    assert!(matches!(err.label(), "invalid_json" | "malformed_call"));
}

#[test]
fn stream_arbitrary_splits() {
    let tools = vec![calendar()];
    let full =
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#;
    for split_at in 1..full.len() {
        if !full.is_char_boundary(split_at) {
            continue;
        }
        let mut d = StreamDecoder::new(&tools);
        d.push(&full[..split_at]).unwrap();
        d.push(&full[split_at..]).unwrap();
        let calls = d.finish().unwrap();
        assert_eq!(calls.len(), 1, "split_at={split_at}");
        assert_eq!(calls[0].arguments["title"], json!("Retro"));
    }
}

#[test]
fn call_roundtrip() {
    let tools = vec![calendar(), email()];
    let calls = vec![
        ToolCall {
            name: "send_email".into(),
            arguments: json!({"to":["a@b.com"],"subject":"s","body":"b"}),
        },
        ToolCall {
            name: "create_calendar_event".into(),
            arguments: json!({"title":"T","start":"2026-10-05T15:00:00+05:30"}),
        },
    ];
    let rendered = render_calls(&calls).unwrap();
    let back = decode_calls(&rendered, &tools).unwrap();
    assert_eq!(back, calls);
}

#[test]
fn schema_roundtrip_semantic() {
    let tools = vec![calendar()];
    let compact = encode_tools(&tools).unwrap();
    let back = decode_tools(&compact).unwrap();
    assert_eq!(back[0].name, tools[0].name);
    let req = back[0].parameters.as_ref().unwrap()["required"]
        .as_array()
        .unwrap();
    assert!(req.iter().any(|v| v == "title"));
    assert!(req.iter().any(|v| v == "start"));
}

#[test]
fn split_like_preserves_concat() {
    let rendered = "abcdefghij";
    let template = vec!["ab".into(), "cdef".into(), "ghij".into()];
    let parts = split_like(rendered, &template);
    assert_eq!(parts.concat(), rendered);
}

#[test]
fn marker_inside_argument_string_does_not_terminate() {
    let tools = vec![email()];
    let text = r#"<<call send_email {"to":["a@b.com"],"subject":"x","body":"Use <<call something>> later"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(
        calls[0].arguments["body"],
        json!("Use <<call something>> later")
    );
}
