//! Compact tool definitions at the egress seam (opt-in, non-streaming).
//!
//! Replaces a request's native `tools` with the one-line-per-tool form from
//! [`nasiko_tool_compact`], then decodes the model's `<<call name {json}>>` replies back into
//! standard `tool_calls`. The client sends and receives ordinary OpenAI-shaped tool calling and
//! never sees the compact form.
//!
//! Runs on the normalized [`ChatRequest`] IR, so one implementation covers the OpenAI, Anthropic
//! and Gemini inbound surfaces.
//!
//! # Fail-closed
//!
//! A reply that does not decode — an unknown tool, invalid arguments, a malformed marker — is
//! never returned, repaired or guessed at. [`restore`] reports the error and the caller re-sends
//! the original native request instead, so the worst case of turning this on is one extra call.
//!
//! # When it steps aside
//!
//! Every case where compaction could change what a client gets back goes out natively instead,
//! with the reason recorded: streaming, a forced or disabled `tool_choice`,
//! `parallel_tool_calls: false`, earlier tool calls or results in the transcript, a non-function
//! tool, and any schema the compact form cannot carry exactly.

use nasiko_tool_compact::{CompactError, decode, encode_tools};
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::ir::chat::{ChatResponse, FunctionCall, Message, ToolCall};
use crate::resolver::ResolvedConfig;

/// Why compaction stepped aside, for logs and tests.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    /// The per-agent token-optimization switch governs every layer, this one included.
    AgentOptedOut,
    /// Coding CLIs lean on exact native tool semantics and ship very large tool sets.
    CodingAgent,
    /// Only non-streaming responses are decoded today.
    Streaming,
    NoTools,
    /// `tool_choice` other than absent/`"auto"`: a prompt cannot guarantee a forced call.
    ToolChoice,
    /// The compact instruction allows several calls, so "at most one" cannot be honoured.
    ParallelCallsOff,
    /// Earlier calls or results would need re-rendering; they are left native for now.
    ToolHistory,
    /// A tool that is not `type: function`, or carries fields the conversion would drop.
    NonFunctionTool,
    /// A schema feature the compact form cannot carry exactly.
    UnsupportedSchema,
}

impl Skipped {
    /// Stable string for logs; spelled out so renaming a variant cannot change a value.
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::AgentOptedOut => "agent_opted_out",
            Self::CodingAgent => "coding_agent",
            Self::Streaming => "streaming",
            Self::NoTools => "no_tools",
            Self::ToolChoice => "tool_choice",
            Self::ParallelCallsOff => "parallel_calls_off",
            Self::ToolHistory => "tool_history",
            Self::NonFunctionTool => "non_function_tool",
            Self::UnsupportedSchema => "unsupported_schema",
        }
    }
}

/// A request that was compacted: what the decoder checks against, and the original to fall back
/// to if the reply does not decode.
pub(crate) struct Compacted {
    tools: Vec<nasiko_tool_compact::ToolDef>,
    pub(crate) native: ChatRequest,
}

