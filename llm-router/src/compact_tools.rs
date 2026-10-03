//! Compact tool schemas at the egress seam (opt-in, `TOKEN_COMPACT_TOOLS`).
//!
//! When it applies, the request goes out with its `tools` replaced by one leading `system`
//! message holding the compact block (`nasiko-tool-compact`), and the model's
//! `<<call NAME {JSON}>>` text is decoded back into standard `tool_calls` before the client
//! sees anything.
//!
//! Coverage: **non-streaming** chat on every inbound surface (the seam is the shared IR).
//! Everything else goes out natively: streaming, forced `tool_choice`, conversations that
//! already carry tool calls or results, `parallel_tool_calls`, non-function tools, schemas the
//! crate cannot state exactly, and blocks that would not be smaller.
//!
//! Fail-closed: if the reply contains a call that does not decode and validate, the caller
//! re-sends the original request natively. Nothing has reached the client yet, so the retry is
//! invisible (I10). With the flag off, [`prepare`] returns before looking at the request, and
//! the handler sends the request it was given — byte-identical (I9).

use nasiko_tool_compact as compact;

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::ir::chat::{ChatResponse, FunctionCall, Message, ToolCall};

/// Why a request went out natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Skipped {
    Disabled,
    Streaming,
    NoTools,
    ForcedToolChoice,
    ToolHistory,
    ParallelToolCalls,
    NotFunctionTool,
    Unsupported,
    NotSmaller,
}

/// A compacted request plus what is needed to decode its reply.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub request: ChatRequest,
    tools: Vec<compact::ToolDef>,
}

