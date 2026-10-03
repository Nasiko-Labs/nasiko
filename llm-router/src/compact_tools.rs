//! Opt-in compact tool schemas at the egress seam (`TOKEN_COMPACT_TOOLS`, default **off**).
//!
//! Replaces the request's native `tools` with one-line signatures plus a call-format instruction
//! (one trailing `system` message, like [`crate::brevity`]), then decodes the model's
//! `<<call NAME {JSON}>>` text back into standard `tool_calls` before the inbound renderer runs,
//! so OpenAI, Anthropic and Gemini clients all receive their native tool-call shape. The format
//! and its guarantees live in the `nasiko-tool-compact` crate; this module only converts between
//! the router IR and that crate's types and decides when compaction is safe.
//!
//! Coverage: non-streaming requests only. Everything this module does not cover is skipped with
//! a reason, and a skipped request is sent exactly as it would be without this module.
//!
//! Every call in a compacted reply is checked against the original schema before the client sees
//! it: compact calls are decoded and validated, and native `tool_calls` (a provider may still
//! emit them) are validated the same way. Calls are released only from a completed reply
//! (`finish_reason` `stop`, or `tool_calls` for native calls); a reply cut off by `length` or a
//! content filter, mixing both call forms, or holding any invalid call is a failure. Failure
//! policy: nothing is guessed, the caller re-sends the original native request once and returns
//! that reply instead.

use nasiko_tool_compact as compact;
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::ir::chat::{ChatResponse, Choice, FunctionCall, Message, ToolCall};
use crate::resolver::ResolvedConfig;

/// Why compaction was not applied.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    /// `TOKEN_COMPACT_TOOLS` is off (the default).
    Disabled,
    /// External coding CLIs parse their own tool protocol; never rewrite it.
    CodingAgent,
    NoTools,
    /// Streaming decode is not wired yet.
    Streaming,
    /// `tool_choice` other than `auto`: a forced or forbidden tool cannot be guaranteed by a
    /// prompt, so the provider's native enforcement is kept.
    ToolChoice,
    /// `parallel_tool_calls: false`: the compact format cannot forbid several calls.
    SequentialOnly,
    /// A `response_format` other than plain text: JSON mode would fight the call syntax.
    ResponseFormat,
    /// `n > 1`: one reply is re-sent natively on failure, so several choices are not compacted.
    MultipleChoices,
    /// Earlier native tool calls or results are in the transcript. Providers (Anthropic) reject
    /// tool history without tool definitions, and history is not re-rendered in compact form.
    ToolHistory,
    /// A non-`function` tool, or one with fields the compact format would drop.
    NonFunctionTool,
    /// A schema feature the grammar cannot express (see the `nasiko-tool-compact` docs).
    Unsupported,
}

impl Skipped {
    /// Stable label for logs.
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::CodingAgent => "coding_agent",
            Self::NoTools => "no_tools",
            Self::Streaming => "streaming",
            Self::ToolChoice => "tool_choice",
            Self::SequentialOnly => "sequential_only",
            Self::ResponseFormat => "response_format",
            Self::MultipleChoices => "multiple_choices",
            Self::ToolHistory => "tool_history",
            Self::NonFunctionTool => "non_function_tool",
            Self::Unsupported => "unsupported_schema",
        }
    }
}

/// A request that was compacted: what decoding needs, and the untouched original for the
/// native retry.
#[derive(Debug)]
pub(crate) struct Compacted {
    pub(crate) tools: Vec<compact::ToolDef>,
    pub(crate) native: ChatRequest,
}

