//! Opt-in tool-schema compaction at the egress seam.
//!
//! Off by default (`TOKEN_TOOL_COMPACT`). With the flag off, [`apply`] returns before reading or
//! changing anything, so the request and the response are byte-identical to today.
//!
//! When on, a non-streaming request whose tools are all in the codec's supported subset has its
//! native `tools` replaced by a compact system message, and the model's reply is decoded back into
//! standard `tool_calls`. Anything the codec cannot guarantee is skipped and sent natively:
//! streaming, forced `tool_choice`, conversations that already contain tool calls or results,
//! tools with fields beyond `name`/`description`/`parameters`, unsupported schema keywords, and
//! requests where compaction would not shrink the tool definitions.
//!
//! Decoding fails closed: an invalid, unknown or truncated call becomes an error to the caller,
//! never a guessed or repaired call. Covered: non-streaming chat completions on every inbound
//! surface (they share the IR). Not covered: streaming, and the Responses API handler.

use nasiko_tool_compact as tc;
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};

/// Compact only when the compact text is at most this fraction of the native tools JSON (bytes).
///
/// Calibrated against exact `o200k_base` token decisions on 24 tool sets (public sample, our own
/// cases, a 12-tool synthetic pool): every set where compaction saved tokens had a ratio of 0.737
/// or less, every set where it did not had 0.82 or more. 0.78 sits in that gap. It is a byte proxy
/// for a token decision, so near the boundary it can be wrong in either direction and it must be
/// re-derived whenever the instruction header in `tool-compact` changes; the offline eval uses
/// exact token counts instead.
pub(crate) const MAX_BYTE_RATIO: f64 = 0.78;

/// What compaction did to one request; needed again to decode the reply.
#[derive(Debug)]
pub(crate) struct Applied {
    tools: Vec<tc::ToolDef>,
    pub(crate) native_bytes: usize,
    pub(crate) compact_bytes: usize,
}

/// Why a request was sent natively.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Skip {
    Disabled,
    Streaming,
    NoTools,
    ForcedToolChoice,
    ToolHistory,
    UnsupportedToolShape,
    Unsupported(String),
    NoSaving,
    EncodeError,
}

