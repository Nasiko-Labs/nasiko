//! Shared fixtures: the two public eval tools plus a nested one.

#![allow(dead_code)]

use nasiko_tool_compact::ToolDef;
use serde_json::{Value, json};

pub fn tool(name: &str, description: &str, parameters: Value) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: Some(description.into()),
        parameters: Some(parameters),
    }
}

pub fn calendar() -> ToolDef {
    tool(
        "create_calendar_event",
        "Create an event in the user's calendar.",
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        }),
    )
}

pub fn email() -> ToolDef {
    tool(
        "send_email",
        "Send an email from the user's account.",
        json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                "subject": {"type": "string", "description": "Subject line"},
                "body": {"type": "string", "description": "Plain-text body"},
                "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
            },
            "required": ["to", "subject", "body"]
        }),
    )
}

/// Nested objects, arrays of objects, nullable, ranges, defaults, closed objects.
pub fn order() -> ToolDef {
    tool(
        "place_order",
        "Place an order.\nCharges the saved card.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "customer": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "tier": {"type": "string", "enum": ["free", "pro", "it's complicated"]}
                    },
                    "required": ["id"]
                },
                "items": {
                    "type": "array",
                    "description": "Line items; at least one",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "sku": {"type": "string"},
                            "qty": {"type": "integer", "minimum": 1, "maximum": 99, "default": 1}
                        },
                        "required": ["sku"]
                    }
                },
                "note": {"type": ["string", "null"], "description": "Gift message, or null"},
                "priority": {"type": "integer", "enum": [1, 2, 3]},
                "deliver_on": {"type": "string", "format": "date"},
                "express": {"type": "boolean", "default": false},
                "meta": {"type": "object"},
                "discount": {"type": "number", "minimum": 0, "maximum": 0.5}
            },
            "required": ["customer", "items"]
        }),
    )
}

pub fn sample_tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

pub fn all_tools() -> Vec<ToolDef> {
    vec![calendar(), email(), order()]
}
