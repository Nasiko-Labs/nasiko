//! Compact tool schemas at the egress seam (opt-in).
//!
//! Native tool definitions are JSON Schema, re-sent in full on every turn. With
//! `TOKEN_COMPACT_TOOLS` on, a request's `tools` are replaced by compact definitions in a leading
//! system message (`nasiko-tool-compact`), the model answers with `<<call NAME {json}>>`, and the
//! reply is decoded back into standard `tool_calls` before the client sees it. The client never
//! sees the compact format.
//!
//! # Preserve or reject
//!
//! Compaction runs only when every semantic of the native request can be carried. Anything the
//! compact path cannot guarantee — a forced `tool_choice`, `parallel_tool_calls: false`, a
//! `response_format`, a conversation that already contains tool calls or results, a schema the
//! grammar cannot express, streaming — leaves the request untouched, and the reason is recorded.
//!
//! # Failure policy
//!
//! A reply that does not decode (unknown tool, invalid arguments, a broken marker) is never
//! passed on and never repaired: the original native request is sent once instead, so the client
//! gets exactly what it would have got without this layer. Usage from both attempts is billed.
//!
//! # Coverage
//!
//! Non-streaming `chat_core` requests: the OpenAI, Anthropic and Gemini inbound surfaces all
//! normalize to the same IR before this seam. Streaming requests and `/v1/responses` bypass.

use nasiko_tool_compact::{CompactError, decode_reply, encode_tools};
use serde_json::{Map, Value};

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::ir::chat::{ChatResponse, FunctionCall, Message, ToolCall, ToolDef, Usage};
use crate::resolver::ResolvedConfig;

/// What a compacted request needs to finish the round trip.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    /// The tools the reply is decoded against — the request's own, converted.
    tools: Vec<nasiko_tool_compact::ToolDef>,
    /// The request exactly as it was before compaction, for the native retry.
    native: ChatRequest,
}

impl Session {
    pub(crate) fn native_request(&self) -> &ChatRequest {
        &self.native
    }
}

/// Why compaction did not run, for telemetry and tests.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    /// The agent has token optimization switched off; that switch governs every layer.
    AgentOptedOut,
    NoTools,
    /// Calls are decoded from the whole reply; streaming is not wired yet.
    Streaming,
    /// `tool_choice` other than `auto`: a text grammar cannot force or forbid a call.
    ForcedToolChoice,
    /// `parallel_tool_calls: false`: a text grammar cannot enforce at most one call.
    ParallelDisabled,
    /// `response_format` constrains the reply to JSON, which the call grammar is not.
    ResponseFormat,
    /// Earlier assistant tool calls or tool results: they would have to be re-rendered.
    ToolHistory,
    /// A tool that is not a plain function definition.
    NonFunctionTool,
    /// A schema the compact grammar cannot carry exactly.
    UnsupportedSchema,
}

impl Skipped {
    /// Stable labels: changing one changes a queryable value, so they are spelled out.
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::AgentOptedOut => "agent_opted_out",
            Self::NoTools => "no_tools",
            Self::Streaming => "streaming",
            Self::ForcedToolChoice => "forced_tool_choice",
            Self::ParallelDisabled => "parallel_disabled",
            Self::ResponseFormat => "response_format",
            Self::ToolHistory => "tool_history",
            Self::NonFunctionTool => "non_function_tool",
            Self::UnsupportedSchema => "unsupported_schema",
        }
    }
}

/// Replace `req.tools` with compact definitions unless a carve-out applies. On `Ok`, the request
/// was modified and the returned session must be passed to [`restore`] with the reply.
pub(crate) fn apply(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
    resolved: &ResolvedConfig,
) -> Result<Session, Skipped> {
    if !cfg.compact_tools_enabled {
        return Err(Skipped::Disabled);
    }
    if !resolved.compress_enabled {
        return Err(Skipped::AgentOptedOut);
    }
    let tools = req
        .tools
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or(Skipped::NoTools)?;
    if req.is_streaming() {
        return Err(Skipped::Streaming);
    }
    check_carve_outs(req)?;
    let tools = tools
        .iter()
        .map(to_compact_tool)
        .collect::<Option<Vec<_>>>()
        .ok_or(Skipped::NonFunctionTool)?;
    let compact = encode_tools(&tools).map_err(|_| Skipped::UnsupportedSchema)?;

    let native = req.clone();
    req.tools = None;
    req.tool_choice = None;
    // Leading, so the definitions sit in the stable prefix providers cache on; author-written
    // messages are left byte-identical.
    req.messages
        .insert(0, system_message(compact.system_prompt()));
    Ok(Session { tools, native })
}