/// Replace `req.tools` with compact system text when it is safe and smaller. On `Err` the request
/// is untouched.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Result<Applied, Skip> {
    if !cfg.tool_compact_enabled {
        return Err(Skip::Disabled);
    }
    if req.is_streaming() {
        return Err(Skip::Streaming);
    }
    let native = match req.tools.as_deref() {
        Some(tools) if !tools.is_empty() => tools,
        _ => return Err(Skip::NoTools),
    };
    match &req.tool_choice {
        None => {}
        Some(Value::String(s)) if s == "auto" => {}
        Some(_) => return Err(Skip::ForcedToolChoice),
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.is_some())
    {
        return Err(Skip::ToolHistory);
    }
    // A field we would drop (for example `strict`) changes what the provider enforces.
    if native
        .iter()
        .any(|t| t.kind != "function" || !t.extra.is_empty())
    {
        return Err(Skip::UnsupportedToolShape);
    }

    let defs: Vec<tc::ToolDef> = native
        .iter()
        .map(|t| tc::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect();
    let encoded = tc::encode_tools(&defs).map_err(|_| Skip::EncodeError)?;
    if let Some(b) = encoded.bypassed.first() {
        return Err(Skip::Unsupported(format!("{}:{}", b.tool, b.reason)));
    }
    let native_bytes = serde_json::to_string(native).map(|s| s.len()).unwrap_or(0);
    let compact_bytes = encoded.text.len();
    if native_bytes == 0 || compact_bytes as f64 > MAX_BYTE_RATIO * native_bytes as f64 {
        return Err(Skip::NoSaving);
    }

    req.tools = None;
    req.messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(Value::String(encoded.text)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    Ok(Applied {
        tools: defs,
        native_bytes,
        compact_bytes,
    })
}

/// Turn compact calls in the reply into standard `tool_calls`. A reply with no call is a plain
/// answer and is left as is. Any invalid call is an error: nothing is guessed.
pub(crate) fn decode_response(
    resp: &mut ChatResponse,
    applied: &Applied,
) -> Result<(), GatewayError> {
    for choice in &mut resp.choices {
        let Some(text) = choice.message.text() else {
            continue;
        };
        let calls = tc::decode_calls(&text, &applied.tools).map_err(|e| {
            GatewayError::Upstream(format!(
                "model wrote an invalid compact tool call: {}",
                e.code()
            ))
        })?;
        if calls.is_empty() {
            continue;
        }
        choice.message.tool_calls = Some(
            calls
                .into_iter()
                .enumerate()
                .map(|(i, c)| ToolCall {
                    id: format!("call_{}_{}", choice.index, i),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: c.name,
                        arguments: c.arguments,
                    },
                    extra: Default::default(),
                })
                .collect(),
        );
        choice.message.content = None;
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn cfg(on: bool) -> GatewayConfig {
        GatewayConfig {
            tool_compact_enabled: on,
            ..Default::default()
        }
    }

    fn weather_tools() -> Value {
        json!([
            {"type": "function", "function": {
                "name": "get_weather", "description": "Get the current weather for a city.",
                "parameters": {"type": "object", "properties": {
                    "city": {"type": "string", "description": "City name"},
                    "units": {"type": "string", "enum": ["metric", "imperial"]}}, "required": ["city"]}}},
            {"type": "function", "function": {
                "name": "send_message", "description": "Send a chat message to a channel or person.",
                "parameters": {"type": "object", "properties": {
                    "channel": {"type": "string", "description": "Channel or user id"},
                    "text": {"type": "string", "description": "Message text"},
                    "thread_id": {"type": "string", "description": "Reply in this thread"}}, "required": ["channel", "text"]}}},
            {"type": "function", "function": {
                "name": "create_event", "description": "Create a calendar event.",
                "parameters": {"type": "object", "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"}}, "required": ["title", "start"]}}}
        ])
    }

    fn request(extra: Value) -> ChatRequest {
        let mut body = json!({
            "messages": [{"role": "user", "content": "weather in Pune?"}],
            "tools": weather_tools()
        });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            body[k] = v;
        }
        serde_json::from_value(body).unwrap()
    }

    fn snapshot(req: &ChatRequest) -> String {
        serde_json::to_string(req).unwrap()
    }

    #[test]
    fn flag_is_off_by_default() {
        assert!(!GatewayConfig::default().tool_compact_enabled);
    }

    #[test]
    fn disabled_leaves_the_request_byte_identical() {
        let mut req = request(json!({}));
        let before = snapshot(&req);
        assert_eq!(apply(&mut req, &cfg(false)).unwrap_err(), Skip::Disabled);
        assert_eq!(snapshot(&req), before);
    }

    #[test]
    fn unsafe_requests_are_skipped_and_untouched() {
        let cases: Vec<(Value, Skip)> = vec![
            (json!({"stream": true}), Skip::Streaming),
            (json!({"tool_choice": "required"}), Skip::ForcedToolChoice),
            (
                json!({"tool_choice": {"type": "function", "function": {"name": "get_weather"}}}),
                Skip::ForcedToolChoice,
            ),
            (json!({"tools": []}), Skip::NoTools),
            (
                json!({"messages": [{"role": "user", "content": "x"}, {"role": "tool", "content": "r", "tool_call_id": "c1"}]}),
                Skip::ToolHistory,
            ),
            (
                json!({"messages": [{"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "get_weather", "arguments": "{}"}}]}]}),
                Skip::ToolHistory,
            ),
        ];
        for (extra, want) in cases {
            let mut req = request(extra.clone());
            let before = snapshot(&req);
            assert_eq!(apply(&mut req, &cfg(true)).unwrap_err(), want, "{extra}");
            assert_eq!(snapshot(&req), before, "{extra}");
        }
    }

    #[test]
    fn tools_with_extra_fields_or_unsupported_schemas_are_skipped() {
        let mut strict = request(json!({}));
        if let Some(tools) = strict.tools.as_mut() {
            tools[0].extra.insert("strict".into(), json!(true));
        }
        assert_eq!(
            apply(&mut strict, &cfg(true)).unwrap_err(),
            Skip::UnsupportedToolShape
        );

        let mut bounded = request(json!({"tools": [{"type": "function", "function": {
            "name": "resize", "description": "Resize an image to the given width in pixels.",
            "parameters": {"type": "object", "properties": {"width": {"type": "integer", "minimum": 1}}, "required": ["width"]}}}]}));
        let before = snapshot(&bounded);
        assert!(matches!(
            apply(&mut bounded, &cfg(true)).unwrap_err(),
            Skip::Unsupported(r) if r.contains("unsupported:minimum")
        ));
        assert_eq!(snapshot(&bounded), before);
    }

    #[test]
    fn tiny_tools_that_would_not_shrink_are_skipped() {
        let mut req = request(json!({"tools": [{"type": "function", "function": {
            "name": "ping", "description": "Ping.", "parameters": {"type": "object", "properties": {"a": {"type": "string"}}}}}]}));
        let before = snapshot(&req);
        assert_eq!(apply(&mut req, &cfg(true)).unwrap_err(), Skip::NoSaving);
        assert_eq!(snapshot(&req), before);
    }

    #[test]
    fn applied_request_drops_native_tools_and_adds_a_system_message() {
        let mut req = request(json!({}));
        let applied = apply(&mut req, &cfg(true)).unwrap();
        assert!(req.tools.is_none());
        assert_eq!(req.messages[0].role, "system");
        let text = req.messages[0].text().unwrap();
        assert!(text.contains("get_weather(city:str, units?:metric|imperial)"));
        assert_eq!(req.messages[1].role, "user");
        assert!(applied.compact_bytes < applied.native_bytes);
    }

    fn reply(text: &str) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "r1", "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
        }))
        .unwrap()
    }

    fn applied() -> Applied {
        let mut req = request(json!({}));
        apply(&mut req, &cfg(true)).unwrap()
    }

    #[test]
    fn valid_compact_call_becomes_a_standard_tool_call() {
        let mut resp = reply(r#"<<call get_weather {"city":"Pune","units":"metric"}>>"#);
        decode_response(&mut resp, &applied()).unwrap();
        let msg = &resp.choices[0].message;
        let calls = msg.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(
            calls[0].function.arguments,
            r#"{"city":"Pune","units":"metric"}"#
        );
        assert_eq!(calls[0].id, "call_0_0");
        assert!(msg.content.is_none());
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn plain_answer_is_left_alone() {
        let mut resp = reply("It is sunny.");
        decode_response(&mut resp, &applied()).unwrap();
        assert!(resp.choices[0].message.tool_calls.is_none());
        assert_eq!(
            resp.choices[0].message.text().as_deref(),
            Some("It is sunny.")
        );
    }

    #[test]
    fn invalid_calls_fail_closed() {
        for bad in [
            r#"<<call get_weather {"units":"metric"}>>"#,
            r#"<<call get_weather {"city":"Pune","units":"kelvin"}>>"#,
            r#"<<call delete_all {}>>"#,
            r#"<<call get_weather {"city":"Pune"}"#,
        ] {
            let mut resp = reply(bad);
            assert!(decode_response(&mut resp, &applied()).is_err(), "{bad}");
            assert!(resp.choices[0].message.tool_calls.is_none(), "{bad}");
        }
    }
}
