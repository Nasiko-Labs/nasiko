//! Compact tool calling at the router seam (P1): request rewrite and response decode.
//!
//! The format and the validating decoder live in `nasiko-tool-compact`, a pure library with
//! its own types; this module converts the router IR to and from those types and decides
//! when compaction is allowed at all. Everything here is fail-closed in the same sense as
//! [`crate::compress`]: any doubt means the request goes out exactly as it came in.
//!
//! # When a request bypasses
//!
//! See [`Bypass`]. In short: no tools; a `tool_choice` other than `auto` (a forced or
//! required call cannot be guaranteed by a prompt); `parallel_tool_calls: false`; legacy
//! `functions`; a schema the compact format cannot carry; history that cannot be replayed;
//! or a compact body that is not actually smaller.
//!
//! # History
//!
//! Previous assistant `tool_calls` are replayed as `<<call …>>` text in the assistant
//! message, and each `tool` result becomes a `user` message headed `<<result NAME>>`, so the
//! model sees its own earlier calls in the same format it is asked to write.

use nasiko_tool_compact as tc;
use serde_json::Value;

use crate::ir::ChatRequest;
use crate::ir::chat::{FunctionCall, Message, ToolCall, ToolDef};

/// Why a request was sent with native tools instead of compact ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bypass {
    NoTools,
    /// `tool_choice` is `none`, `required` or a named function.
    ToolChoice,
    ParallelToolCallsOff,
    LegacyFunctions,
    /// A tool uses a schema feature the compact format cannot carry (the crate's error).
    Unsupported(String),
    /// History has a tool result whose call cannot be found, or unparseable call arguments.
    History(String),
    /// The compact body is not smaller than the native one.
    NotSmaller,
}

impl Bypass {
    /// Stable label for telemetry and eval output.
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::NoTools => "no_tools",
            Self::ToolChoice => "tool_choice",
            Self::ParallelToolCallsOff => "parallel_tool_calls_off",
            Self::LegacyFunctions => "legacy_functions",
            Self::Unsupported(_) => "unsupported_schema",
            Self::History(_) => "history",
            Self::NotSmaller => "not_smaller",
        }
    }
}

/// A rewritten request plus what is needed to decode its response.
#[derive(Debug, Clone)]
pub struct Compacted {
    pub request: ChatRequest,
    /// The tools offered, in the crate's types, for [`decode_text`] / [`tc::StreamDecoder`].
    pub tools: Vec<tc::ToolDef>,
}

/// Router tool → crate tool.
pub fn to_compact_tool(t: &ToolDef) -> tc::ToolDef {
    tc::ToolDef {
        kind: t.kind.clone(),
        function: tc::FunctionDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        },
        extra: t.extra.clone(),
    }
}

/// Crate call → router call. The router assigns ids; `id` must be unique within the response.
pub fn to_router_call(c: tc::ToolCall, id: String) -> ToolCall {
    ToolCall {
        id,
        kind: "function".into(),
        function: FunctionCall {
            name: c.name,
            arguments: c.arguments,
        },
        extra: Default::default(),
    }
}

fn text_message(role: &str, text: String) -> Message {
    Message {
        role: role.into(),
        content: Some(Value::String(text)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    }
}

/// Replay history in the compact call format. `Err` names what could not be replayed.
fn replay_history(messages: &[Message]) -> Result<Vec<Message>, String> {
    let mut out = Vec::with_capacity(messages.len());
    // tool_call_id → tool name, from earlier assistant messages.
    let mut names: Vec<(String, String)> = Vec::new();
    for m in messages {
        if let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
            let crate_calls: Vec<tc::ToolCall> = calls
                .iter()
                .map(|c| tc::ToolCall {
                    name: c.function.name.clone(),
                    arguments: c.function.arguments.clone(),
                })
                .collect();
            let rendered = tc::render_calls(&crate_calls).map_err(|e| e.to_string())?;
            for c in calls {
                names.push((c.id.clone(), c.function.name.clone()));
            }
            let text = match m.text() {
                Some(t) if !t.trim().is_empty() => format!("{t}\n{rendered}"),
                _ => rendered,
            };
            out.push(Message {
                tool_calls: None,
                content: Some(Value::String(text)),
                ..m.clone()
            });
        } else if m.role == "tool" {
            let id = m.tool_call_id.as_deref().unwrap_or_default();
            let Some((_, name)) = names.iter().find(|(i, _)| i == id) else {
                return Err(format!("tool result '{id}' has no matching call"));
            };
            let body = match &m.content {
                Some(Value::String(s)) => s.clone(),
                Some(other) => m.text().unwrap_or_else(|| other.to_string()),
                None => String::new(),
            };
            out.push(text_message("user", format!("<<result {name}>>\n{body}")));
        } else {
            out.push(m.clone());
        }
    }
    Ok(out)
}

