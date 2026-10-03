//! Compact tool schemas on the way to OpenAI, and decode the reply on the way back.
//!
//! The flag is read by the caller. This module does not read the environment.
//! Streaming and non-OpenAI providers keep the native request.

use serde_json::{Map, Value};

use crate::ir::chat::{ChatResponse, FunctionCall, Message, ToolCall, ToolDef};
use crate::ir::ChatRequest;

/// Why a request kept its native tool definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Disabled,
    UnsupportedSchema,
    ForcedToolChoice,
    UnsupportedPath,
}

/// What `apply` did. Original tools stay here for the response path and never
/// go back onto the outbound request.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub applied: bool,
    pub skip: Option<Skip>,
    pub originals: Vec<ToolDef>,
}

/// Rewrite `req` when compact tools are enabled for OpenAI non-streaming.
/// A disabled flag returns [`Skip::Disabled`] and does not change `req`.
pub fn apply(req: &mut ChatRequest, enabled: bool, provider: &str) -> Outcome {
    if !enabled {
        return skipped(Skip::Disabled);
    }
    if provider != "openai" || req.is_streaming() {
        return skipped(Skip::UnsupportedPath);
    }
    if forced_tool_choice(&req.tool_choice) {
        return skipped(Skip::ForcedToolChoice);
    }
    let Some(tools) = req.tools.clone() else {
        return skipped_quiet();
    };
    if tools.is_empty() {
        return skipped_quiet();
    }
    let converted: Vec<nasiko_tool_compact::ToolDef> = tools.iter().map(to_crate_tool).collect();
    match nasiko_tool_compact::encode_tools(&converted) {
        Ok(compact) => {
            req.messages.push(system_message(compact.text));
            req.tools = None;
            Outcome { applied: true, skip: None, originals: tools }
        }
        Err(_) => skipped(Skip::UnsupportedSchema),
    }
}

/// Decode assistant text into OpenAI tool calls. Ids are `call_1`, `call_2`, …
/// On error the caller keeps the provider text and invents no call.
pub fn decode_response(
    text: &str,
    originals: &[ToolDef],
) -> Result<Vec<ToolCall>, nasiko_tool_compact::CompactError> {
    let tools: Vec<nasiko_tool_compact::ToolDef> = originals.iter().map(to_crate_tool).collect();
    let calls = nasiko_tool_compact::decode_calls(text, &tools)?;
    Ok(calls
        .into_iter()
        .enumerate()
        .map(|(index, call)| ToolCall {
            id: format!("call_{}", index + 1),
            kind: "function".into(),
            function: FunctionCall { name: call.name, arguments: call.arguments },
            extra: Map::new(),
        })
        .collect())
}

/// Replace compact call text on an OpenAI reply with standard tool calls.
/// A decode error leaves the choice unchanged.
pub fn attach_decoded_calls(resp: &mut ChatResponse, originals: &[ToolDef]) {
    for choice in &mut resp.choices {
        let Some(text) = choice.message.text() else {
            continue;
        };
        match decode_response(&text, originals) {
            Ok(calls) if !calls.is_empty() => {
                choice.message.tool_calls = Some(calls);
                choice.message.content = None;
                choice.finish_reason = Some("tool_calls".into());
            }
            Ok(_) => {}
            Err(err) => {
                tracing::debug!(
                    target: "nasiko::llm_router::compact_tools",
                    error = %err,
                    "compact tools: reply did not decode; leaving provider text"
                );
            }
        }
    }
}

fn skipped(skip: Skip) -> Outcome {
    Outcome { applied: false, skip: Some(skip), originals: Vec::new() }
}

fn skipped_quiet() -> Outcome {
    Outcome { applied: false, skip: None, originals: Vec::new() }
}

