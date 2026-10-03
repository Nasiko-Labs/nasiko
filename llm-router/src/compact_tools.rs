//! Compact tool schemas at the egress seam. Opt-in (`TOKEN_TOOL_COMPACT`), off by default.
//!
//! Replaces the native `tools` array with one compact signature line per tool in a leading system
//! message (see `nasiko-tool-compact` for the grammar), then turns the model's
//! `<<call NAME {json}>>` replies back into standard tool calls on the IR response, before any
//! inbound renderer sees it. Every inbound surface (OpenAI, Anthropic, Gemini) therefore gets
//! native tool calls and never sees the compact format.
//!
//! # Coverage
//!
//! Non-streaming requests. Everything else goes out untouched, with the reason recorded:
//! streaming, a forced `tool_choice` or `parallel_tool_calls` (compaction cannot guarantee
//! either), conversations that already contain tool calls or results (history would need
//! rewriting into the compact grammar too), non-function tools, and any schema the grammar cannot
//! carry exactly.
//!
//! # Fail closed
//!
//! A reply that does not decode against the original schemas is never repaired or guessed. The
//! caller re-sends [`Session::native`], the request exactly as it would have gone out without
//! this layer, so the client still gets a correct answer and pays for one extra call only on a
//! failure.

use nasiko_tool_compact as compact;
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};

/// Why the request went out native.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    NoTools,
    /// Not implemented yet for streamed responses.
    Streaming,
    /// `tool_choice` other than `auto`, or `parallel_tool_calls`: a text format cannot enforce it.
    ToolChoice,
    /// Earlier tool calls or results in the transcript.
    ToolHistory,
    /// A non-function tool, or a schema feature the compact grammar cannot carry exactly.
    Unsupported,
}

impl Skipped {
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoTools => "no_tools",
            Self::Streaming => "streaming",
            Self::ToolChoice => "tool_choice",
            Self::ToolHistory => "tool_history",
            Self::Unsupported => "unsupported",
        }
    }
}

/// What a compacted request needs to decode its reply, or to fall back.
pub(crate) struct Session {
    tools: Vec<compact::ToolDef>,
    /// The request exactly as it would have gone out without compaction.
    pub(crate) native: ChatRequest,
}

/// Compact `req`'s tools unless a carve-out applies. On `Err`, `req` is untouched.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Result<Session, Skipped> {
    if !cfg.tool_compact_enabled {
        return Err(Skipped::Disabled);
    }
    let Some(tools) = req.tools.as_deref().filter(|t| !t.is_empty()) else {
        return Err(Skipped::NoTools);
    };
    if req.is_streaming() {
        return Err(Skipped::Streaming);
    }
    match &req.tool_choice {
        None => {}
        Some(Value::String(choice)) if choice == "auto" => {}
        Some(_) => return Err(Skipped::ToolChoice),
    }
    if req.extra.contains_key("parallel_tool_calls") {
        return Err(Skipped::ToolChoice);
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.is_some())
    {
        return Err(Skipped::ToolHistory);
    }
    let defs = tools
        .iter()
        .map(to_compact)
        .collect::<Option<Vec<_>>>()
        .ok_or(Skipped::Unsupported)?;
    let encoded = compact::encode_tools(&defs).map_err(|_| Skipped::Unsupported)?;

    let native = req.clone();
    req.tools = None;
    req.tool_choice = None;
    req.messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(Value::String(encoded.prompt())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    Ok(Session {
        tools: defs,
        native,
    })
}

/// The router's tool type at the seam. Anything beyond a plain function tool is not carried.
fn to_compact(t: &ToolDef) -> Option<compact::ToolDef> {
    (t.kind == "function" && t.extra.is_empty()).then(|| compact::ToolDef {
        name: t.function.name.clone(),
        description: t.function.description.clone(),
        parameters: t.function.parameters.clone(),
    })
}

