//! Compact tool schemas at the egress seam (opt-in, `COMPACT_TOOLS_ENABLED`, default off).
//!
//! When it applies, the request's native `tools` are replaced by one system message carrying
//! the compact definitions ([`nasiko_tool_compact`]), and the model's `<<call …>>` text is turned
//! back into ordinary OpenAI-shaped `tool_calls` before the response is rendered — so the agent
//! never sees the difference. Every inbound surface on `chat_core` (OpenAI, Anthropic, Gemini)
//! renders from the same IR, so all three are covered; `/v1/responses` is not on `chat_core`
//! and is never compacted.
//!
//! Runs **after** `brevity::apply` (brevity reads `req.tools` to detect a tool loop; clearing it
//! first would silently disable that check) and before `sent_bytes` is measured. Like every
//! layer here it is a pure function of the request, so a given tool list renders identical bytes
//! every turn and provider prompt-cache prefixes stay stable.
//!
//! Fail-closed: a response whose calls do not decode and validate is an error, never a guess.

use futures::StreamExt;
use futures::stream::BoxStream;
use nasiko_tool_compact::{self as tc, Event};
use serde_json::{Value, json};

use crate::config::GatewayConfig;
use crate::ir::chat::{
    ChatChunk, ChatRequest, ChatResponse, ChunkChoice, Delta, FunctionCall, FunctionCallDelta,
    Message, ToolCall, ToolCallDelta,
};
use crate::providers::ProviderError;
use crate::resolver::ResolvedConfig;

/// Why compaction was not applied. The request is left exactly as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Skipped {
    /// `COMPACT_TOOLS_ENABLED` is off (the default).
    Disabled,
    /// The agent's token-optimization switch (`compress_enabled`) is off; it governs every layer.
    AgentOptedOut,
    /// Coding-agent CLIs: large, schema-rich tool sets and constant tool history — always native.
    CodingAgent,
    NoTools,
    /// `tool_choice` other than `"auto"` (required / named / none) needs native enforcement.
    ForcedToolChoice,
    /// A non-function tool, or one carrying fields compaction cannot preserve.
    NonFunctionTool,
    /// `parallel_tool_calls: false` cannot be guaranteed by a text protocol.
    ParallelCallsDisabled,
    /// Prior tool calls/results in the transcript (v1 compacts the first tool turn only).
    ToolHistory,
    /// A schema outside the supported subset (or one that would not round-trip).
    UnsupportedSchema,
    /// Compact text would not be smaller than the native tools.
    NotSmaller,
}

impl Skipped {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Skipped::Disabled => "disabled",
            Skipped::AgentOptedOut => "agent_opted_out",
            Skipped::CodingAgent => "coding_agent",
            Skipped::NoTools => "no_tools",
            Skipped::ForcedToolChoice => "forced_tool_choice",
            Skipped::NonFunctionTool => "non_function_tool",
            Skipped::ParallelCallsDisabled => "parallel_calls_disabled",
            Skipped::ToolHistory => "tool_history",
            Skipped::UnsupportedSchema => "unsupported_schema",
            Skipped::NotSmaller => "not_smaller",
        }
    }
}

/// A request that was compacted: the tools its response must be decoded against.
#[derive(Debug, Clone)]
pub(crate) struct Applied {
    pub tools: Vec<tc::ToolDef>,
    pub native_bytes: usize,
    pub compact_bytes: usize,
}

pub(crate) type Outcome = Result<Applied, Skipped>;