/// Rewrites `req` to the compact form unless a carve-out applies. On `Err`, `req` is untouched.
pub(crate) fn apply(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
    resolved: &ResolvedConfig,
) -> Result<Compacted, Skipped> {
    if !cfg.compact_tools_enabled {
        return Err(Skipped::Disabled);
    }
    if resolved.is_coding_agent {
        return Err(Skipped::CodingAgent);
    }
    let native_tools = match &req.tools {
        Some(tools) if !tools.is_empty() => tools,
        _ => return Err(Skipped::NoTools),
    };
    if req.is_streaming() {
        return Err(Skipped::Streaming);
    }
    if !matches!(req.tool_choice.as_ref(), None | Some(Value::Null))
        && req.tool_choice.as_ref().and_then(Value::as_str) != Some("auto")
    {
        return Err(Skipped::ToolChoice);
    }
    if req.extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Err(Skipped::SequentialOnly);
    }
    if req
        .extra
        .get("response_format")
        .is_some_and(|f| !f.is_null() && f.get("type").and_then(Value::as_str) != Some("text"))
    {
        return Err(Skipped::ResponseFormat);
    }
    if req
        .extra
        .get("n")
        .and_then(Value::as_u64)
        .is_some_and(|n| n > 1)
    {
        return Err(Skipped::MultipleChoices);
    }
    if req
        .messages
        .iter()
        .any(|m| m.tool_calls.is_some() || m.role == "tool" || m.tool_call_id.is_some())
    {
        return Err(Skipped::ToolHistory);
    }
    if native_tools
        .iter()
        .any(|t| t.kind != "function" || !t.extra.is_empty())
    {
        return Err(Skipped::NonFunctionTool);
    }
    let tools: Vec<compact::ToolDef> = native_tools
        .iter()
        .map(|t| compact::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect();
    let encoded = compact::encode_tools(&tools).map_err(|_| Skipped::Unsupported)?;

    let native = req.clone();
    req.tools = None;
    req.tool_choice = None;
    req.messages.push(Message {
        role: "system".into(),
        content: Some(Value::String(encoded.system_prompt())),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    Ok(Compacted { tools, native })
}

/// Checks and restores a compacted reply. Returns the failure label on any problem, with the
/// response untouched; on success, compact calls have become native `tool_calls`.
///
/// Per choice: native `tool_calls` are validated against the original schemas and kept as sent
/// (their ids included); compact calls are decoded, validated, and replace the text, with the
/// prose around them as `content` (or `null`) and `finish_reason: "tool_calls"`. A choice with
/// neither is a plain answer and stays byte-identical.
pub(crate) fn decode_response(
    resp: &mut ChatResponse,
    tools: &[compact::ToolDef],
) -> Result<(), &'static str> {
    // Check every choice before changing any, so a failure leaves the response as received.
    let mut restored = Vec::with_capacity(resp.choices.len());
    for choice in &resp.choices {
        restored.push(check_choice(choice, tools)?);
    }
    for (choice, decoded) in resp.choices.iter_mut().zip(restored) {
        let Some(decoded) = decoded else { continue };
        let text = decoded.text.trim();
        choice.message.content = (!text.is_empty()).then(|| Value::String(text.to_string()));
        choice.message.tool_calls = Some(decoded.calls.iter().map(to_ir_call).collect());
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(())
}

/// `Some(decoded)` when the choice holds compact calls to restore, `None` when it needs no change.
fn check_choice(
    choice: &Choice,
    tools: &[compact::ToolDef],
) -> Result<Option<compact::Decoded>, &'static str> {
    let finish = choice.finish_reason.as_deref();
    let text = choice.message.text().unwrap_or_default();
    if let Some(native) = choice.message.tool_calls.as_ref().filter(|c| !c.is_empty()) {
        if text.contains(compact::CALL_OPEN) {
            return Err("mixed_call_representations");
        }
        if !matches!(finish, Some("tool_calls" | "stop")) {
            return Err("incomplete_completion");
        }
        for call in native {
            compact::validate_call(&call.function.name, &call.function.arguments, tools)
                .map_err(|e| e.kind())?;
        }
        return Ok(None);
    }
    let decoded = compact::decode(&text, tools).map_err(|e| e.kind())?;
    if decoded.calls.is_empty() {
        return Ok(None);
    }
    if finish != Some("stop") {
        return Err("incomplete_completion");
    }
    Ok(Some(decoded))
}

fn to_ir_call(call: &compact::ToolCall) -> ToolCall {
    ToolCall {
        id: format!("call_{}", uuid::Uuid::new_v4().simple()),
        kind: "function".into(),
        function: FunctionCall {
            name: call.name.clone(),
            arguments: call.arguments_json(),
        },
        extra: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolved(compress_enabled: bool, is_coding_agent: bool) -> ResolvedConfig {
        ResolvedConfig {
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            litellm_model: "openai/gpt-4o-mini".into(),
            api_key: "sk-test".into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent,
            compress_enabled,
        }
    }

    fn enabled() -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        }
    }

    fn request(extra: Value) -> ChatRequest {
        let mut body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "What's the weather in Pune?"}],
            "tools": [{"type": "function", "function": {
                "name": "get_weather",
                "description": "Weather for a city.",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}
            }}]
        });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            body[k] = v;
        }
        serde_json::from_value(body).unwrap()
    }

    fn skipped(req: &ChatRequest, cfg: &GatewayConfig, resolved: &ResolvedConfig) -> Skipped {
        let mut r = req.clone();
        let before = serde_json::to_string(&r).unwrap();
        let reason = apply(&mut r, cfg, resolved).unwrap_err();
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            before,
            "{reason:?} modified the request"
        );
        reason
    }

    #[test]
    fn off_by_default_and_byte_identical_when_off() {
        assert!(!GatewayConfig::default().compact_tools_enabled);
        let req = request(json!({}));
        assert_eq!(
            skipped(&req, &GatewayConfig::default(), &resolved(true, false)),
            Skipped::Disabled
        );
    }

    #[test]
    fn every_carve_out_leaves_the_request_byte_identical() {
        let on = enabled();
        let r = resolved(true, false);
        assert_eq!(
            skipped(&request(json!({})), &on, &resolved(true, true)),
            Skipped::CodingAgent
        );
        assert_eq!(
            skipped(&request(json!({"tools": null})), &on, &r),
            Skipped::NoTools
        );
        assert_eq!(
            skipped(&request(json!({"stream": true})), &on, &r),
            Skipped::Streaming
        );
        for choice in [
            json!("required"),
            json!("none"),
            json!({"type": "function", "function": {"name": "get_weather"}}),
        ] {
            assert_eq!(
                skipped(&request(json!({"tool_choice": choice})), &on, &r),
                Skipped::ToolChoice
            );
        }
        assert_eq!(
            skipped(&request(json!({"parallel_tool_calls": false})), &on, &r),
            Skipped::SequentialOnly
        );
        for format in [
            json!({"type": "json_object"}),
            json!({"type": "json_schema", "json_schema": {}}),
        ] {
            assert_eq!(
                skipped(&request(json!({"response_format": format})), &on, &r),
                Skipped::ResponseFormat
            );
        }
        assert_eq!(
            skipped(&request(json!({"n": 2})), &on, &r),
            Skipped::MultipleChoices
        );
        let history = json!({"messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "get_weather", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": "sunny"}
        ]});
        assert_eq!(skipped(&request(history), &on, &r), Skipped::ToolHistory);
        let pattern = json!({"tools": [{"type": "function", "function": {"name": "f",
            "parameters": {"type": "object", "properties": {"x": {"type": "string", "pattern": "^a"}}}}}]});
        assert_eq!(skipped(&request(pattern), &on, &r), Skipped::Unsupported);
    }

    #[test]
    fn applies_by_replacing_tools_with_a_trailing_system_message() {
        let mut req = request(json!({"tool_choice": "auto"}));
        let original = serde_json::to_string(&req).unwrap();
        let c = apply(&mut req, &enabled(), &resolved(true, false)).unwrap();
        assert!(req.tools.is_none() && req.tool_choice.is_none());
        let last = req.messages.last().unwrap();
        assert_eq!(last.role, "system");
        assert!(
            last.text()
                .unwrap()
                .starts_with("get_weather(city:str) - Weather for a city.\n")
        );
        assert_eq!(serde_json::to_string(&c.native).unwrap(), original);
        assert_eq!(c.tools[0].name, "get_weather");
    }

    fn response(content: &str) -> ChatResponse {
        ChatResponse {
            id: "r".into(),
            object: "chat.completion".into(),
            created: None,
            model: "m".into(),
            choices: vec![Choice {
                index: 0,
                message: serde_json::from_value(json!({"role": "assistant", "content": content}))
                    .unwrap(),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        }
    }

    fn tools() -> Vec<compact::ToolDef> {
        let mut req = request(json!({}));
        apply(&mut req, &enabled(), &resolved(true, false))
            .unwrap()
            .tools
    }

    #[test]
    fn compact_calls_become_native_tool_calls() {
        let mut resp = response(r#"Checking. <<call get_weather {"city":"Pune"}>>"#);
        decode_response(&mut resp, &tools()).unwrap();
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.content, Some(json!("Checking.")));
        let call = &choice.message.tool_calls.as_ref().unwrap()[0];
        assert!(call.id.starts_with("call_"));
        assert_eq!(call.function.name, "get_weather");
        assert_eq!(call.function.arguments, r#"{"city":"Pune"}"#);
    }

    #[test]
    fn a_plain_answer_is_left_byte_identical() {
        let mut resp = response("It is sunny in Pune.");
        let before = serde_json::to_string(&resp).unwrap();
        decode_response(&mut resp, &tools()).unwrap();
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn a_call_that_does_not_decode_is_an_error_and_changes_nothing() {
        for text in [
            r#"<<call delete_everything {}>>"#,
            r#"<<call get_weather {"town":"Pune"}>>"#,
            r#"<<call get_weather {"city":"Pune"}"#,
        ] {
            let mut resp = response(text);
            let before = serde_json::to_string(&resp).unwrap();
            assert!(decode_response(&mut resp, &tools()).is_err(), "{text}");
            assert_eq!(serde_json::to_string(&resp).unwrap(), before);
        }
    }

    #[test]
    fn the_flag_alone_turns_it_on_and_text_formats_still_compact() {
        // An agent without token optimization is compacted too: one operator flag decides.
        let mut req = request(json!({"response_format": {"type": "text"}, "n": 1}));
        assert!(apply(&mut req, &enabled(), &resolved(false, false)).is_ok());
    }

    fn with_finish(mut resp: ChatResponse, finish: &str) -> ChatResponse {
        resp.choices[0].finish_reason = Some(finish.into());
        resp
    }

    fn native(arguments: &str, content: Value) -> ChatResponse {
        let mut resp = response("");
        resp.choices[0].message = serde_json::from_value(json!({
            "role": "assistant", "content": content,
            "tool_calls": [{"id": "call_up", "type": "function",
                "function": {"name": "get_weather", "arguments": arguments}}]
        }))
        .unwrap();
        with_finish(resp, "tool_calls")
    }

    fn fails_untouched(mut resp: ChatResponse) -> &'static str {
        let before = serde_json::to_string(&resp).unwrap();
        let err = decode_response(&mut resp, &tools()).unwrap_err();
        assert_eq!(serde_json::to_string(&resp).unwrap(), before, "{err}");
        err
    }

    #[test]
    fn calls_are_released_only_from_a_completed_reply() {
        let call = r#"<<call get_weather {"city":"Pune"}>>"#;
        for finish in ["length", "content_filter", "tool_calls"] {
            assert_eq!(
                fails_untouched(with_finish(response(call), finish)),
                "incomplete_completion"
            );
        }
        // Without executable output a cut-off reply is just text and passes through.
        let mut resp = with_finish(response("It is sunny in"), "length");
        let before = serde_json::to_string(&resp).unwrap();
        decode_response(&mut resp, &tools()).unwrap();
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn native_tool_calls_are_validated_and_kept_as_sent() {
        let mut ok = native(r#"{"city":"Pune"}"#, Value::Null);
        let before = serde_json::to_string(&ok).unwrap();
        decode_response(&mut ok, &tools()).unwrap();
        assert_eq!(serde_json::to_string(&ok).unwrap(), before);

        assert_eq!(
            fails_untouched(native(r#"{"town":"Pune"}"#, Value::Null)),
            "invalid_arguments"
        );
        assert_eq!(
            fails_untouched(native(r#"{"city":"a","city":"b"}"#, Value::Null)),
            "invalid_arguments"
        );
        assert_eq!(
            fails_untouched(native(
                r#"{"city":"Pune"}"#,
                json!(r#"<<call get_weather {"city":"Goa"}>>"#)
            )),
            "mixed_call_representations"
        );
        assert_eq!(
            fails_untouched(with_finish(
                native(r#"{"city":"Pune"}"#, Value::Null),
                "length"
            )),
            "incomplete_completion"
        );
    }
}
