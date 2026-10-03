//! Compact tool schemas at the egress seam (experimental, opt-in).
//!
//! When `TOKEN_COMPACT_TOOLS` is on, a request's JSON-Schema `tools` are replaced by compact
//! signatures in a leading `system` message ([`nasiko_tool_compact`]), and the model's
//! `<<call ...>>` output is decoded back into standard `tool_calls` before the client sees it.
//! The client never sees the compact format: a response that does not decode to valid calls is
//! an upstream error, never a guessed call or leaked markers.
//!
//! # Coverage
//!
//! Non-streaming Chat Completions only, on every provider (it runs on the IR). Everything
//! outside the safe envelope **bypasses** and goes out exactly as today:
//!
//! * streaming requests, and the Responses API;
//! * `tool_choice` other than absent/`"auto"` — a forced or disabled choice cannot be guaranteed
//!   by a prompt;
//! * conversations that already carry `tool_calls` or `tool` results — history stays native, so
//!   only the first step of a tool loop is compacted;
//! * any tool the compact format cannot express exactly (see the crate's *Supported schemas*),
//!   non-`function` tools, and tools with extra fields such as `strict`.
//!
//! # Placement
//!
//! A new **leading** system message, so the definitions sit where `tools` would in a provider's
//! cached prefix and stay byte-stable across turns. Author-written messages are untouched.

use nasiko_tool_compact as tc;
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall, ToolDef};

/// Why compaction was not applied. The request is untouched in every case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bypass {
    Disabled,
    NoTools,
    Streaming,
    ToolChoice,
    History,
    Unsupported(String),
}

impl Bypass {
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoTools => "no_tools",
            Self::Streaming => "streaming",
            Self::ToolChoice => "tool_choice",
            Self::History => "history",
            Self::Unsupported(_) => "unsupported_schema",
        }
    }
}

/// State needed to decode the response of a compacted request.
#[derive(Debug, Clone)]
pub struct Compacted {
    pub tools: Vec<tc::ToolDef>,
    /// Bytes of the injected system message, for telemetry.
    pub prompt_bytes: usize,
}

/// Router-side gate: the config flag and the paths this integration covers, then [`compact`].
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Result<Compacted, Bypass> {
    if !cfg.compact_tools_enabled {
        return Err(Bypass::Disabled);
    }
    if req.is_streaming() {
        return Err(Bypass::Streaming);
    }
    compact(req)
}