/// Build the compacted request, or say why the original must go out unchanged.
pub(crate) fn prepare(req: &ChatRequest, cfg: &GatewayConfig) -> Result<Prepared, Skipped> {
    if !cfg.compact_tools_enabled {
        return Err(Skipped::Disabled);
    }
    if req.is_streaming() {
        return Err(Skipped::Streaming);
    }
    let Some(native) = req.tools.as_ref().filter(|t| !t.is_empty()) else {
        return Err(Skipped::NoTools);
    };
    if req.tool_choice.as_ref().is_some_and(|c| c != "auto") {
        return Err(Skipped::ForcedToolChoice);
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.is_some() || m.tool_call_id.is_some())
    {
        return Err(Skipped::ToolHistory);
    }
    if req.extra.contains_key("parallel_tool_calls") {
        return Err(Skipped::ParallelToolCalls);
    }
    if native
        .iter()
        .any(|t| t.kind != "function" || !t.extra.is_empty())
    {
        return Err(Skipped::NotFunctionTool);
    }
    let tools: Vec<compact::ToolDef> = native
        .iter()
        .map(|t| compact::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect();
    let block = compact::encode_tools(&tools).map_err(|_| Skipped::Unsupported)?;

    let mut request = req.clone();
    request.tools = None;
    request.tool_choice = None;
    request.messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(serde_json::Value::String(block.text)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    let size = |r: &ChatRequest| serde_json::to_vec(r).map(|v| v.len()).unwrap_or(usize::MAX);
    if size(&request) >= size(req) {
        return Err(Skipped::NotSmaller);
    }
    Ok(Prepared { request, tools })
}

impl Prepared {
    /// Turn compact calls in `resp` into standard `tool_calls`. All-or-nothing: on error
    /// `resp` is untouched and the caller must retry natively.
    pub(crate) fn restore(&self, resp: &mut ChatResponse) -> Result<(), compact::Error> {
        let mut updates = Vec::new();
        for (i, choice) in resp.choices.iter().enumerate() {
            let Some(serde_json::Value::String(text)) = &choice.message.content else {
                continue;
            };
            let mut decoder = compact::StreamDecoder::new(&self.tools);
            let mut events = decoder.push(text)?;
            events.extend(decoder.finish()?);
            let mut prose = String::new();
            let mut calls = Vec::new();
            for event in events {
                match event {
                    compact::StreamEvent::Text(t) => prose.push_str(&t),
                    compact::StreamEvent::Call(c) => calls.push(ToolCall {
                        id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: c.name,
                            arguments: c.arguments.to_string(),
                        },
                        extra: Default::default(),
                    }),
                }
            }
            if !calls.is_empty() {
                let prose = prose.trim();
                updates.push((i, (!prose.is_empty()).then(|| prose.to_string()), calls));
            }
        }
        for (i, prose, calls) in updates {
            if let Some(choice) = resp.choices.get_mut(i) {
                choice.message.content = prose.map(serde_json::Value::String);
                choice.message.tool_calls = Some(calls);
                choice.finish_reason = Some("tool_calls".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(extra: serde_json::Value) -> ChatRequest {
        let mut body = json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "Book a retro Sunday 10am"}],
            "tools": [{"type": "function", "function": {
                "name": "create_calendar_event",
                "description": "Create an event in the user's calendar.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                }
            }}]
        });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        serde_json::from_value(body).unwrap()
    }

    fn on() -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        }
    }

    fn reply(content: &str) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "r", "object": "chat.completion", "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
        }))
        .unwrap()
    }

    /// I9: off by default, and off means the request is not even looked at.
    #[test]
    fn off_by_default() {
        assert!(!GatewayConfig::default().compact_tools_enabled);
        let req = request(json!({}));
        let before = serde_json::to_vec(&req).unwrap();
        assert_eq!(
            prepare(&req, &GatewayConfig::default()).unwrap_err(),
            Skipped::Disabled
        );
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    }

    #[test]
    fn compacts_and_restores_calls() {
        let p = prepare(&request(json!({})), &on()).unwrap();
        assert!(p.request.tools.is_none());
        assert_eq!(p.request.messages[0].role, "system");
        let mut resp = reply(
            "Booking it.\n<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>",
        );
        p.restore(&mut resp).unwrap();
        let msg = &resp.choices[0].message;
        let call = &msg.tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.function.name, "create_calendar_event");
        let args: serde_json::Value = serde_json::from_str(&call.function.arguments).unwrap();
        assert_eq!(args["title"], "Retro");
        assert!(call.id.starts_with("call_"));
        assert_eq!(msg.content, Some(json!("Booking it.")));
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn plain_answer_is_untouched_and_bad_calls_fail_closed() {
        let p = prepare(&request(json!({})), &on()).unwrap();
        let mut plain = reply("It is sunny.");
        p.restore(&mut plain).unwrap();
        assert!(plain.choices[0].message.tool_calls.is_none());
        for bad in [
            "<<call delete_everything {}>>",
            "<<call create_calendar_event {\"title\":\"x\"}>>",
            "<<call create_calendar_event {",
        ] {
            let mut resp = reply(bad);
            assert!(p.restore(&mut resp).is_err(), "{bad}");
            assert_eq!(
                resp.choices[0].message.content,
                Some(json!(bad)),
                "untouched"
            );
        }
    }

    #[test]
    fn bypasses_what_it_cannot_guarantee() {
        let cases = [
            (json!({"stream": true}), Skipped::Streaming),
            (json!({"tools": []}), Skipped::NoTools),
            (
                json!({"tool_choice": "required"}),
                Skipped::ForcedToolChoice,
            ),
            (
                json!({"parallel_tool_calls": false}),
                Skipped::ParallelToolCalls,
            ),
            (
                json!({"messages": [{"role": "tool", "tool_call_id": "c1", "content": "ok"}]}),
                Skipped::ToolHistory,
            ),
            (
                json!({"tools": [{"type": "function", "function": {"name": "t", "parameters": {"type": "object", "properties": {"a": {"anyOf": [{"type": "string"}]}}}}}]}),
                Skipped::Unsupported,
            ),
        ];
        for (extra, want) in cases {
            assert_eq!(
                prepare(&request(extra.clone()), &on()).unwrap_err(),
                want,
                "{extra}"
            );
        }
        assert!(prepare(&request(json!({"tool_choice": "auto"})), &on()).is_ok());
    }
}
