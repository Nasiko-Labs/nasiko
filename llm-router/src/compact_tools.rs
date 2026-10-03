//! Opt-in compaction of tool definitions at the egress seam (`[compact-tools]`).
//!
//! With `GatewayConfig::compact_tools_enabled` set, an eligible request has its native `tools`
//! replaced by one compact system block (see `nasiko-tool-compact`), and the model's
//! `<<call name {json}>>` reply is decoded back into standard `tool_calls` before the agent sees
//! it. The agent never sees the compact format. Off by default: with the flag off [`apply`]
//! returns before touching the request.
//!
//! Covered: non-streaming requests whose tools are plain function definitions in the supported
//! schema subset. Bypassed (the request goes out exactly as today): streaming, conversations that
//! already contain tool calls or tool results, a `tool_choice` other than `auto`, tool
//! definitions carrying extra options (e.g. `strict`), and schemas the format cannot represent.
//!
//! Fail closed: if the model writes a call that is unknown or fails the original schema, the
//! request fails with a 502 instead of forwarding a guessed call.

use nasiko_tool_compact as tc;
use serde_json::{Map, Value};

use crate::error::GatewayError;
use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};

/// What [`restore`] needs to decode the reply: the original definitions.
pub(crate) struct Compaction {
    defs: Vec<tc::ToolDef>,
}

/// Why a request was sent with its native tools.
#[derive(Debug, PartialEq)]
pub(crate) enum Skip {
    Disabled,
    NoTools,
    Streaming,
    ToolHistory,
    ToolChoice,
    Unsupported(String),
}

/// Replaces `req.tools` with a compact system block when the request is eligible. On `Err` the
/// request is left exactly as it was.
pub(crate) fn apply(req: &mut ChatRequest, enabled: bool) -> Result<Compaction, Skip> {
    if !enabled {
        return Err(Skip::Disabled);
    }
    let Some(tools) = req.tools.as_deref().filter(|t| !t.is_empty()) else {
        return Err(Skip::NoTools);
    };
    if req.is_streaming() {
        return Err(Skip::Streaming);
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.is_some())
    {
        return Err(Skip::ToolHistory);
    }
    if req
        .tool_choice
        .as_ref()
        .is_some_and(|c| c.as_str() != Some("auto"))
    {
        return Err(Skip::ToolChoice);
    }
    let defs = tools.iter().map(to_def).collect::<Result<Vec<_>, _>>()?;
    let compact = tc::encode_tools(&defs).map_err(|e| Skip::Unsupported(e.to_string()))?;

    req.tools = None;
    req.tool_choice = None;
    // Providers reject this knob when no `tools` are sent.
    req.extra.remove("parallel_tool_calls");
    inject_system(&mut req.messages, &compact.prompt);
    Ok(Compaction { defs })
}

/// Turns a compact reply into standard `tool_calls`. A plain answer is left untouched.
pub(crate) fn restore(
    resp: &mut ChatResponse,
    compaction: &Compaction,
) -> Result<(), GatewayError> {
    for choice in &mut resp.choices {
        if choice.message.tool_calls.is_some() {
            continue;
        }
        let Some(text) = choice.message.text() else {
            continue;
        };
        let reply = tc::decode_reply(&text, &compaction.defs).map_err(|e| {
            GatewayError::Upstream(format!(
                "model wrote an invalid compact tool call (not forwarded): {e}"
            ))
        })?;
        if reply.calls.is_empty() {
            continue;
        }
        choice.message.content = (!reply.text.is_empty()).then_some(Value::String(reply.text));
        choice.message.tool_calls = Some(
            reply
                .calls
                .into_iter()
                .map(|call| ToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                    kind: "function".to_string(),
                    function: FunctionCall {
                        name: call.name,
                        arguments: call.arguments,
                    },
                    extra: Map::new(),
                })
                .collect(),
        );
        choice.finish_reason = Some("tool_calls".to_string());
    }
    Ok(())
}

fn to_def(tool: &ToolDef) -> Result<tc::ToolDef, Skip> {
    if tool.kind != "function" || !tool.extra.is_empty() {
        return Err(Skip::Unsupported(format!(
            "tool `{}` has a non-plain definition",
            tool.function.name
        )));
    }
    Ok(tc::ToolDef {
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        parameters: tool.function.parameters.clone(),
    })
}

