//! Broad deterministic coverage for the P1 compact-tools contract.

use crate::{
    CompactError, StreamDecoder, ToolDef, decode_calls, decode_tools, encode_tools, render_calls,
    tool_call,
};
use proptest::prelude::*;
use serde_json::json;

fn calendar() -> ToolDef {
    ToolDef {
        name: "create_calendar_event".into(),
        description: Some("Create an event in the user's calendar.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "duration_min": {"type": "integer"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "meta": {
                    "type": "object",
                    "properties": {
                        "room": {"type": "string"}
                    }
                }
            },
            "required": ["title", "start"]
        })),
    }
}

fn email() -> ToolDef {
    ToolDef {
        name: "send_email".into(),
        description: Some("Send an email.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string"},
                "body": {"type": "string"},
                "urgent": {"type": "boolean"}
            },
            "required": ["to", "subject", "body"]
        })),
    }
}

#[test]
fn single_tool_encoding() {
    let c = encode_tools(&[calendar()]).unwrap();
    assert!(c.prompt.contains("create_calendar_event("));
    assert!(c.prompt.contains("title:str!"));
}

#[test]
fn multiple_tool_encoding() {
    let c = encode_tools(&[calendar(), email()]).unwrap();
    assert!(c.prompt.contains("create_calendar_event("));
    assert!(c.prompt.contains("send_email("));
}

#[test]
fn required_and_optional_markers() {
    let c = encode_tools(&[calendar()]).unwrap();
    assert!(c.prompt.contains("title:str!"));
    assert!(c.prompt.contains("duration_min?:int"));
}

#[test]
fn enum_array_datetime_nested() {
    let c = encode_tools(&[calendar()]).unwrap();
    assert!(c.prompt.contains("visibility?:public|private"));
    assert!(c.prompt.contains("attendees?:[str]"));
    assert!(c.prompt.contains("start:datetime!"));
    assert!(c.prompt.contains("meta?:{room?:str}") || c.prompt.contains("meta?:{"));
}

#[test]
fn unknown_tool_fails() {
    assert_eq!(
        decode_calls("<<nope {}>>", &[calendar()]).unwrap_err(),
        CompactError::UnknownTool
    );
}

#[test]
fn missing_required_fails() {
    let text = r#"<<create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#;
    assert_eq!(
        decode_calls(text, &[calendar()]).unwrap_err(),
        CompactError::InvalidArguments
    );
}

#[test]
fn invalid_enum_fails() {
    let text = r#"<<create_calendar_event {"title":"t","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
    assert_eq!(
        decode_calls(text, &[calendar()]).unwrap_err(),
        CompactError::InvalidArguments
    );
}

#[test]
fn invalid_type_fails() {
    let text = r#"<<create_calendar_event {"title":1,"start":"2026-10-05T15:00:00+05:30"}>>"#;
    assert_eq!(
        decode_calls(text, &[calendar()]).unwrap_err(),
        CompactError::InvalidArguments
    );
}