/// Replace `req.tools` with compact definitions, or leave the request untouched.
pub(crate) fn apply(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
    resolved: &ResolvedConfig,
) -> Outcome {
    if !cfg.compact_tools_enabled {
        return Err(Skipped::Disabled);
    }
    if !resolved.compress_enabled {
        return Err(Skipped::AgentOptedOut);
    }
    if resolved.is_coding_agent {
        return Err(Skipped::CodingAgent);
    }
    let Some(native) = req.tools.as_ref().filter(|t| !t.is_empty()) else {
        return Err(Skipped::NoTools);
    };
    match &req.tool_choice {
        None => {}
        Some(Value::String(s)) if s == "auto" => {}
        Some(_) => return Err(Skipped::ForcedToolChoice),
    }
    if native
        .iter()
        .any(|t| t.kind != "function" || !t.extra.is_empty())
    {
        return Err(Skipped::NonFunctionTool);
    }
    if req.extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Err(Skipped::ParallelCallsDisabled);
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.as_ref().is_some_and(|c| !c.is_empty()))
    {
        return Err(Skipped::ToolHistory);
    }

    let tools: Vec<tc::ToolDef> = native
        .iter()
        .map(|t| tc::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect();
    let compact = match tc::encode_tools(&tools) {
        Ok(c) => c,
        Err(tc::EncodeError::NotSmaller) => return Err(Skipped::NotSmaller),
        Err(_) => return Err(Skipped::UnsupportedSchema),
    };
    let native_bytes = serde_json::to_string(native).map(|s| s.len()).unwrap_or(0);
    let text = compact.system_text();
    let compact_bytes = text.len();

    // After the leading run of system messages: stable prefix for provider prompt caching, and
    // anything appended later (brevity's directive) stays last.
    let at = req
        .messages
        .iter()
        .position(|m| !matches!(m.role.as_str(), "system" | "developer"))
        .unwrap_or(req.messages.len());
    req.messages.insert(
        at,
        Message {
            role: "system".into(),
            content: Some(Value::String(text)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    req.tools = None;
    req.tool_choice = None;
    Ok(Applied {
        tools,
        native_bytes,
        compact_bytes,
    })
}

/// `token_usage.metadata`-style summary of the decision (no text, no argument values).
pub(crate) fn to_metadata(outcome: &Outcome) -> Value {
    match outcome {
        Ok(a) => json!({
            "compacted": true,
            "native_bytes": a.native_bytes,
            "compact_bytes": a.compact_bytes,
        }),
        Err(s) => json!({ "compacted": false, "skip_reason": s.as_str() }),
    }
}

/// `call_{name}_{index}` — the router's existing synthesized-id convention (Gemini inbound).
fn call_id(name: &str, index: usize) -> String {
    format!("call_{name}_{index}")
}

/// Turn the model's `<<call …>>` text into native `tool_calls` on every choice. Returns the
/// number of calls decoded. Any invalid call fails the whole response.
pub(crate) fn decode_response(
    resp: &mut ChatResponse,
    applied: &Applied,
) -> Result<usize, tc::DecodeError> {
    let mut total = 0;
    for choice in &mut resp.choices {
        let text = choice.message.text().unwrap_or_default();
        let decoded = tc::decode(&text, &applied.tools)?;
        if decoded.calls.is_empty() {
            continue;
        }
        total += decoded.calls.len();
        choice.message.content =
            (!decoded.text.trim().is_empty()).then_some(Value::String(decoded.text));
        choice.message.tool_calls = Some(
            decoded
                .calls
                .into_iter()
                .enumerate()
                .map(|(i, c)| ToolCall {
                    id: call_id(&c.name, i),
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

/// Decode a streamed response on the fly: text deltas pass through (minus anything inside a
/// call marker), each completed call becomes one complete `ToolCallDelta`. A decode error ends
/// the stream with a provider error (text already sent cannot be retracted).
pub(crate) fn wrap_stream(
    stream: BoxStream<'static, Result<ChatChunk, ProviderError>>,
    tools: Vec<tc::ToolDef>,
) -> BoxStream<'static, Result<ChatChunk, ProviderError>> {
    let decoder = match tc::StreamDecoder::new(&tools) {
        Ok(d) => d,
        // `apply` already encoded these tools, so this cannot fail; never pass raw text through.
        Err(e) => {
            let err = ProviderError::Parse(format!("compact tools: {e}"));
            return futures::stream::once(async move { Err(err) }).boxed();
        }
    };
    let out = async_stream::stream! {
        let mut decoder = Some(decoder);
        let mut calls_emitted = 0usize;
        let mut template: Option<ChatChunk> = None;
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            let mut chunk = match item {
                Ok(c) => c,
                Err(e) => { yield Err(e); return; }
            };
            template.get_or_insert_with(|| chunk.clone());
            let Some(choice) = chunk.choices.first_mut() else { yield Ok(chunk); continue };
            let mut events = Vec::new();
            if let (Some(d), Some(text)) = (decoder.as_mut(), choice.delta.content.take()) {
                match d.feed(&text) {
                    Ok(ev) => events.extend(ev),
                    Err(e) => { yield Err(decode_error(&e)); return; }
                }
            }
            if choice.finish_reason.is_some()
                && let Some(d) = decoder.take()
            {
                match d.finish() {
                    Ok(ev) => events.extend(ev),
                    Err(e) => { yield Err(decode_error(&e)); return; }
                }
            }
            apply_events(&mut choice.delta, events, &mut calls_emitted);
            if calls_emitted > 0 && choice.finish_reason.as_deref() == Some("stop") {
                choice.finish_reason = Some("tool_calls".into());
            }
            yield Ok(chunk);
        }
        // Provider ended without a finish_reason: flush whatever the decoder still holds.
        if let Some(d) = decoder.take() {
            match d.finish() {
                Ok(events) if !events.is_empty() => {
                    if let Some(mut chunk) = template {
                        chunk.usage = None;
                        chunk.choices = vec![ChunkChoice { index: 0, delta: Delta::default(), finish_reason: None }];
                        if let Some(choice) = chunk.choices.first_mut() {
                            apply_events(&mut choice.delta, events, &mut calls_emitted);
                        }
                        yield Ok(chunk);
                    }
                }
                Ok(_) => {}
                Err(e) => yield Err(decode_error(&e)),
            }
        }
    };
    out.boxed()
}

fn decode_error(e: &tc::DecodeError) -> ProviderError {
    ProviderError::Parse(format!("compact tool call rejected ({}): {e}", e.code()))
}

fn apply_events(delta: &mut Delta, events: Vec<Event>, calls_emitted: &mut usize) {
    let mut text = String::new();
    let mut calls = Vec::new();
    for e in events {
        match e {
            Event::Text(t) => text.push_str(&t),
            Event::Call(c) => {
                let index = *calls_emitted;
                *calls_emitted += 1;
                calls.push(ToolCallDelta {
                    index: index as i64,
                    id: Some(call_id(&c.name, index)),
                    kind: Some("function".into()),
                    function: Some(FunctionCallDelta {
                        name: Some(c.name),
                        arguments: Some(c.arguments),
                    }),
                });
            }
        }
    }
    delta.content = (!text.is_empty()).then_some(text);
    if !calls.is_empty() {
        delta.tool_calls = Some(calls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Choice, FunctionDef, ToolDef};

    const CAL: &str = r#"{"title":"Retro","start":"2026-10-04T10:00:00+05:30"}"#;

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: enabled,
            ..Default::default()
        }
    }

    fn resolved() -> ResolvedConfig {
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
            compress_enabled: true,
        }
    }

    fn calendar_tool() -> ToolDef {
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
            extra: Default::default(),
        }
    }

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: Some(Value::String(text.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    fn request() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![
                msg("system", "You are helpful."),
                msg("user", "Book a retro tomorrow 10am"),
            ],
            tools: Some(vec![calendar_tool()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    fn response(content: &str) -> ChatResponse {
        ChatResponse {
            id: "r1".into(),
            object: "chat.completion".into(),
            created: None,
            model: "gpt-4o-mini".into(),
            choices: vec![Choice {
                index: 0,
                message: msg("assistant", content),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_is_byte_identical() {
        let mut req = request();
        let before = serde_json::to_string(&req).unwrap();
        assert_eq!(
            apply(&mut req, &cfg(false), &resolved()).unwrap_err(),
            Skipped::Disabled
        );
        assert_eq!(serde_json::to_string(&req).unwrap(), before);
    }

    #[test]
    fn every_skip_reason_leaves_request_untouched() {
        type Setup = Box<dyn Fn(&mut ChatRequest, &mut ResolvedConfig)>;
        let cases: Vec<(Skipped, Setup)> = vec![
            (
                Skipped::AgentOptedOut,
                Box::new(|_, r| r.compress_enabled = false),
            ),
            (
                Skipped::CodingAgent,
                Box::new(|_, r| r.is_coding_agent = true),
            ),
            (Skipped::NoTools, Box::new(|q, _| q.tools = None)),
            (Skipped::NoTools, Box::new(|q, _| q.tools = Some(vec![]))),
            (
                Skipped::ForcedToolChoice,
                Box::new(|q, _| q.tool_choice = Some(json!("required"))),
            ),
            (
                Skipped::ForcedToolChoice,
                Box::new(|q, _| q.tool_choice = Some(json!("none"))),
            ),
            (
                Skipped::ForcedToolChoice,
                Box::new(|q, _| {
                    q.tool_choice = Some(
                        json!({"type": "function", "function": {"name": "create_calendar_event"}}),
                    )
                }),
            ),
            (
                Skipped::NonFunctionTool,
                Box::new(|q, _| {
                    if let Some(t) = q.tools.as_mut().and_then(|t| t.first_mut()) {
                        t.kind = "web_search".into();
                    }
                }),
            ),
            (
                Skipped::NonFunctionTool,
                Box::new(|q, _| {
                    if let Some(t) = q.tools.as_mut().and_then(|t| t.first_mut()) {
                        t.extra.insert("strict".into(), json!(true));
                    }
                }),
            ),
            (
                Skipped::ParallelCallsDisabled,
                Box::new(|q, _| {
                    q.extra.insert("parallel_tool_calls".into(), json!(false));
                }),
            ),
            (
                Skipped::ToolHistory,
                Box::new(|q, _| q.messages.push(msg("tool", "{}"))),
            ),
            (
                Skipped::UnsupportedSchema,
                Box::new(|q, _| {
                    if let Some(t) = q.tools.as_mut().and_then(|t| t.first_mut()) {
                        t.function.parameters =
                            Some(json!({"type": "object", "properties": {"a": {"anyOf": []}}}));
                    }
                }),
            ),
            (
                Skipped::NotSmaller,
                Box::new(|q, _| {
                    if let Some(t) = q.tools.as_mut().and_then(|t| t.first_mut()) {
                        t.function.description = None;
                        t.function.parameters = None;
                    }
                }),
            ),
        ];
        for (want, setup) in cases {
            let mut req = request();
            let mut r = resolved();
            setup(&mut req, &mut r);
            let before = serde_json::to_string(&req).unwrap();
            assert_eq!(apply(&mut req, &cfg(true), &r).unwrap_err(), want);
            assert_eq!(
                serde_json::to_string(&req).unwrap(),
                before,
                "{want:?} mutated the request"
            );
        }
    }

    #[test]
    fn auto_tool_choice_is_compacted() {
        let mut req = request();
        req.tool_choice = Some(json!("auto"));
        assert!(apply(&mut req, &cfg(true), &resolved()).is_ok());
        assert!(req.tool_choice.is_none());
    }

    #[test]
    fn applied_request_shape() {
        let mut req = request();
        // Brevity runs first and appends its directive at the end; it must stay last.
        req.messages.push(msg("system", "BREVITY"));
        let applied = apply(&mut req, &cfg(true), &resolved()).unwrap();
        assert!(req.tools.is_none() && req.tool_choice.is_none());
        let roles: Vec<&str> = req.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["system", "system", "user", "system"]);
        let block = req.messages[1].text().unwrap();
        assert!(block.starts_with(tc::INSTRUCTIONS));
        assert!(
            block.contains("## create_calendar_event: Create an event in the user's calendar.")
        );
        assert_eq!(req.messages[3].text().unwrap(), "BREVITY");
        assert!(applied.compact_bytes < applied.native_bytes);
        assert_eq!(to_metadata(&Ok(applied))["compacted"], json!(true));
        assert_eq!(
            to_metadata(&Err(Skipped::ToolHistory))["skip_reason"],
            json!("tool_history")
        );
    }

    #[test]
    fn applying_is_deterministic() {
        let mut a = request();
        let mut b = request();
        apply(&mut a, &cfg(true), &resolved()).unwrap();
        apply(&mut b, &cfg(true), &resolved()).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    #[test]
    fn brevity_still_detects_tool_loops() {
        // Brevity runs before compaction, so it still sees `tools`; and a tool-loop request is
        // never compacted, so `tools` survives for any later reader too.
        let mut req = request();
        req.messages.push(msg("tool", "{\"ok\":true}"));
        assert!(crate::routing::is_tool_continuation(&req.messages));
        assert_eq!(
            apply(&mut req, &cfg(true), &resolved()).unwrap_err(),
            Skipped::ToolHistory
        );
        assert!(req.tools.is_some());
    }

    fn applied() -> Applied {
        let mut req = request();
        apply(&mut req, &cfg(true), &resolved()).unwrap()
    }

    #[test]
    fn non_streaming_calls_become_native_tool_calls() {
        let mut resp = response(&format!("<<call create_calendar_event {CAL}>>"));
        assert_eq!(decode_response(&mut resp, &applied()).unwrap(), 1);
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert!(choice.message.content.is_none());
        let call = &choice.message.tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.id, "call_create_calendar_event_0");
        assert_eq!(call.function.arguments, CAL);
    }

    #[test]
    fn plain_answers_pass_through_unchanged() {
        let mut resp = response("It's sunny, riya@example.com.");
        let before = serde_json::to_string(&resp).unwrap();
        assert_eq!(decode_response(&mut resp, &applied()).unwrap(), 0);
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn invalid_calls_are_errors_not_guesses() {
        let mut resp =
            response(r#"<<call create_calendar_event {"title":"x","visibility":"secret"}>>"#);
        assert_eq!(
            decode_response(&mut resp, &applied()).unwrap_err().code(),
            "invalid_arguments"
        );
        let mut resp = response("<<call delete_everything {}>>");
        assert_eq!(
            decode_response(&mut resp, &applied()).unwrap_err().code(),
            "unknown_tool"
        );
    }

    #[test]
    fn rendered_by_every_inbound_surface() {
        use crate::inbound::{InboundFormat, inbound_for};
        for format in [
            InboundFormat::OpenAi,
            InboundFormat::Anthropic,
            InboundFormat::Gemini,
        ] {
            let mut resp = response(&format!("Booking.\n<<call create_calendar_event {CAL}>>"));
            decode_response(&mut resp, &applied()).unwrap();
            let rendered = inbound_for(format).render_chat_response(resp).to_string();
            assert!(
                rendered.contains("create_calendar_event"),
                "{format:?}: {rendered}"
            );
            assert!(rendered.contains("Retro"), "{format:?}: {rendered}");
            assert!(
                !rendered.contains("<<call"),
                "{format:?} leaked the marker: {rendered}"
            );
        }
    }

    fn chunk(content: Option<&str>, finish: Option<&str>) -> ChatChunk {
        ChatChunk {
            id: "c1".into(),
            object: "chat.completion.chunk".into(),
            created: None,
            model: "gpt-4o-mini".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: content.map(str::to_string),
                    tool_calls: None,
                },
                finish_reason: finish.map(str::to_string),
            }],
            usage: None,
            extra: Default::default(),
        }
    }

    async fn collect(parts: Vec<ChatChunk>) -> Vec<Result<ChatChunk, String>> {
        let input = futures::stream::iter(parts.into_iter().map(Ok)).boxed();
        wrap_stream(input, applied().tools)
            .map(|r| r.map_err(|e| e.to_string()))
            .collect()
            .await
    }

    #[tokio::test]
    async fn streaming_matches_non_streaming() {
        let full = format!("Booking.\n<<call create_calendar_event {CAL}>>");
        let mut parts: Vec<ChatChunk> = Vec::new();
        let chars: Vec<char> = full.chars().collect();
        for piece in chars.chunks(3) {
            parts.push(chunk(Some(&piece.iter().collect::<String>()), None));
        }
        parts.push(chunk(None, Some("stop")));
        let out = collect(parts).await;
        let mut text = String::new();
        let mut calls = Vec::new();
        let mut finish = None;
        for c in out {
            let c = c.unwrap();
            let choice = &c.choices[0];
            text.push_str(choice.delta.content.as_deref().unwrap_or_default());
            calls.extend(choice.delta.tool_calls.clone().unwrap_or_default());
            if choice.finish_reason.is_some() {
                finish = choice.finish_reason.clone();
            }
        }
        assert_eq!(text, "Booking.\n");
        assert_eq!(calls.len(), 1);
        let f = calls[0].function.as_ref().unwrap();
        assert_eq!(f.name.as_deref(), Some("create_calendar_event"));
        assert_eq!(f.arguments.as_deref(), Some(CAL));
        assert_eq!(calls[0].id.as_deref(), Some("call_create_calendar_event_0"));
        assert_eq!(finish.as_deref(), Some("tool_calls"));

        let mut resp = response(&full);
        decode_response(&mut resp, &applied()).unwrap();
        let native = &resp.choices[0].message.tool_calls.as_ref().unwrap()[0];
        assert_eq!(
            Some(native.function.arguments.as_str()),
            f.arguments.as_deref()
        );
    }

    #[tokio::test]
    async fn streaming_error_ends_stream_without_a_call() {
        let out = collect(vec![
            chunk(Some("<<call create_calendar_event {\"title\":1}"), None),
            chunk(Some(">>"), None),
            chunk(Some("never seen"), Some("stop")),
        ])
        .await;
        assert!(out.last().unwrap().is_err());
        assert!(
            out.iter()
                .filter_map(|c| c.as_ref().ok())
                .all(|c| c.choices[0].delta.tool_calls.is_none())
        );
        assert!(
            !out.iter()
                .filter_map(|c| c.as_ref().ok())
                .any(|c| c.choices[0].delta.content.as_deref() == Some("never seen"))
        );
    }

    #[tokio::test]
    async fn streaming_incomplete_call_at_end_is_an_error() {
        let out = collect(vec![chunk(Some("<<call create_calendar_event {"), None)]).await;
        assert!(out.last().unwrap().is_err());
    }
}