fn system_message(text: String) -> Message {
    Message {
        role: "system".into(),
        content: Some(Value::String(text)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Map::new(),
    }
}

/// Anything other than an absent choice or the string `"auto"` is a forced choice.
fn forced_tool_choice(choice: &Option<Value>) -> bool {
    match choice {
        None => false,
        Some(Value::String(value)) if value == "auto" => false,
        Some(_) => true,
    }
}

fn to_crate_tool(tool: &ToolDef) -> nasiko_tool_compact::ToolDef {
    nasiko_tool_compact::ToolDef {
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        parameters: tool.function.parameters.clone(),
    }
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

    fn design_review() -> &'static str {
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>"#
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

    #[test]
    fn enabled_openai_request_drops_native_tools_and_adds_the_compact_text() {
        let mut req = sample_chat_request();
        let outcome = apply(&mut req, true, "openai");
        assert!(outcome.applied);
        assert!(req.tools.is_none());
        let blob = serde_json::to_string(&req).unwrap();
        assert!(blob.contains("<<call name {json args}>>"));
        assert!(blob.contains("create_calendar_event("));
    }

    #[test]
    fn decoded_reply_is_an_openai_tool_call() {
        let calls = decode_response(design_review(), &[router_calendar()]).unwrap();
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], "Design review");
    }

    #[test]
    fn streaming_and_other_providers_are_not_rewritten() {
        let mut streaming = sample_chat_request();
        streaming.stream = Some(true);
        let before = serde_json::to_vec(&streaming).unwrap();
        assert!(matches!(
            apply(&mut streaming, true, "openai").skip,
            Some(Skip::UnsupportedPath)
        ));
        assert_eq!(serde_json::to_vec(&streaming).unwrap(), before);

        let mut anthropic = sample_chat_request();
        let before = serde_json::to_vec(&anthropic).unwrap();
        assert!(matches!(
            apply(&mut anthropic, true, "anthropic").skip,
            Some(Skip::UnsupportedPath)
        ));
        assert_eq!(serde_json::to_vec(&anthropic).unwrap(), before);
    }

    #[test]
    fn unsupported_schema_keeps_native_tools() {
        let mut req = sample_chat_request();
        req.tools.as_mut().unwrap()[0].function.parameters = Some(json!({ "$ref": "#/$defs/Id" }));
        let before = serde_json::to_vec(&req).unwrap();
        let outcome = apply(&mut req, true, "openai");
        assert!(matches!(outcome.skip, Some(Skip::UnsupportedSchema)));
        assert!(!outcome.applied);
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    }

    #[test]
    fn forced_tool_choice_keeps_native_tools() {
        let mut req = sample_chat_request();
        req.tool_choice = Some(json!({
            "type": "function",
            "function": {"name": "create_calendar_event"}
        }));
        let before = serde_json::to_vec(&req).unwrap();
        let outcome = apply(&mut req, true, "openai");
        assert!(matches!(outcome.skip, Some(Skip::ForcedToolChoice)));
        assert!(!outcome.applied);
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);

        let mut none = sample_chat_request();
        none.tool_choice = Some(Value::String("none".into()));
        let before = serde_json::to_vec(&none).unwrap();
        let outcome = apply(&mut none, true, "openai");
        assert!(matches!(outcome.skip, Some(Skip::ForcedToolChoice)));
        assert_eq!(serde_json::to_vec(&none).unwrap(), before);
    }

    #[test]
    fn auto_tool_choice_still_compacts() {
        let mut req = sample_chat_request();
        req.tool_choice = Some(Value::String("auto".into()));
        let outcome = apply(&mut req, true, "openai");
        assert!(outcome.applied);
        assert!(req.tools.is_none());
    }

    #[test]
    fn gemini_streaming_keeps_native_tools() {
        let mut req = sample_chat_request();
        req.stream = Some(true);
        let before = serde_json::to_vec(&req).unwrap();
        let outcome = apply(&mut req, true, "gemini");
        assert!(matches!(outcome.skip, Some(Skip::UnsupportedPath)));
        assert!(!outcome.applied);
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    }
}