/// Appends to a leading text system message, or adds one in front.
fn inject_system(messages: &mut Vec<Message>, prompt: &str) {
    if let Some(Message {
        role,
        content: Some(Value::String(existing)),
        ..
    }) = messages.first_mut()
        && role == "system"
    {
        existing.push_str("\n\n");
        existing.push_str(prompt);
        return;
    }
    messages.insert(
        0,
        Message {
            role: "system".to_string(),
            content: Some(Value::String(prompt.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar() -> Value {
        json!({"type": "function", "function": {
            "name": "create_calendar_event",
            "description": "Create an event in the user's calendar.",
            "parameters": {"type": "object", "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            }, "required": ["title", "start"]}
        }})
    }

    fn request(extra: Value) -> ChatRequest {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Book a design review Monday 3pm"}],
            "tools": [calendar()],
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(body).unwrap()
    }

    fn response(content: &str) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "x", "model": "m",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": content}}]
        }))
        .unwrap()
    }

    fn json_of(req: &ChatRequest) -> Value {
        serde_json::to_value(req).unwrap()
    }

    #[test]
    fn disabled_leaves_the_request_byte_identical() {
        let mut req = request(json!({"tool_choice": "auto", "parallel_tool_calls": true}));
        let before = serde_json::to_string(&req).unwrap();
        assert_eq!(apply(&mut req, false).err(), Some(Skip::Disabled));
        assert_eq!(serde_json::to_string(&req).unwrap(), before);
    }

    #[test]
    fn enabled_swaps_native_tools_for_a_system_block() {
        let mut req = request(json!({"tool_choice": "auto", "parallel_tool_calls": true}));
        assert!(apply(&mut req, true).is_ok());
        let v = json_of(&req);
        assert!(v.get("tools").is_none() && v.get("tool_choice").is_none());
        assert!(v.get("parallel_tool_calls").is_none());
        assert_eq!(v["messages"][0]["role"], "system");
        let system = v["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("create_calendar_event(title:str, start:datetime"));
        assert_eq!(
            v["messages"][1]["content"],
            "Book a design review Monday 3pm"
        );
    }

    #[test]
    fn existing_system_message_is_extended_not_replaced() {
        let mut req: ChatRequest = serde_json::from_value(json!({
            "messages": [{"role": "system", "content": "Be brief."},
                         {"role": "user", "content": "hi"}],
            "tools": [calendar()],
        }))
        .unwrap();
        assert!(apply(&mut req, true).is_ok());
        let v = json_of(&req);
        assert_eq!(v["messages"].as_array().unwrap().len(), 2);
        assert!(
            v["messages"][0]["content"]
                .as_str()
                .unwrap()
                .starts_with("Be brief.\n\nTools")
        );
    }

    fn assert_bypassed(mut req: ChatRequest, why: Skip) {
        let before = serde_json::to_string(&req).unwrap();
        assert_eq!(apply(&mut req, true).err(), Some(why));
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            before,
            "request was modified"
        );
    }

    #[test]
    fn ineligible_requests_are_left_untouched() {
        assert_bypassed(request(json!({"stream": true})), Skip::Streaming);
        assert_bypassed(
            request(json!({"tool_choice": "required"})),
            Skip::ToolChoice,
        );
        assert_bypassed(
            request(json!({"tool_choice": {"type": "function", "function": {"name": "x"}}})),
            Skip::ToolChoice,
        );
        assert_bypassed(request(json!({"tools": []})), Skip::NoTools);
        assert_bypassed(
            request(json!({"messages": [
                {"role": "user", "content": "q"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "c1", "content": "r"}]})),
            Skip::ToolHistory,
        );
    }

    #[test]
    fn unsupported_tool_definitions_are_left_untouched() {
        let mut strict = calendar();
        strict["strict"] = json!(true);
        assert!(matches!(
            apply(&mut request(json!({"tools": [strict]})), true).err(),
            Some(Skip::Unsupported(_))
        ));
        let mut one_of = calendar();
        one_of["function"]["parameters"]["properties"]["title"] =
            json!({"oneOf": [{"type": "string"}, {"type": "integer"}]});
        assert!(matches!(
            apply(&mut request(json!({"tools": [one_of]})), true).err(),
            Some(Skip::Unsupported(_))
        ));
    }

    #[test]
    fn a_compact_call_becomes_a_standard_tool_call() {
        let mut req = request(json!({}));
        let compaction = apply(&mut req, true).unwrap();
        let mut resp = response(
            "Booking it.\n<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
        );
        restore(&mut resp, &compaction).unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        let msg = &v["choices"][0]["message"];
        assert_eq!(msg["content"], "Booking it.");
        assert_eq!(msg["tool_calls"][0]["type"], "function");
        assert!(
            msg["tool_calls"][0]["id"]
                .as_str()
                .unwrap()
                .starts_with("call_")
        );
        assert_eq!(
            msg["tool_calls"][0]["function"]["name"],
            "create_calendar_event"
        );
        let args: Value = serde_json::from_str(
            msg["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(args["title"], "Design review");
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn a_plain_answer_is_untouched() {
        let compaction = apply(&mut request(json!({})), true).unwrap();
        let mut resp = response("It looks sunny.");
        let before = serde_json::to_string(&resp).unwrap();
        restore(&mut resp, &compaction).unwrap();
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn an_invalid_call_fails_closed() {
        let compaction = apply(&mut request(json!({})), true).unwrap();
        for bad in [
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#, // missing title
            r#"<<call create_calendar_event {"title":"x","start":"s","visibility":"secret"}>>"#,
            r#"<<call delete_everything {}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"s"}}>"#, // malformed close
        ] {
            let mut resp = response(bad);
            let err = restore(&mut resp, &compaction).unwrap_err();
            assert!(matches!(err, GatewayError::Upstream(_)), "{bad}");
            assert!(resp.choices[0].message.tool_calls.is_none(), "{bad}");
        }
    }
}