fn check_carve_outs(req: &ChatRequest) -> Result<(), Skipped> {
    let auto = req
        .tool_choice
        .as_ref()
        .is_none_or(|c| c.as_str() == Some("auto"));
    if !auto {
        return Err(Skipped::ForcedToolChoice);
    }
    if req.extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Err(Skipped::ParallelDisabled);
    }
    if req.extra.contains_key("response_format") {
        return Err(Skipped::ResponseFormat);
    }
    let has_tool_history = req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.as_ref().is_some_and(|c| !c.is_empty()));
    if has_tool_history {
        return Err(Skipped::ToolHistory);
    }
    Ok(())
}

/// Decode compact calls in the reply into standard `tool_calls`.
///
/// A reply without calls is left byte-identical. On `Err` the reply must not be used: the
/// caller sends [`Session::native_request`] instead.
pub(crate) fn restore(resp: &mut ChatResponse, session: &Session) -> Result<usize, CompactError> {
    let mut decoded_calls = 0;
    for choice in &mut resp.choices {
        if choice.message.tool_calls.is_some() {
            continue;
        }
        let text = choice.message.text().unwrap_or_default();
        let decoded = decode_reply(&text, &session.tools)?;
        if decoded.calls.is_empty() {
            continue;
        }
        decoded_calls += decoded.calls.len();
        choice.message.tool_calls = Some(decoded.calls.into_iter().map(to_ir_call).collect());
        choice.message.content =
            (!decoded.text.trim().is_empty()).then_some(Value::String(decoded.text));
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(decoded_calls)
}

/// Bill both attempts when a compact reply failed and the native request was sent instead.
pub(crate) fn add_failed_attempt_usage(usage: &mut Option<Usage>, failed: Option<Usage>) {
    let Some(failed) = failed else {
        return;
    };
    let total = usage.get_or_insert_with(Usage::default);
    let sum = |a: Option<i64>, b: Option<i64>| match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
    total.prompt_tokens = sum(total.prompt_tokens, failed.prompt_tokens);
    total.completion_tokens = sum(total.completion_tokens, failed.completion_tokens);
    total.total_tokens = sum(total.total_tokens, failed.total_tokens);
}

fn to_compact_tool(tool: &ToolDef) -> Option<nasiko_tool_compact::ToolDef> {
    // Outer fields beyond `type`/`function` (e.g. provider cache hints) have no compact form.
    (tool.kind == "function" && tool.extra.is_empty()).then(|| {
        nasiko_tool_compact::ToolDef::new(
            tool.function.name.clone(),
            tool.function.description.clone(),
            tool.function.parameters.clone(),
        )
    })
}

fn to_ir_call(call: nasiko_tool_compact::ToolCall) -> ToolCall {
    ToolCall {
        id: format!("call_{}", uuid::Uuid::new_v4().simple()),
        kind: "function".into(),
        function: FunctionCall {
            arguments: call.arguments_json(),
            name: call.name,
        },
        extra: Map::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Choice, FunctionDef};
    use serde_json::json;

    /// A change to a valid request that must make compaction step aside.
    type Mutation = Box<dyn Fn(&mut ChatRequest)>;

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: enabled,
            ..Default::default()
        }
    }

    fn resolved(compress_enabled: bool) -> ResolvedConfig {
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
            is_coding_agent: false,
            compress_enabled,
        }
    }

    fn weather_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "get_weather".into(),
                description: Some("Current weather for a city.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "city": {"type": "string", "description": "City name"},
                        "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]}
                    },
                    "required": ["city"]
                })),
            },
            extra: Map::new(),
        }
    }

    fn request() -> ChatRequest {
        serde_json::from_value(json!({
            "model": "gpt-4o-mini",
            "messages": [
                {"role": "system", "content": "You are a travel assistant."},
                {"role": "user", "content": "Weather in Paris?"}
            ],
            "tools": [serde_json::to_value(weather_tool()).unwrap()],
            "tool_choice": "auto"
        }))
        .unwrap()
    }

    fn reply(content: &str) -> ChatResponse {
        ChatResponse {
            id: "chatcmpl-1".into(),
            object: "chat.completion".into(),
            created: None,
            model: "gpt-4o-mini".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(Value::String(content.into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Map::new(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn disabled_leaves_the_request_byte_identical() {
        let mut req = request();
        let before = serde_json::to_string(&req).unwrap();

        assert_eq!(
            apply(&mut req, &cfg(false), &resolved(true)).unwrap_err(),
            Skipped::Disabled
        );

        assert_eq!(serde_json::to_string(&req).unwrap(), before);
    }

    #[test]
    fn the_per_agent_switch_stops_this_layer_too() {
        let mut req = request();
        let before = serde_json::to_string(&req).unwrap();
        assert_eq!(
            apply(&mut req, &cfg(true), &resolved(false)).unwrap_err(),
            Skipped::AgentOptedOut
        );
        assert_eq!(serde_json::to_string(&req).unwrap(), before);
    }

    #[test]
    fn applied_replaces_tools_with_a_leading_system_block_and_edits_nothing_else() {
        let mut req = request();
        let original = req.messages.clone();

        let session = apply(&mut req, &cfg(true), &resolved(true)).unwrap();

        assert!(req.tools.is_none() && req.tool_choice.is_none());
        let block = req.messages[0].text().unwrap();
        assert!(
            block.starts_with(nasiko_tool_compact::INSTRUCTION),
            "{block}"
        );
        assert!(block.contains("get_weather: Current weather for a city."));
        assert!(block.contains(" unit?: celsius|fahrenheit"));
        for (i, before) in original.iter().enumerate() {
            assert_eq!(
                serde_json::to_value(&req.messages[i + 1]).unwrap(),
                serde_json::to_value(before).unwrap(),
                "message {i} was rewritten"
            );
        }
        assert_eq!(
            serde_json::to_string(session.native_request()).unwrap(),
            serde_json::to_string(&request()).unwrap(),
            "the native retry must be the request as it arrived"
        );
    }

    #[test]
    fn every_carve_out_leaves_the_request_untouched_and_says_why() {
        let tool_call = json!({"id": "c1", "type": "function",
            "function": {"name": "get_weather", "arguments": "{\"city\":\"Rome\"}"}});
        let cases: Vec<(Mutation, Skipped)> = vec![
            (Box::new(|r| r.tools = None), Skipped::NoTools),
            (Box::new(|r| r.tools = Some(vec![])), Skipped::NoTools),
            (Box::new(|r| r.stream = Some(true)), Skipped::Streaming),
            (
                Box::new(|r| r.tool_choice = Some(json!("required"))),
                Skipped::ForcedToolChoice,
            ),
            (
                Box::new(|r| r.tool_choice = Some(json!("none"))),
                Skipped::ForcedToolChoice,
            ),
            (
                Box::new(|r| {
                    r.tool_choice =
                        Some(json!({"type": "function", "function": {"name": "get_weather"}}))
                }),
                Skipped::ForcedToolChoice,
            ),
            (
                Box::new(|r| {
                    r.extra.insert("parallel_tool_calls".into(), json!(false));
                }),
                Skipped::ParallelDisabled,
            ),
            (
                Box::new(|r| {
                    r.extra
                        .insert("response_format".into(), json!({"type": "json_object"}));
                }),
                Skipped::ResponseFormat,
            ),
            (
                Box::new(move |r| {
                    r.messages.push(
                        serde_json::from_value(json!({
                            "role": "assistant", "content": null, "tool_calls": [tool_call.clone()]
                        }))
                        .unwrap(),
                    );
                }),
                Skipped::ToolHistory,
            ),
            (
                Box::new(|r| {
                    r.messages.push(
                        serde_json::from_value(json!({
                            "role": "tool", "tool_call_id": "c1", "content": "sunny"
                        }))
                        .unwrap(),
                    );
                }),
                Skipped::ToolHistory,
            ),
            (
                Box::new(|r| {
                    if let Some(tools) = r.tools.as_mut() {
                        tools[0].kind = "web_search".into();
                    }
                }),
                Skipped::NonFunctionTool,
            ),
            (
                Box::new(|r| {
                    if let Some(tools) = r.tools.as_mut() {
                        tools[0].function.parameters = Some(json!({
                            "type": "object",
                            "properties": {"q": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}
                        }));
                    }
                }),
                Skipped::UnsupportedSchema,
            ),
        ];
        for (mutate, expected) in cases {
            let mut req = request();
            mutate(&mut req);
            let before = serde_json::to_string(&req).unwrap();
            assert_eq!(
                apply(&mut req, &cfg(true), &resolved(true)).unwrap_err(),
                expected
            );
            assert_eq!(serde_json::to_string(&req).unwrap(), before, "{expected:?}");
        }
    }

    #[test]
    fn a_compact_call_becomes_a_standard_tool_call() {
        let session = apply(&mut request(), &cfg(true), &resolved(true)).unwrap();
        let mut resp = reply(r#"<<call get_weather {"city":"Paris","unit":"celsius"}>>"#);

        assert_eq!(restore(&mut resp, &session).unwrap(), 1);

        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.content, None, "only a call: no stray text");
        let call = &choice.message.tool_calls.as_ref().unwrap()[0];
        assert!(call.id.starts_with("call_"));
        assert_eq!(call.kind, "function");
        assert_eq!(call.function.name, "get_weather");
        assert_eq!(
            call.function.arguments,
            r#"{"city":"Paris","unit":"celsius"}"#
        );
    }

    #[test]
    fn text_around_a_call_is_kept_as_content() {
        let session = apply(&mut request(), &cfg(true), &resolved(true)).unwrap();
        let mut resp = reply("Checking now.\n<<call get_weather {\"city\":\"Oslo\"}>>");
        restore(&mut resp, &session).unwrap();
        assert_eq!(
            resp.choices[0].message.content,
            Some(Value::String("Checking now.\n".into()))
        );
    }

    #[test]
    fn a_plain_answer_is_left_byte_identical() {
        let session = apply(&mut request(), &cfg(true), &resolved(true)).unwrap();
        let mut resp = reply("I can only check the weather.");
        let before = serde_json::to_string(&resp).unwrap();
        assert_eq!(restore(&mut resp, &session).unwrap(), 0);
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn an_undecodable_reply_is_an_error_never_a_guess() {
        let session = apply(&mut request(), &cfg(true), &resolved(true)).unwrap();
        for bad in [
            r#"<<call get_weather {"city":"Paris","unit":"kelvin"}>>"#,
            r#"<<call get_forecast {"city":"Paris"}>>"#,
            r#"<<call get_weather {"city":"Paris""#,
        ] {
            let mut resp = reply(bad);
            assert!(restore(&mut resp, &session).is_err(), "{bad}");
        }
    }

    #[test]
    fn both_attempts_are_billed_after_a_native_retry() {
        let mut usage = Some(Usage {
            prompt_tokens: Some(300),
            completion_tokens: Some(20),
            total_tokens: Some(320),
            ..Default::default()
        });
        let failed = Some(Usage {
            prompt_tokens: Some(120),
            completion_tokens: Some(15),
            total_tokens: Some(135),
            ..Default::default()
        });
        add_failed_attempt_usage(&mut usage, failed);
        let usage = usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(420));
        assert_eq!(usage.completion_tokens, Some(35));
        assert_eq!(usage.total_tokens, Some(455));
    }

    #[test]
    fn skip_labels_are_stable() {
        for (reason, label) in [
            (Skipped::Disabled, "disabled"),
            (Skipped::AgentOptedOut, "agent_opted_out"),
            (Skipped::NoTools, "no_tools"),
            (Skipped::Streaming, "streaming"),
            (Skipped::ForcedToolChoice, "forced_tool_choice"),
            (Skipped::ParallelDisabled, "parallel_disabled"),
            (Skipped::ResponseFormat, "response_format"),
            (Skipped::ToolHistory, "tool_history"),
            (Skipped::NonFunctionTool, "non_function_tool"),
            (Skipped::UnsupportedSchema, "unsupported_schema"),
        ] {
            assert_eq!(reason.as_label(), label);
        }
    }
}