#[test]
fn malformed_json_fails() {
    assert!(decode_calls(r#"<<create_calendar_event {"title":>>"#, &[calendar()]).is_err());
}

#[test]
fn plain_text_no_error() {
    assert!(decode_calls("hello", &[calendar()]).unwrap().is_empty());
}

#[test]
fn text_before_and_after() {
    let text = "before\n<<create_calendar_event {\"title\":\"t\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>\nafter";
    assert_eq!(decode_calls(text, &[calendar()]).unwrap().len(), 1);
}

#[test]
fn multiple_calls() {
    let text = r#"<<send_email {"to":["a@b.com"],"subject":"s","body":"b"}>>
<<create_calendar_event {"title":"t","start":"2026-10-05T15:00:00+05:30"}>>"#;
    assert_eq!(decode_calls(text, &[calendar(), email()]).unwrap().len(), 2);
}

#[test]
fn gtgt_inside_string() {
    let text = r#"<<send_email {"to":["a@b.com"],"subject":"use >> here","body":"x"}>>"#;
    assert_eq!(
        decode_calls(text, &[email()]).unwrap()[0].arguments["subject"],
        "use >> here"
    );
}

#[test]
fn split_marker_json_and_close() {
    let chunks = [
        "<<cre",
        "ate_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let mut d = StreamDecoder::new(vec![calendar()]);
    for c in chunks {
        let _ = d.push(c).unwrap();
    }
    assert_eq!(d.finish().unwrap()[0].arguments["title"], "Retro");
}

#[test]
fn empty_chunks() {
    let mut d = StreamDecoder::new(vec![calendar()]);
    assert!(d.push("").unwrap().is_empty());
    assert!(d.push("").unwrap().is_empty());
    assert!(d.finish().unwrap().is_empty());
}

#[test]
fn unsupported_ref_and_anyof() {
    let t = ToolDef {
        name: "x".into(),
        description: None,
        parameters: Some(json!({"$ref": "#/defs/X"})),
    };
    assert!(matches!(
        encode_tools(&[t]).unwrap_err(),
        CompactError::UnsupportedSchema(_)
    ));
    let t = ToolDef {
        name: "y".into(),
        description: None,
        parameters: Some(json!({"anyOf": [{"type": "string"}]})),
    };
    assert!(matches!(
        encode_tools(&[t]).unwrap_err(),
        CompactError::UnsupportedSchema(_)
    ));
}

#[test]
fn schema_and_call_roundtrip() {
    let tools = vec![calendar(), email()];
    let compact = encode_tools(&tools).unwrap();
    assert_eq!(decode_tools(&compact).unwrap(), tools);

    let call = tool_call(
        "create_calendar_event",
        json!({
            "title": "Design review",
            "start": "2026-10-05T15:00:00+05:30",
            "attendees": ["riya@example.com"],
            "meta": {"room": "A"}
        }),
    );
    let rendered = render_calls(&[call.clone()]).unwrap();
    let decoded = decode_calls(&rendered, &tools).unwrap();
    assert_eq!(decoded[0].name, call.name);
    assert_eq!(decoded[0].arguments, call.arguments);
}

proptest! {
    #[test]
    fn prop_roundtrip_msg(msg in "[a-zA-Z0-9 _>]{0,40}") {
        let tools = [email()];
        let call = tool_call(
            "send_email",
            json!({"to": ["a@b.com"], "subject": msg, "body": "x"}),
        );
        let rendered = render_calls(&[call.clone()]).unwrap();
        let decoded = decode_calls(&rendered, &tools).unwrap();
        assert_eq!(decoded[0].arguments["subject"], call.arguments["subject"]);
    }
}

#[test]
fn nested_object_validated() {
    let text = r#"<<create_calendar_event {"title":"t","start":"2026-10-05T15:00:00+05:30","meta":{"room":"B"}}>>"#;
    let calls = decode_calls(text, &[calendar()]).unwrap();
    assert_eq!(calls[0].arguments["meta"]["room"], "B");
}

#[test]
fn wrong_nested_type_fails() {
    let text = r#"<<create_calendar_event {"title":"t","start":"2026-10-05T15:00:00+05:30","meta":{"room":1}}>>"#;
    assert_eq!(
        decode_calls(text, &[calendar()]).unwrap_err(),
        CompactError::InvalidArguments
    );
}

#[test]
fn ambiguous_string_params_keep_distinct_descriptions() {
    let tool = ToolDef {
        name: "notify_employee".into(),
        description: Some("Notify an employee.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "recipient": {
                    "type": "string",
                    "description": "Email address of the employee receiving the notification"
                },
                "employee_id": {
                    "type": "string",
                    "description": "Internal employee identifier"
                }
            },
            "required": ["recipient", "employee_id"]
        })),
    };
    let prompt = encode_tools(&[tool]).unwrap().prompt;
    assert!(
        prompt.contains("recipient:str!") && prompt.contains("employee_id:str!"),
        "prompt={prompt}"
    );
    // Both are strings — short hints must distinguish email vs internal id.
    assert!(
        prompt.to_lowercase().contains("email") && prompt.to_lowercase().contains("internal"),
        "ambiguous string fields must keep semantic hints; prompt={prompt}"
    );
    assert!(
        prompt.contains("\"email address\""),
        "recipient hint missing; prompt={prompt}"
    );
    assert!(
        prompt.contains("\"internal identifier\""),
        "employee_id hint missing; prompt={prompt}"
    );
    // Name already says notify_employee — tool description omitted.
    assert!(
        !prompt.contains("Notify an employee"),
        "redundant tool description should be omitted; prompt={prompt}"
    );
}