/// Rewrite `req` to use compact tool definitions, or say why not.
///
/// Pure: no IO, no config. The caller decides whether compaction is enabled at all.
pub fn compact_request(req: &ChatRequest) -> Result<Compacted, Bypass> {
    let tools = match &req.tools {
        Some(t) if !t.is_empty() => t,
        _ => return Err(Bypass::NoTools),
    };
    match &req.tool_choice {
        None => {}
        Some(Value::String(s)) if s == "auto" => {}
        Some(_) => return Err(Bypass::ToolChoice),
    }
    if req.extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Err(Bypass::ParallelToolCallsOff);
    }
    if req.extra.contains_key("functions") || req.extra.contains_key("function_call") {
        return Err(Bypass::LegacyFunctions);
    }

    let crate_tools: Vec<tc::ToolDef> = tools.iter().map(to_compact_tool).collect();
    let compact = tc::encode_tools(&crate_tools).map_err(|e| Bypass::Unsupported(e.to_string()))?;
    let history = replay_history(&req.messages).map_err(Bypass::History)?;

    let mut messages = Vec::with_capacity(history.len() + 1);
    // First, so the definitions sit in the stable prefix like native tools do (prompt caching).
    messages.push(text_message("system", compact.system_prompt()));
    messages.extend(history);

    let mut extra = req.extra.clone();
    extra.remove("parallel_tool_calls");
    let request = ChatRequest {
        messages,
        tools: None,
        tool_choice: None,
        extra,
        ..req.clone()
    };

    // Bytes are a cheap proxy for tokens; a rewrite that is not smaller is not worth the risk.
    let size = |r: &ChatRequest| serde_json::to_vec(r).map(|v| v.len()).unwrap_or(usize::MAX);
    if size(&request) >= size(req) {
        return Err(Bypass::NotSmaller);
    }
    Ok(Compacted {
        request,
        tools: crate_tools,
    })
}

/// What the request seam did: the tools to decode with, and the untouched original to resend
/// if the compact reply cannot be decoded.
#[derive(Debug, Clone)]
pub(crate) struct Applied {
    pub tools: Vec<tc::ToolDef>,
    pub original: ChatRequest,
}

/// The router seam. `None` leaves `req` untouched — always the case with the flag off.
///
/// Wired for non-streaming requests only (OpenAI, Anthropic and Gemini inbound alike: the
/// rewrite happens on the shared IR). A streaming request keeps its native tools.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &crate::config::GatewayConfig) -> Option<Applied> {
    if !cfg.compact_tools_enabled || req.is_streaming() {
        return None;
    }
    match compact_request(req) {
        Ok(c) => {
            let original = std::mem::replace(req, c.request);
            Some(Applied {
                tools: c.tools,
                original,
            })
        }
        Err(bypass) => {
            tracing::debug!(
                target: "nasiko::llm_router::compact_tools",
                reason = bypass.as_label(),
                "compact_tools: bypassed, sending native tools"
            );
            None
        }
    }
}

