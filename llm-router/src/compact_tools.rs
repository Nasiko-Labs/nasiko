//! Compact tool schemas on the way to a provider.
//!
//! The flag is read by the caller. This module does not read the environment.
//! When the flag is off the request is left untouched.

use crate::ir::ChatRequest;

/// Why a request kept its native tool definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Disabled,
}

/// What `apply` did.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub applied: bool,
    pub skip: Option<Skip>,
}

/// Rewrite `req` when compact tools are enabled for this provider. A disabled
/// flag returns [`Skip::Disabled`] and does not change `req`.
pub fn apply(req: &mut ChatRequest, enabled: bool, provider: &str) -> Outcome {
    let _ = (req, provider);
    if !enabled {
        return Outcome { applied: false, skip: Some(Skip::Disabled) };
    }
    Outcome { applied: false, skip: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{ChatRequest, FunctionDef, Message, ToolDef};
    use serde_json::{Map, Value, json};

    fn sample_chat_request() -> ChatRequest {
        ChatRequest {
            model: None,
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("Book a design review tomorrow.".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            }],
            tools: Some(vec![router_calendar()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: Some(false),
            extra: Map::new(),
        }
    }

    fn router_calendar() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event in the user's calendar.".into()),
                parameters: Some(json!({
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
            },
            extra: Map::new(),
        }
    }

    #[test]
    fn unset_flag_leaves_the_request_byte_identical() {
        let mut req = sample_chat_request();
        let before = serde_json::to_vec(&req).unwrap();
        let outcome = apply(&mut req, false, "openai");
        assert!(matches!(outcome.skip, Some(Skip::Disabled)));
        assert!(!outcome.applied);
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    }
}
