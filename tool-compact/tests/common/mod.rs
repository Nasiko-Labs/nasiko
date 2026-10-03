//! Shared fixtures for the integration tests. Tool names here are test fixtures only; the
//! library never refers to them.

#![allow(dead_code)]

use nasiko_tool_compact::ToolDef;
use serde_json::{Value, json};

pub fn calendar() -> ToolDef {
    ToolDef::new(
        "create_calendar_event",
        Some("Create an event in the user's calendar.".into()),
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
    ToolDef::new(
        "send_email",
        Some("Send an email from the user's account.".into()),
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

pub fn ticket() -> ToolDef {
    ToolDef::new(
        "tracker.create_ticket",
        Some("Open a ticket.".into()),
        Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "priority": {"type": "string", "enum": ["low", "medium", "high"]},
                "assignee": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}, "email": {"type": "string"}},
                    "required": ["name"]
                },
                "labels": {"type": "array", "items": {"type": "string"}, "maxItems": 5}
            },
            "required": ["title", "priority"]
        })),
    )
}

pub fn ping() -> ToolDef {
    ToolDef::new("ping", Some("Health check.".into()), None)
}

pub fn tools() -> Vec<ToolDef> {
    vec![calendar(), email(), ticket(), ping()]
}

pub fn args(value: Value) -> serde_json::Map<String, Value> {
    value
        .as_object()
        .cloned()
        .expect("fixture arguments are an object")
}