/// Turn compact calls in a response back into native `tool_calls`, in place.
///
/// All-or-nothing: on `Err` the response is left exactly as received, and the caller must not
/// return it as a tool-calling response. A choice with no calls is left as plain text.
pub(crate) fn restore_response(
    resp: &mut crate::ir::chat::ChatResponse,
    tools: &[tc::ToolDef],
) -> Result<(), tc::Error> {
    let mut decoded = Vec::with_capacity(resp.choices.len());
    for choice in &resp.choices {
        let text = choice.message.text().unwrap_or_default();
        let prefix = format!("call_{}_{}", resp.id, choice.index);
        decoded.push(decode_text(&text, tools, &prefix)?);
    }
    for (choice, (text, calls)) in resp.choices.iter_mut().zip(decoded) {
        if calls.is_empty() {
            continue;
        }
        let text = text.trim();
        choice.message.content = (!text.is_empty()).then(|| Value::String(text.to_string()));
        choice.message.tool_calls = Some(calls);
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(())
}

/// Decode a complete compact response into text and router tool calls with ids
/// `{id_prefix}_{index}`.
///
/// The text returned is what the model wrote *before* its first call (all of it when there are
/// no calls). A native tool-calling turn ends at the calls; text after them is the model
/// narrating results it has not received yet (seen live: invented flight tables), so it is
/// dropped rather than handed to the agent as fact. Calls and text in between are still
/// validated in full.
pub fn decode_text(
    text: &str,
    tools: &[tc::ToolDef],
    id_prefix: &str,
) -> Result<(String, Vec<ToolCall>), tc::Error> {
    let mut decoder = tc::StreamDecoder::new(tools)?;
    let mut events = decoder.push(text)?;
    events.extend(decoder.finish()?);
    let mut lead = String::new();
    let mut calls = Vec::new();
    for e in events {
        match e {
            tc::StreamEvent::Text(t) if calls.is_empty() => lead.push_str(&t),
            tc::StreamEvent::Text(_) => {}
            tc::StreamEvent::Call { index, call } => {
                calls.push(to_router_call(call, format!("{id_prefix}_{index}")));
            }
        }
    }
    Ok((lead, calls))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(body: Value) -> ChatRequest {
        serde_json::from_value(body).unwrap()
    }

    fn weather_tools() -> Value {
        json!([{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather for a city.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string", "description": "City name"},
                        "unit": {"type": "string", "enum": ["c", "f"]}
                    },
                    "required": ["city"]
                }
            }
        }])
    }

    #[test]
    fn rewrites_tools_into_a_leading_system_message() {
        let r = req(json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "Weather in Pune?"}],
            "tools": weather_tools(),
            "tool_choice": "auto",
            "temperature": 0.2
        }));
        let c = compact_request(&r).unwrap();
        assert!(c.request.tools.is_none());
        assert!(c.request.tool_choice.is_none());
        assert_eq!(c.request.messages.len(), 2);
        assert_eq!(c.request.messages[0].role, "system");
        let sys = c.request.messages[0].text().unwrap();
        assert!(sys.contains("get_weather: Get the current weather for a city."));
        assert!(sys.contains(" city: str # City name"));
        assert_eq!(c.request.temperature, Some(0.2));
        assert_eq!(c.request.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn bypass_reasons() {
        let base =
            json!({"messages": [{"role": "user", "content": "hi"}], "tools": weather_tools()});
        let with = |k: &str, v: Value| {
            let mut b = base.clone();
            b[k] = v;
            req(b)
        };
        assert_eq!(
            compact_request(&req(
                json!({"messages": [{"role": "user", "content": "hi"}]})
            ))
            .unwrap_err(),
            Bypass::NoTools
        );
        for choice in [
            json!("required"),
            json!("none"),
            json!({"type": "function", "function": {"name": "get_weather"}}),
        ] {
            assert_eq!(
                compact_request(&with("tool_choice", choice)).unwrap_err(),
                Bypass::ToolChoice
            );
        }
        assert_eq!(
            compact_request(&with("parallel_tool_calls", json!(false))).unwrap_err(),
            Bypass::ParallelToolCallsOff
        );
        let mut t = weather_tools();
        t[0]["function"]["parameters"]["properties"]["city"] =
            json!({"anyOf": [{"type": "string"}]});
        assert_eq!(
            compact_request(&with("tools", t)).unwrap_err().as_label(),
            "unsupported_schema"
        );
    }

    #[test]
    fn history_is_replayed_in_the_compact_format() {
        let r = req(json!({
            "messages": [
                {"role": "user", "content": "Weather in Pune?"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Pune\"}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": "31C, clear"}
            ],
            "tools": weather_tools()
        }));
        let c = compact_request(&r).unwrap();
        let m = &c.request.messages;
        assert_eq!(m[2].role, "assistant");
        assert!(m[2].tool_calls.is_none());
        assert_eq!(
            m[2].text().unwrap(),
            "<<call get_weather {\"city\":\"Pune\"}>>"
        );
        assert_eq!(m[3].role, "user");
        assert_eq!(m[3].text().unwrap(), "<<result get_weather>>\n31C, clear");
    }

    #[test]
    fn an_orphan_tool_result_bypasses() {
        let r = req(json!({
            "messages": [{"role": "tool", "tool_call_id": "call_9", "content": "x"}],
            "tools": weather_tools()
        }));
        assert_eq!(compact_request(&r).unwrap_err().as_label(), "history");
    }

    fn cfg(enabled: bool) -> crate::config::GatewayConfig {
        crate::config::GatewayConfig {
            compact_tools_enabled: enabled,
            ..Default::default()
        }
    }

    fn tool_request() -> ChatRequest {
        req(json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": "Weather in Pune?"}],
            "tools": weather_tools(),
            "tool_choice": "auto",
            "top_p": 0.9
        }))
    }

    #[test]
    fn the_flag_is_off_by_default() {
        assert!(!crate::config::GatewayConfig::default().compact_tools_enabled);
    }

    #[test]
    fn flag_off_leaves_the_request_byte_identical() {
        let mut r = tool_request();
        let before = serde_json::to_string(&r).unwrap();
        assert!(apply(&mut r, &cfg(false)).is_none());
        assert_eq!(serde_json::to_string(&r).unwrap(), before);
    }

    #[test]
    fn streaming_requests_keep_native_tools() {
        let mut r = tool_request();
        r.stream = Some(true);
        let before = serde_json::to_string(&r).unwrap();
        assert!(apply(&mut r, &cfg(true)).is_none());
        assert_eq!(serde_json::to_string(&r).unwrap(), before);
    }

    #[test]
    fn flag_on_rewrites_and_keeps_the_original_for_fallback() {
        let mut r = tool_request();
        let before = serde_json::to_string(&r).unwrap();
        let applied = apply(&mut r, &cfg(true)).unwrap();
        assert!(r.tools.is_none());
        assert_eq!(serde_json::to_string(&applied.original).unwrap(), before);
        assert_eq!(
            r.extra.get("top_p"),
            Some(&json!(0.9)),
            "other params pass through"
        );
    }

    fn response(content: &str) -> crate::ir::chat::ChatResponse {
        serde_json::from_value(json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o-mini",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
        }))
        .unwrap()
    }

    #[test]
    fn a_compact_reply_becomes_native_tool_calls() {
        let mut r = tool_request();
        let applied = apply(&mut r, &cfg(true)).unwrap();
        let mut resp = response("<<call get_weather {\"city\":\"Pune\",\"unit\":\"c\"}>>");
        restore_response(&mut resp, &applied.tools).unwrap();
        let m = &resp.choices[0].message;
        assert_eq!(m.content, None);
        let call = &m.tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.id, "call_chatcmpl-1_0_0");
        assert_eq!(call.function.name, "get_weather");
        assert_eq!(
            call.function.arguments,
            "{\"city\":\"Pune\",\"unit\":\"c\"}"
        );
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn text_after_the_calls_is_dropped() {
        let mut r = tool_request();
        let applied = apply(&mut r, &cfg(true)).unwrap();
        let mut resp = response(
            "Let me check.\n<<call get_weather {\"city\":\"Pune\"}>>It is 31C and sunny in Pune!",
        );
        restore_response(&mut resp, &applied.tools).unwrap();
        let m = &resp.choices[0].message;
        assert_eq!(m.text().as_deref(), Some("Let me check."));
        assert_eq!(m.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn a_plain_reply_is_left_alone() {
        let mut r = tool_request();
        let applied = apply(&mut r, &cfg(true)).unwrap();
        let mut resp = response("It is sunny.");
        let before = serde_json::to_string(&resp).unwrap();
        restore_response(&mut resp, &applied.tools).unwrap();
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn an_invalid_reply_is_an_error_and_leaves_the_response_untouched() {
        let mut r = tool_request();
        let applied = apply(&mut r, &cfg(true)).unwrap();
        let mut resp = response("<<call get_weather {\"unit\":\"kelvin\"}>>");
        let before = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            restore_response(&mut resp, &applied.tools)
                .unwrap_err()
                .code(),
            "invalid_arguments"
        );
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn decoded_calls_get_router_ids_and_unaltered_arguments() {
        let r = req(json!({"messages": [], "tools": weather_tools()}));
        let tools: Vec<tc::ToolDef> = r
            .tools
            .as_ref()
            .unwrap()
            .iter()
            .map(to_compact_tool)
            .collect();
        let (text, calls) = decode_text(
            "Checking.\n<<call get_weather {\"city\": \"Pune\"}>>",
            &tools,
            "call_abc",
        )
        .unwrap();
        assert_eq!(text, "Checking.\n");
        assert_eq!(calls[0].id, "call_abc_0");
        assert_eq!(calls[0].function.arguments, "{\"city\": \"Pune\"}");
        assert!(decode_text("<<call get_weather {}>>", &tools, "x").is_err());
    }
}
