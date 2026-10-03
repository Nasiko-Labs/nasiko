//! Shared fixtures: the public compact-tools sample tools, embedded so tests never read the
//! (git-excluded) eval file.

#![allow(dead_code)]

use nasiko_tool_compact::ToolDef;
use serde_json::{Value, json};

pub fn sample_tools_json() -> Value {
    json!([
        {
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
        },
        {
            "type": "function",
            "function": {
                "name": "send_email",
                "description": "Send an email from the user's account.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" }, "description": "Recipient emails" },
                        "subject": { "type": "string", "description": "Subject line" },
                        "body": { "type": "string", "description": "Plain-text body" },
                        "cc": { "type": "array", "items": { "type": "string" }, "description": "CC emails" }
                    },
                    "required": ["to", "subject", "body"]
                }
            }
        }
    ])
}

pub fn sample_tools() -> Vec<ToolDef> {
    sample_tools_json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| ToolDef::from_openai(t).unwrap())
        .collect()
}

/// A tool with one parameter schema, for focused tests.
pub fn tool(name: &str, parameters: Value) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: Some(format!("{name} tool")),
        parameters: Some(parameters),
    }
}

/// Nested object + arrays of objects.
pub fn nested_tool() -> ToolDef {
    tool(
        "create_ticket",
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "reporter": {
                    "type": "object",
                    "description": "Who filed it",
                    "properties": {
                        "email": { "type": "string" },
                        "team": { "type": "string", "enum": ["infra", "web"] }
                    },
                    "required": ["email"]
                },
                "labels": { "type": "array", "items": { "type": "string" } },
                "subtasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "estimate": { "type": "number" },
                            "done": { "type": "boolean" },
                            "due": { "type": "string", "format": "date" }
                        },
                        "required": ["name"]
                    }
                }
            },
            "required": ["title", "reporter"]
        }),
    )
}