/// Turn compact calls in every choice into standard tool calls. On `Err`, `resp` is untouched.
///
/// A choice with no call is left byte-identical, so a plain answer passes straight through.
pub(crate) fn decode_response(
    resp: &mut ChatResponse,
    session: &Session,
) -> Result<(), compact::Error> {
    // Decode every choice before changing any, so a failure leaves the whole response as it was.
    let mut decoded = Vec::with_capacity(resp.choices.len());
    for choice in &resp.choices {
        decoded.push(
            match (choice.message.tool_calls.is_none(), choice.message.text()) {
                (true, Some(text)) => Some(compact::decode(&text, &session.tools)?),
                _ => None,
            },
        );
    }
    for (choice, decoded) in resp.choices.iter_mut().zip(decoded) {
        let Some(decoded) = decoded.filter(|d| !d.calls.is_empty()) else {
            continue;
        };
        choice.message.tool_calls = Some(
            decoded
                .calls
                .iter()
                .map(|call| ToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: call.name.clone(),
                        arguments: call.arguments_json(),
                    },
                    extra: Default::default(),
                })
                .collect(),
        );
        let text = decoded.text.trim();
        choice.message.content = (!text.is_empty()).then(|| Value::String(text.to_string()));
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            tool_compact_enabled: enabled,
            ..Default::default()
        }
    }

    fn request(extra: Value) -> ChatRequest {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Book a retro tomorrow 10am"}],
            "tools": [{"type": "function", "function": {
                "name": "create_calendar_event",
                "description": "Create an event.",
                "parameters": {"type": "object",
                    "properties": {"title": {"type": "string"},
                                   "start": {"type": "string", "format": "date-time"}},
                    "required": ["title", "start"]}
            }}]
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(body).unwrap()
    }

    fn reply(content: &str) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "x", "model": "gpt-4o",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": content}}]
        }))
        .unwrap()
    }

    #[test]
    fn every_skip_leaves_the_request_byte_identical() {
        let cases = [
            (cfg(false), json!({}), Skipped::Disabled),
            (cfg(true), json!({"tools": null}), Skipped::NoTools),
            (cfg(true), json!({"stream": true}), Skipped::Streaming),
            (
                cfg(true),
                json!({"tool_choice": "required"}),
                Skipped::ToolChoice,
            ),
            (
                cfg(true),
                json!({"parallel_tool_calls": false}),
                Skipped::ToolChoice,
            ),
            (
                cfg(true),
                json!({"messages": [{"role": "tool", "tool_call_id": "c", "content": "x"}]}),
                Skipped::ToolHistory,
            ),
            (
                cfg(true),
                json!({"tools": [{"type": "function", "function": {"name": "f",
                    "parameters": {"type": "object", "properties": {"n": {"type": "integer", "multipleOf": 5}}}}}]}),
                Skipped::Unsupported,
            ),
        ];
        for (cfg, extra, want) in cases {
            let mut req = request(extra);
            let before = serde_json::to_vec(&req).unwrap();
            assert_eq!(apply(&mut req, &cfg).err(), Some(want));
            assert_eq!(
                serde_json::to_vec(&req).unwrap(),
                before,
                "{want:?} mutated the request"
            );
        }
    }

    #[test]
    fn compacts_into_a_leading_system_message() {
        let mut req = request(json!({"tool_choice": "auto"}));
        let session = apply(&mut req, &cfg(true)).unwrap();
        assert!(req.tools.is_none() && req.tool_choice.is_none());
        assert_eq!(req.messages[0].role, "system");
        let prompt = req.messages[0].text().unwrap();
        assert!(prompt.contains("<<call NAME {JSON args}>>"));
        assert!(
            prompt
                .contains("create_calendar_event(start:datetime, title:string) - Create an event.")
        );
        assert_eq!(
            req.messages[1].text().as_deref(),
            Some("Book a retro tomorrow 10am")
        );
        // The fallback request is the original, untouched.
        assert_eq!(
            serde_json::to_vec(&session.native).unwrap(),
            serde_json::to_vec(&request(json!({"tool_choice": "auto"}))).unwrap()
        );
    }

    #[test]
    fn decodes_calls_into_standard_tool_calls() {
        let mut req = request(json!({}));
        let session = apply(&mut req, &cfg(true)).unwrap();
        let mut resp = reply(
            "On it.\n<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-03T10:00:00+05:30\"}>>",
        );
        decode_response(&mut resp, &session).unwrap();
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.content, Some(json!("On it.")));
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].id.starts_with("call_"));
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(
            args,
            json!({"title": "Retro", "start": "2026-10-03T10:00:00+05:30"})
        );
    }

    #[test]
    fn plain_answers_pass_through_byte_identical() {
        let mut req = request(json!({}));
        let session = apply(&mut req, &cfg(true)).unwrap();
        let mut resp = reply("It's sunny. Use x << 2 if you like.");
        let before = serde_json::to_vec(&resp).unwrap();
        decode_response(&mut resp, &session).unwrap();
        assert_eq!(serde_json::to_vec(&resp).unwrap(), before);
    }

    #[test]
    fn invalid_calls_fail_closed_and_leave_the_response_untouched() {
        let mut req = request(json!({}));
        let session = apply(&mut req, &cfg(true)).unwrap();
        for bad in [
            "<<call create_calendar_event {\"start\":\"2026-10-03T10:00:00Z\"}>>",
            "<<call delete_everything {}>>",
            "<<call create_calendar_event {\"title\":\"x\"",
        ] {
            let mut resp = reply(bad);
            let before = serde_json::to_vec(&resp).unwrap();
            assert!(decode_response(&mut resp, &session).is_err(), "{bad}");
            assert_eq!(serde_json::to_vec(&resp).unwrap(), before);
        }
    }
}