/// Swap native tool definitions for the compact form, unless a carve-out applies.
pub(crate) fn apply(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
    resolved: &ResolvedConfig,
) -> Result<Compacted, Skipped> {
    if !cfg.tool_compact_enabled {
        return Err(Skipped::Disabled);
    }
    if !resolved.compress_enabled {
        return Err(Skipped::AgentOptedOut);
    }
    if resolved.is_coding_agent {
        return Err(Skipped::CodingAgent);
    }
    if req.is_streaming() {
        return Err(Skipped::Streaming);
    }
    let native_tools = match &req.tools {
        Some(tools) if !tools.is_empty() => tools,
        _ => return Err(Skipped::NoTools),
    };
    match &req.tool_choice {
        None => {}
        Some(Value::String(choice)) if choice == "auto" => {}
        Some(_) => return Err(Skipped::ToolChoice),
    }
    if req.extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Err(Skipped::ParallelCallsOff);
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.is_some())
    {
        return Err(Skipped::ToolHistory);
    }
    let tools = native_tools
        .iter()
        .map(|t| {
            (t.kind == "function" && t.extra.is_empty()).then(|| nasiko_tool_compact::ToolDef {
                name: t.function.name.clone(),
                description: t.function.description.clone(),
                parameters: t.function.parameters.clone(),
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(Skipped::NonFunctionTool)?;
    let compact = encode_tools(&tools).map_err(|_| Skipped::UnsupportedSchema)?;

    let native = req.clone();
    req.tools = None;
    req.tool_choice = None;
    // Only valid alongside `tools`; providers reject it once they are gone.
    req.extra.remove("parallel_tool_calls");
    // Leading, so the tool block stays in the stable prefix the way native tools do.
    req.messages.insert(
        0,
        Message {
            role: "system".into(),
            content: Some(Value::String(compact.prompt())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        },
    );
    Ok(Compacted { tools, native })
}

/// Turn compact calls in the reply into `tool_calls`. Returns how many calls were found.
///
/// On error the response is left untouched and must not be returned to the client.
pub(crate) fn restore(
    resp: &mut ChatResponse,
    compacted: &Compacted,
) -> Result<usize, CompactError> {
    // Decode every choice before changing any, so an error leaves nothing half-rewritten.
    let decoded = resp
        .choices
        .iter()
        .map(|c| decode(&c.message.text().unwrap_or_default(), &compacted.tools))
        .collect::<Result<Vec<_>, _>>()?;
    let mut found = 0;
    for (choice, decoded) in resp.choices.iter_mut().zip(decoded) {
        if decoded.calls.is_empty() {
            continue;
        }
        found += decoded.calls.len();
        let text = decoded.text.trim();
        choice.message.content = (!text.is_empty()).then(|| Value::String(text.to_string()));
        choice.message.tool_calls = Some(
            decoded
                .calls
                .into_iter()
                .map(|call| ToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: call.name,
                        arguments: call.arguments,
                    },
                    extra: Default::default(),
                })
                .collect(),
        );
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Choice, FunctionDef, ToolDef};
    use serde_json::json;

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            tool_compact_enabled: enabled,
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

    fn calendar() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event in the user's calendar.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
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
        let mut extra = serde_json::Map::new();
        extra.insert("parallel_tool_calls".into(), json!(true));
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![
                msg("system", "You are a scheduler."),
                msg("user", "Book a design review Monday 3pm"),
            ],
            tools: Some(vec![calendar()]),
            tool_choice: Some(json!("auto")),
            temperature: None,
            max_tokens: None,
            stream: None,
            extra,
        }
    }

    fn response(text: &str) -> ChatResponse {
        ChatResponse {
            id: "chatcmpl-1".into(),
            object: "chat.completion".into(),
            created: None,
            model: "gpt-4o-mini".into(),
            choices: vec![Choice {
                index: 0,
                message: msg("assistant", text),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_leaves_the_request_byte_identical() {
        let mut r = request();
        let before = serde_json::to_string(&r).unwrap();
        assert!(matches!(
            apply(&mut r, &cfg(false), &resolved()),
            Err(Skipped::Disabled)
        ));
        assert_eq!(serde_json::to_string(&r).unwrap(), before);
    }

    #[test]
    fn every_carve_out_leaves_the_request_byte_identical() {
        let opted_out = ResolvedConfig {
            compress_enabled: false,
            ..resolved()
        };
        let coding = ResolvedConfig {
            is_coding_agent: true,
            ..resolved()
        };
        let mut streaming = request();
        streaming.stream = Some(true);
        let mut no_tools = request();
        no_tools.tools = None;
        let mut forced = request();
        forced.tool_choice = Some(json!({"type": "function", "function": {"name": "x"}}));
        let mut required = request();
        required.tool_choice = Some(json!("required"));
        let mut serial = request();
        serial
            .extra
            .insert("parallel_tool_calls".into(), json!(false));
        let mut history = request();
        history.messages.push(Message {
            tool_call_id: Some("call_1".into()),
            ..msg("tool", "{}")
        });
        let mut odd_tool = request();
        odd_tool.tools.as_mut().unwrap()[0]
            .extra
            .insert("cache_control".into(), json!({"type": "ephemeral"}));
        let mut unsupported = request();
        unsupported.tools.as_mut().unwrap()[0].function.parameters = Some(json!({
            "type": "object",
            "properties": {"code": {"type": "string", "pattern": "^[A-Z]+$"}}
        }));

        let cases = [
            (request(), opted_out, Skipped::AgentOptedOut),
            (request(), coding, Skipped::CodingAgent),
            (streaming, resolved(), Skipped::Streaming),
            (no_tools, resolved(), Skipped::NoTools),
            (forced, resolved(), Skipped::ToolChoice),
            (required, resolved(), Skipped::ToolChoice),
            (serial, resolved(), Skipped::ParallelCallsOff),
            (history, resolved(), Skipped::ToolHistory),
            (odd_tool, resolved(), Skipped::NonFunctionTool),
            (unsupported, resolved(), Skipped::UnsupportedSchema),
        ];
        for (mut r, res, want) in cases {
            let before = serde_json::to_string(&r).unwrap();
            match apply(&mut r, &cfg(true), &res) {
                Err(got) => assert_eq!(got, want),
                Ok(_) => panic!("expected {want:?}, got compaction"),
            }
            assert_eq!(
                serde_json::to_string(&r).unwrap(),
                before,
                "{want:?} mutated the request"
            );
        }
    }

    #[test]
    fn compaction_swaps_tools_for_a_leading_system_message() {
        let mut r = request();
        let original = r.clone();
        let compacted = apply(&mut r, &cfg(true), &resolved()).unwrap();

        assert!(r.tools.is_none());
        assert!(r.tool_choice.is_none());
        assert!(!r.extra.contains_key("parallel_tool_calls"));
        assert_eq!(r.messages.len(), original.messages.len() + 1);
        assert_eq!(r.messages[0].role, "system");
        let prompt = r.messages[0].text().unwrap();
        assert!(prompt.contains("create_calendar_event(title:str, start:datetime"));
        assert!(prompt.contains("<<call name {json args}>>"));
        // The author's own messages follow untouched.
        for (i, before) in original.messages.iter().enumerate() {
            assert_eq!(
                serde_json::to_value(&r.messages[i + 1]).unwrap(),
                serde_json::to_value(before).unwrap()
            );
        }
        // The fallback keeps the request exactly as the client sent it.
        assert_eq!(
            serde_json::to_string(&compacted.native).unwrap(),
            serde_json::to_string(&original).unwrap()
        );
    }

    #[test]
    fn compact_calls_come_back_as_openai_tool_calls() {
        let mut r = request();
        let compacted = apply(&mut r, &cfg(true), &resolved()).unwrap();
        let args = r#"{"title":"Design review","start":"2026-10-05T15:00:00+05:30"}"#;
        let mut resp = response(&format!(
            "Booking it. <<call create_calendar_event {args}>>"
        ));

        assert_eq!(restore(&mut resp, &compacted).unwrap(), 1);

        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(choice.message.text().as_deref(), Some("Booking it."));
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].kind, "function");
        assert!(calls[0].id.starts_with("call_"));
        assert_eq!(calls[0].function.name, "create_calendar_event");
        assert_eq!(calls[0].function.arguments, args);
        let wire = serde_json::to_string(&resp).unwrap();
        assert!(!wire.contains("<<call"), "compact form leaked: {wire}");
    }

    #[test]
    fn a_plain_answer_passes_through_unchanged() {
        let mut r = request();
        let compacted = apply(&mut r, &cfg(true), &resolved()).unwrap();
        let mut resp = response("Which day works for you?");
        let before = serde_json::to_string(&resp).unwrap();

        assert_eq!(restore(&mut resp, &compacted).unwrap(), 0);
        assert_eq!(serde_json::to_string(&resp).unwrap(), before);
    }

    #[test]
    fn a_bad_call_is_an_error_and_leaves_the_response_untouched() {
        let mut r = request();
        let compacted = apply(&mut r, &cfg(true), &resolved()).unwrap();
        for bad in [
            "<<call delete_everything {}>>",
            r#"<<call create_calendar_event {"title":"x"}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"s","visibility":"secret"}>>"#,
            r#"<<call create_calendar_event {"title":"x","start":"s"}}>"#,
        ] {
            let mut resp = response(bad);
            let before = serde_json::to_string(&resp).unwrap();
            assert!(restore(&mut resp, &compacted).is_err(), "accepted {bad}");
            assert_eq!(serde_json::to_string(&resp).unwrap(), before);
        }
    }
}