/// Replace `req.tools` with a compact system message. On `Err` the request is untouched.
///
/// Public so the eval example builds its requests through this exact code path.
pub fn compact(req: &mut ChatRequest) -> Result<Compacted, Bypass> {
    let Some(tools) = req.tools.as_ref().filter(|t| !t.is_empty()) else {
        return Err(Bypass::NoTools);
    };
    match &req.tool_choice {
        None => {}
        Some(Value::String(s)) if s == "auto" => {}
        Some(_) => return Err(Bypass::ToolChoice),
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.as_ref().is_some_and(|c| !c.is_empty()))
    {
        return Err(Bypass::History);
    }
    let defs = tools
        .iter()
        .map(to_compact_def)
        .collect::<Result<Vec<_>, _>>()?;
    let compact = tc::encode_tools(&defs).map_err(|e| Bypass::Unsupported(e.to_string()))?;

    let prompt = compact.prompt();
    let prompt_bytes = prompt.len();
    req.tools = None;
    req.tool_choice = None;
    // Only meaningful alongside `tools`; OpenAI rejects it without them.
    req.extra.remove("parallel_tool_calls");
    req.messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(Value::String(prompt)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    Ok(Compacted {
        tools: defs,
        prompt_bytes,
    })
}

/// The seam conversion. Anything carrying fields the compact type does not model bypasses.
pub fn to_compact_def(t: &ToolDef) -> Result<tc::ToolDef, Bypass> {
    if t.kind != "function" || !t.extra.is_empty() {
        return Err(Bypass::Unsupported(format!(
            "tool '{}' is not a plain function tool",
            t.function.name
        )));
    }
    Ok(tc::ToolDef {
        name: t.function.name.clone(),
        description: t.function.description.clone(),
        parameters: t.function.parameters.clone(),
    })
}

/// Decode compact calls in every choice into `tool_calls`. `next_id` mints each call's id.
///
/// A choice with no call is left byte-identical. A choice with calls gets `tool_calls`, its
/// remaining text (trimmed; `null` if empty) as `content`, and `finish_reason: "tool_calls"`.
/// Any invalid call fails the whole response — the caller turns that into an upstream error.
pub fn restore(
    resp: &mut ChatResponse,
    compacted: &Compacted,
    mut next_id: impl FnMut() -> String,
) -> Result<usize, tc::Error> {
    let mut total = 0;
    for choice in &mut resp.choices {
        let Some(text) = choice.message.text() else {
            continue;
        };
        let decoded = tc::decode(&text, &compacted.tools)?;
        if decoded.calls.is_empty() {
            continue;
        }
        total += decoded.calls.len();
        let rest = decoded.text.trim();
        choice.message.content = (!rest.is_empty()).then(|| Value::String(rest.to_owned()));
        choice.message.tool_calls = Some(
            decoded
                .calls
                .into_iter()
                .map(|c| ToolCall {
                    id: next_id(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: c.name,
                        arguments: c.arguments,
                    },
                    extra: Default::default(),
                })
                .collect(),
        );
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(extra: Value) -> ChatRequest {
        let mut body = json!({
            "model": "gpt-4o-mini",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Book a design review Monday 3pm IST"}
            ],
            "tools": [{"type": "function", "function": {
                "name": "create_calendar_event",
                "description": "Create an event in the user's calendar.",
                "parameters": {"type": "object", "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"}
                }, "required": ["title", "start"]}
            }}],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "temperature": 0.2
        });
        if let (Value::Object(b), Value::Object(e)) = (&mut body, extra) {
            b.extend(e);
        }
        serde_json::from_value(body).unwrap()
    }

    fn response(content: &str) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "chatcmpl-1", "model": "gpt-4o-mini",
            "choices": [{"index": 0, "finish_reason": "stop",
                "message": {"role": "assistant", "content": content}}]
        }))
        .unwrap()
    }

    fn ser<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string(v).unwrap()
    }

    #[test]
    fn flag_off_is_byte_identical() {
        let cfg = GatewayConfig::default();
        assert!(!cfg.compact_tools_enabled, "must default off");
        let mut req = request(json!({}));
        let before = ser(&req);
        assert_eq!(apply(&mut req, &cfg).unwrap_err(), Bypass::Disabled);
        assert_eq!(ser(&req), before);
    }

    #[test]
    fn every_bypass_leaves_the_request_untouched() {
        let on = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };
        let cases = [
            (json!({"stream": true}), Bypass::Streaming),
            (json!({"tools": []}), Bypass::NoTools),
            (json!({"tool_choice": "required"}), Bypass::ToolChoice),
            (
                json!({"tool_choice": {"type": "function", "function": {"name": "x"}}}),
                Bypass::ToolChoice,
            ),
            (
                json!({"messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function",
                        "function": {"name": "create_calendar_event", "arguments": "{}"}}]},
                    {"role": "tool", "tool_call_id": "c1", "content": "ok"}
                ]}),
                Bypass::History,
            ),
        ];
        for (extra, want) in cases {
            let mut req = request(extra);
            let before = ser(&req);
            assert_eq!(apply(&mut req, &on).unwrap_err(), want);
            assert_eq!(ser(&req), before);
        }

        let mut req = request(json!({"tools": [{"type": "function", "function": {
            "name": "f", "parameters": {"type": "object", "properties": {"a": {"anyOf": []}}}
        }}]}));
        let before = ser(&req);
        assert_eq!(
            apply(&mut req, &on).unwrap_err().as_label(),
            "unsupported_schema"
        );
        assert_eq!(ser(&req), before);

        let mut req = request(
            json!({"tools": [{"type": "function", "strict": true, "function": {
                "name": "f", "parameters": {"type": "object", "properties": {}}
            }}]}),
        );
        assert_eq!(
            apply(&mut req, &on).unwrap_err().as_label(),
            "unsupported_schema"
        );
    }

    #[test]
    fn compacts_into_a_leading_system_message() {
        let mut req = request(json!({}));
        let c = compact(&mut req).unwrap();
        assert!(req.tools.is_none() && req.tool_choice.is_none());
        assert!(!req.extra.contains_key("parallel_tool_calls"));
        assert_eq!(req.messages.len(), 3);
        assert_eq!(req.messages[0].role, "system");
        let prompt = req.messages[0].text().unwrap();
        assert!(prompt.contains("create_calendar_event(title:str, start:datetime)"));
        assert_eq!(c.prompt_bytes, prompt.len());
        assert_eq!(req.messages[1].text().unwrap(), "You are helpful.");
        assert_eq!(req.temperature, Some(0.2));
    }

    #[test]
    fn restores_standard_tool_calls() {
        let mut req = request(json!({}));
        let c = compact(&mut req).unwrap();
        let mut resp = response(
            "On it.\n<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
        );
        let mut n = 0;
        let count = restore(&mut resp, &c, || {
            n += 1;
            format!("call_{n}")
        })
        .unwrap();
        assert_eq!(count, 1);
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.content, Some(json!("On it.")));
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "create_calendar_event");
        assert_eq!(
            calls[0].function.arguments,
            r#"{"title":"Design review","start":"2026-10-05T15:00:00+05:30"}"#
        );
        // Wire shape the client sees.
        let wire = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            wire["choices"][0]["message"]["tool_calls"][0]["type"],
            "function"
        );
    }

    #[test]
    fn plain_answers_pass_through_and_bad_calls_fail() {
        let mut req = request(json!({}));
        let c = compact(&mut req).unwrap();

        let mut resp = response("I can't check the weather.");
        let before = ser(&resp);
        assert_eq!(restore(&mut resp, &c, String::new).unwrap(), 0);
        assert_eq!(ser(&resp), before);

        for bad in [
            "<<call delete_everything {}>>",
            "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
            "<<call create_calendar_event {\"title\":\"x\"",
        ] {
            let mut resp = response(bad);
            assert!(restore(&mut resp, &c, String::new).is_err(), "{bad}");
        }
    }
}
