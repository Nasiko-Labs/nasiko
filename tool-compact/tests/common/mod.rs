#![allow(dead_code)]
use nasiko_tool_compact::ToolDef;
use serde_json::{Value, json};

pub fn tool(name: &str, description: Option<&str>, parameters: Option<Value>) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: description.map(Into::into),
        parameters,
    }
}

/// Parameters for a tool with a single required string `msg`.
pub fn msg_params() -> Value {
    json!({"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"]})
}

pub fn calendar() -> ToolDef {
    tool(
        "create_calendar_event",
        Some("Create an event in the user's calendar."),
        Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        })),
    )
}

pub fn email() -> ToolDef {
    tool(
        "send_email",
        Some("Send an email from the user's account."),
        Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                "subject": {"type": "string", "description": "Subject line"},
                "body": {"type": "string", "description": "Plain-text body"},
                "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
            },
            "required": ["to", "subject", "body"]
        })),
    )
}

/// Parse a call's JSON-string arguments back into a `Value` for order-insensitive asserts.
pub fn args(call: &nasiko_tool_compact::ToolCall) -> Value {
    serde_json::from_str(&call.arguments).expect("decoder must emit valid JSON arguments")
}
