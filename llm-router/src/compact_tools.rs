//! Compact tool definitions (opt-in): replace native `tools` with a compact system-message
//! catalog on the way out, and rebuild native `tool_calls` from the model's `<<call …>>` reply
//! on the way back.
//!
//! This module is the only router code that knows about `nasiko-tool-compact`. It exposes three
//! pure steps so the handler and the evaluation example run the very same transformation:
//!
//! 1. [`plan`] decides, from the parsed request plus [`RawFacts`] read off the raw body, whether
//!    the request can be compacted at all. Every refusal is a [`Bypass`] label; a bypassed
//!    request takes today's native path untouched.
//! 2. [`apply`] rewrites the outbound request: `tools`, `tool_choice` and `parallel_tool_calls`
//!    go away and one system message carrying the catalog and the call grammar is inserted after
//!    the leading system messages, so the agent's own prompt keeps primacy and the stable
//!    catalog sits in the cacheable prefix.
//! 3. [`finalize`] reads the upstream reply and decides what, if anything, is released:
//!    textual `<<call …>>` calls need `finish_reason == "stop"`, native `tool_calls` need
//!    `finish_reason == "tool_calls"` and are validated against the original schemas too, mixing
//!    the two is rejected, and any other completion releases nothing. [`restore`] applies that
//!    decision to the response.
//!
//! Nothing here retries, repairs or re-samples. A reply the decoder cannot use is reported to
//! the caller as [`crate::GatewayError::CompactToolDecode`] after usage has been recorded.

use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::config::GatewayConfig;
use crate::inbound::InboundFormat;
use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};

/// Why the layer declined a request. Labels are a queryable wire contract (usage metadata and
/// evaluation output), spelled out by hand for the same reason brevity's are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bypass {
    FeatureDisabled,
    /// The operator enabled the feature, but this agent has not opted in
    /// (`agents.compact_tools_enabled`).
    AgentOptedOut,
    NonOpenAiInbound,
    Streaming,
    NoTools,
    UnsupportedToolKind,
    ToolChoiceNone,
    ToolChoiceForced,
    ParallelToolCallsDisabled,
    ResponseFormat,
    MultipleChoices,
    StrictOrUnknownToolKeys,
    HistoryHasToolCallsOrResults,
    UnsupportedSchema,
    InvalidCatalog,
    LimitExceeded,
    /// The compact message is not enough smaller (in bytes) than the `tools` JSON it replaces
    /// to be worth changing the protocol. See [`MIN_SAVING_RATIO`].
    NoByteSaving,
}

impl Bypass {
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::FeatureDisabled => "feature_disabled",
            Self::AgentOptedOut => "agent_opted_out",
            Self::NonOpenAiInbound => "non_openai_inbound",
            Self::Streaming => "streaming",
            Self::NoTools => "no_tools",
            Self::UnsupportedToolKind => "unsupported_tool_kind",
            Self::ToolChoiceNone => "tool_choice_none",
            Self::ToolChoiceForced => "tool_choice_forced",
            Self::ParallelToolCallsDisabled => "parallel_tool_calls_disabled",
            Self::ResponseFormat => "response_format",
            Self::MultipleChoices => "multiple_choices",
            Self::StrictOrUnknownToolKeys => "strict_or_unknown_tool_keys",
            Self::HistoryHasToolCallsOrResults => "history_has_tool_calls_or_results",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::InvalidCatalog => "invalid_catalog",
            Self::LimitExceeded => "limit_exceeded",
            Self::NoByteSaving => "no_byte_saving",
        }
    }
}

/// A request is compacted only when the compact message is at most this fraction of the native
/// `tools` JSON, in bytes: `compact_bytes * DEN <= native_bytes * NUM`.
///
/// Bytes are the only size available without a tokenizer in the request path. Measured with
/// `o200k_base` over the shipped fixtures (`examples/compact_tools_measure.rs`), token break-even
/// sits near a byte ratio of 0.78–0.80: the compact text lives inside a JSON string where every
/// `"` costs an escape, and the fixed framing weighs most on one- or two-tool catalogs. Three
/// quarters keeps every measured case at or above zero saving.
pub const MIN_SAVING_RATIO: (usize, usize) = (3, 4);

/// What only the un-parsed body can tell us. `FunctionDef` has no catch-all map, so a key such
/// as `strict` inside `tools[i].function` is gone once the body has been parsed into the IR; the
/// handler reads these facts before parsing and hands them to [`plan`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawFacts {
    /// Some `tools[i].function` carries a key other than `name`, `description`, `parameters`.
    pub function_has_extra_keys: bool,
}

/// Inspect the raw request body. Cheap: it walks `tools` only.
pub fn inspect_raw(body: &Value) -> RawFacts {
    let extra = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|t| {
                t.get("function")
                    .and_then(Value::as_object)
                    .is_some_and(|f| {
                        f.keys()
                            .any(|k| !matches!(k.as_str(), "name" | "description" | "parameters"))
                    })
            })
        });
    RawFacts {
        function_has_extra_keys: extra,
    }
}

/// Everything [`apply`] and [`finalize`] need, computed once by [`plan`].
#[derive(Debug, Clone)]
pub struct Compiled {
    pub tool_count: usize,
    /// Serialized size of the native `tools` array that is being replaced.
    pub definitions_bytes_in: usize,
    /// Size of the system message that replaces it.
    pub definitions_bytes_out: usize,
    pub system_message: String,
    /// The original tool definitions, kept for validating the reply.
    pub tools: Vec<nasiko_tool_compact::ToolDef>,
}

#[derive(Debug, Clone)]
pub enum Plan {
    Bypass(Bypass),
    Apply(Compiled),
}

impl Plan {
    pub fn bypass(&self) -> Option<Bypass> {
        match self {
            Self::Bypass(b) => Some(*b),
            Self::Apply(_) => None,
        }
    }

    pub fn compiled(&self) -> Option<&Compiled> {
        match self {
            Self::Apply(c) => Some(c),
            Self::Bypass(_) => None,
        }
    }
}

/// Decide whether this request is compacted. Checks run in a fixed order; the first refusal is
/// the recorded label. `consent` is the agent's own opt-in
/// (`ResolvedConfig::compact_tools_enabled`); the fleet flag in `cfg` is the operator's gate and
/// is checked first, so an unconfigured deployment records nothing at all.
pub fn plan(
    req: &ChatRequest,
    raw: &RawFacts,
    format: InboundFormat,
    cfg: &GatewayConfig,
    consent: bool,
) -> Plan {
    if !cfg.compact_tools_enabled {
        return Plan::Bypass(Bypass::FeatureDisabled);
    }
    if !consent {
        return Plan::Bypass(Bypass::AgentOptedOut);
    }
    if format != InboundFormat::OpenAi {
        return Plan::Bypass(Bypass::NonOpenAiInbound);
    }
    if req.is_streaming() {
        return Plan::Bypass(Bypass::Streaming);
    }
    let Some(tools) = req.tools.as_ref().filter(|t| !t.is_empty()) else {
        return Plan::Bypass(Bypass::NoTools);
    };
    if tools.iter().any(|t| t.kind != "function") {
        return Plan::Bypass(Bypass::UnsupportedToolKind);
    }
    match req.tool_choice.as_ref() {
        None => {}
        Some(Value::String(s)) if s == "auto" => {}
        Some(Value::String(s)) if s == "none" => return Plan::Bypass(Bypass::ToolChoiceNone),
        Some(_) => return Plan::Bypass(Bypass::ToolChoiceForced),
    }
    if req.extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Plan::Bypass(Bypass::ParallelToolCallsDisabled);
    }
    if let Some(rf) = req.extra.get("response_format")
        && rf.get("type").and_then(Value::as_str) != Some("text")
    {
        return Plan::Bypass(Bypass::ResponseFormat);
    }
    if req.extra.get("n").and_then(Value::as_i64).unwrap_or(1) > 1 {
        return Plan::Bypass(Bypass::MultipleChoices);
    }
    if raw.function_has_extra_keys || tools.iter().any(|t| !t.extra.is_empty()) {
        return Plan::Bypass(Bypass::StrictOrUnknownToolKeys);
    }
    if req
        .messages
        .iter()
        .any(|m| m.role == "tool" || m.tool_calls.as_ref().is_some_and(|c| !c.is_empty()))
    {
        return Plan::Bypass(Bypass::HistoryHasToolCallsOrResults);
    }

    let defs: Vec<nasiko_tool_compact::ToolDef> = tools
        .iter()
        .map(|t| nasiko_tool_compact::ToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect();
    let compact = match nasiko_tool_compact::encode_tools(&defs) {
        Ok(c) => c,
        Err(e) => {
            return Plan::Bypass(match e.kind() {
                "unsupported_schema" => Bypass::UnsupportedSchema,
                "limit_exceeded" => Bypass::LimitExceeded,
                _ => Bypass::InvalidCatalog,
            });
        }
    };
    let system_message = compact.prompt();
    let definitions_bytes_in = serde_json::to_string(tools).map(|s| s.len()).unwrap_or(0);
    let (num, den) = MIN_SAVING_RATIO;
    if system_message.len().saturating_mul(den) > definitions_bytes_in.saturating_mul(num) {
        return Plan::Bypass(Bypass::NoByteSaving);
    }
    Plan::Apply(Compiled {
        tool_count: defs.len(),
        definitions_bytes_in,
        definitions_bytes_out: system_message.len(),
        system_message,
        tools: defs,
    })
}

/// [`plan`] for callers that still own the raw body (the evaluation example, tests). Such
/// callers stand in for an agent that has opted in.
pub fn plan_from_body(
    req: &ChatRequest,
    body: &Value,
    format: InboundFormat,
    cfg: &GatewayConfig,
) -> Plan {
    plan(req, &inspect_raw(body), format, cfg, true)
}

/// Rewrite the outbound request. Only `tools`, `tool_choice`, `parallel_tool_calls` and the
/// one inserted system message change; every other byte of the request is untouched.
pub fn apply(req: &mut ChatRequest, compiled: &Compiled) {
    req.tools = None;
    req.tool_choice = None;
    req.extra.remove("parallel_tool_calls");
    let index = req
        .messages
        .iter()
        .take_while(|m| m.role == "system")
        .count();
    req.messages.insert(
        index,
        Message {
            role: "system".into(),
            content: Some(Value::String(compiled.system_message.clone())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        },
    );
}

/// Which shape the model used to call tools, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Representation {
    /// No executable output: the reply is passed through untouched.
    None,
    /// `<<call …>>` text, decoded and rebuilt into native `tool_calls`.
    Text,
    /// The provider returned native `tool_calls` despite receiving no `tools`; each was
    /// validated against the original schema and kept as is.
    Native,
}

impl Representation {
    pub const fn as_label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Text => Some("text"),
            Self::Native => Some("native"),
        }
    }
}

/// The calls a reply is allowed to release, and the prose around them.
#[derive(Debug, Clone, PartialEq)]
pub struct Finalized {
    /// Prose outside the calls exactly as written; `None` when there is none.
    pub content: Option<String>,
    pub calls: Vec<nasiko_tool_compact::ToolCall>,
    pub representation: Representation,
}

/// Why a reply released nothing. `kind` is one of the crate's error kinds or
/// `unexpected_choice_count` | `mixed_call_representations` | `incomplete_completion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeFailure {
    pub kind: String,
}

impl DecodeFailure {
    fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
        }
    }
}

/// The text of an assistant message: a string, or the concatenated `text` parts of an array.
fn message_text(message: &Message) -> Option<String> {
    match &message.content {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => {
            let text: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect();
            Some(text)
        }
        _ => None,
    }
}

/// Decide what a reply to a compacted request releases. Pure; shared by the handler and the
/// evaluation example so live results obey exactly the production rules.
pub fn finalize(resp: &ChatResponse, compiled: &Compiled) -> Result<Finalized, DecodeFailure> {
    if resp.choices.len() != 1 {
        return Err(DecodeFailure::new("unexpected_choice_count"));
    }
    let choice = &resp.choices[0];
    let message = &choice.message;
    let finish = choice.finish_reason.as_deref();
    let text = message_text(message);
    let has_marker = text.as_deref().is_some_and(|t| t.contains("<<call"));
    let native = message.tool_calls.as_deref().filter(|c| !c.is_empty());

    match (native, has_marker) {
        (Some(_), true) => Err(DecodeFailure::new("mixed_call_representations")),
        (Some(native_calls), false) => {
            if finish != Some("tool_calls") {
                return Err(DecodeFailure::new("incomplete_completion"));
            }
            // The same limits the stream decoder enforces apply to a native batch, checked
            // before any argument is parsed; any failure rejects the whole response.
            let pairs: Vec<(&str, &str)> = native_calls
                .iter()
                .map(|c| (c.function.name.as_str(), c.function.arguments.as_str()))
                .collect();
            let calls = nasiko_tool_compact::validate_calls(&pairs, &compiled.tools)
                .map_err(|e| DecodeFailure::new(e.kind()))?;
            Ok(Finalized {
                content: text,
                calls,
                representation: Representation::Native,
            })
        }
        (None, true) => {
            if finish != Some("stop") {
                return Err(DecodeFailure::new("incomplete_completion"));
            }
            let decoded =
                nasiko_tool_compact::decode_calls(text.as_deref().unwrap_or(""), &compiled.tools)
                    .map_err(|e| DecodeFailure::new(e.kind()))?;
            Ok(Finalized {
                content: (!decoded.content.is_empty()).then_some(decoded.content),
                calls: decoded.calls,
                representation: Representation::Text,
            })
        }
        (None, false) => Ok(Finalized {
            content: text,
            calls: Vec::new(),
            representation: Representation::None,
        }),
    }
}

/// Apply [`finalize`] to the response. Only a textual reply with at least one call changes the
/// message; everything else, including `id`, `model`, `usage` and `created`, is left as is.
pub fn restore(resp: &mut ChatResponse, compiled: &Compiled) -> Result<Finalized, DecodeFailure> {
    let finalized = finalize(resp, compiled)?;
    if finalized.representation == Representation::Text
        && !finalized.calls.is_empty()
        && let Some(choice) = resp.choices.first_mut()
    {
        choice.message.content = finalized.content.clone().map(Value::String);
        choice.message.tool_calls = Some(
            finalized
                .calls
                .iter()
                .map(|c| ToolCall {
                    id: new_call_id(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: c.name.clone(),
                        arguments: nasiko_tool_compact::canonical_json(&c.arguments),
                    },
                    extra: Map::new(),
                })
                .collect(),
        );
        choice.finish_reason = Some("tool_calls".into());
    }
    Ok(finalized)
}

/// `call_` followed by 24 hex characters. Random, like every provider's ids.
pub fn new_call_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("call_{}", hex.get(..24).unwrap_or(&hex))
}

/// The `metadata.compact_tools` block for the usage row. `None` while the feature is off, so
/// rows of an unconfigured deployment stay byte-identical.
pub fn to_metadata(
    plan: &Plan,
    outcome: Option<&Result<Finalized, DecodeFailure>>,
) -> Option<Value> {
    match plan {
        Plan::Bypass(Bypass::FeatureDisabled) => None,
        Plan::Bypass(b) => Some(json!({
            "applied": false,
            "bypass": b.as_label(),
            "tool_count": null,
            "definitions_bytes_in": null,
            "definitions_bytes_out": null,
            "decode": null,
            "representation": null,
            "calls": null,
        })),
        Plan::Apply(c) => {
            let (decode, representation, calls) = match outcome {
                None => (Value::Null, Value::Null, Value::Null),
                Some(Ok(f)) => (
                    json!(if f.representation == Representation::None {
                        "passed_through"
                    } else {
                        "ok"
                    }),
                    json!(f.representation.as_label()),
                    json!(f.calls.len()),
                ),
                Some(Err(e)) => (json!(e.kind), Value::Null, Value::Null),
            };
            Some(json!({
                "applied": true,
                "bypass": null,
                "tool_count": c.tool_count,
                "definitions_bytes_in": c.definitions_bytes_in,
                "definitions_bytes_out": c.definitions_bytes_out,
                "decode": decode,
                "representation": representation,
                "calls": calls,
            }))
        }
    }
}

/// Usage metadata for the native re-send that follows a compacted reply the decoder refused
/// (`GatewayConfig::compact_tools_native_retry`). The request on the wire was the original one,
/// so nothing was applied; `retry_after` names the failure that caused the second, billed call
/// so the two usage rows can be read together.
pub fn native_retry_metadata(failure: &DecodeFailure) -> Value {
    json!({
        "applied": false,
        "bypass": "native_retry",
        "retry_after": failure.kind,
        "tool_count": null,
        "definitions_bytes_in": null,
        "definitions_bytes_out": null,
        "decode": null,
        "representation": null,
        "calls": null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Choice, FunctionDef, ToolDef};

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: enabled,
            ..GatewayConfig::default()
        }
    }

    fn tool(name: &str, parameters: Value) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: Some(format!("{name} description")),
                parameters: Some(parameters),
            },
            extra: Map::new(),
        }
    }

    fn weather() -> ToolDef {
        tool(
            "get_weather",
            json!({"type": "object", "properties": {
                "city": {"type": "string"},
                "unit": {"type": "string", "enum": ["c", "f"]}
            }, "required": ["city"]}),
        )
    }

    fn forecast() -> ToolDef {
        tool(
            "get_forecast",
            json!({"type": "object", "properties": {
                "city": {"type": "string", "description": "City name"},
                "days": {"type": "integer", "minimum": 1, "maximum": 5, "description": "Number of days"}
            }, "required": ["city"]}),
        )
    }

    fn request(tools: Vec<ToolDef>) -> ChatRequest {
        serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are terse."},
                {"role": "user", "content": "Weather in Paris?"}
            ],
            "tools": serde_json::to_value(tools).unwrap(),
        }))
        .unwrap()
    }

    fn plan_for(req: &ChatRequest) -> Plan {
        plan(
            req,
            &RawFacts::default(),
            InboundFormat::OpenAi,
            &cfg(true),
            true,
        )
    }

    fn response(content: Value, finish: Option<&str>, tool_calls: Option<Value>) -> ChatResponse {
        serde_json::from_value(json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1, "model": "gpt-4o",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": content, "tool_calls": tool_calls}, "finish_reason": finish}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }))
        .unwrap()
    }

    fn compiled() -> Compiled {
        match plan_for(&request(vec![weather(), forecast()])) {
            Plan::Apply(c) => c,
            Plan::Bypass(b) => panic!("unexpected bypass {b:?}"),
        }
    }

    #[test]
    fn plan_bypasses_in_the_documented_order() {
        let base = request(vec![weather(), forecast()]);
        assert_eq!(
            plan(
                &base,
                &RawFacts::default(),
                InboundFormat::OpenAi,
                &cfg(false),
                true
            )
            .bypass(),
            Some(Bypass::FeatureDisabled)
        );
        // The operator's gate comes first: with the fleet flag off nothing is recorded, whatever
        // the agent chose. With it on, an agent that has not opted in is a labelled bypass.
        assert_eq!(
            plan(
                &base,
                &RawFacts::default(),
                InboundFormat::OpenAi,
                &cfg(false),
                false
            )
            .bypass(),
            Some(Bypass::FeatureDisabled)
        );
        assert_eq!(
            plan(
                &base,
                &RawFacts::default(),
                InboundFormat::OpenAi,
                &cfg(true),
                false
            )
            .bypass(),
            Some(Bypass::AgentOptedOut)
        );
        assert_eq!(
            plan(
                &base,
                &RawFacts::default(),
                InboundFormat::Anthropic,
                &cfg(true),
                true
            )
            .bypass(),
            Some(Bypass::NonOpenAiInbound)
        );
        let mut r = base.clone();
        r.stream = Some(true);
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::Streaming));
        let mut r = base.clone();
        r.tools = Some(vec![]);
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::NoTools));
        let mut r = base.clone();
        r.tools = None;
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::NoTools));
        let mut r = base.clone();
        r.tools.as_mut().unwrap()[0].kind = "custom".into();
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::UnsupportedToolKind));
        let mut r = base.clone();
        r.tool_choice = Some(json!("none"));
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::ToolChoiceNone));
        for forced in [
            json!("required"),
            json!({"type": "function", "function": {"name": "get_weather"}}),
            json!("weird"),
        ] {
            let mut r = base.clone();
            r.tool_choice = Some(forced);
            assert_eq!(plan_for(&r).bypass(), Some(Bypass::ToolChoiceForced));
        }
        let mut r = base.clone();
        r.tool_choice = Some(json!("auto"));
        assert!(plan_for(&r).bypass().is_none());
        let mut r = base.clone();
        r.extra.insert("parallel_tool_calls".into(), json!(false));
        assert_eq!(
            plan_for(&r).bypass(),
            Some(Bypass::ParallelToolCallsDisabled)
        );
        let mut r = base.clone();
        r.extra.insert("parallel_tool_calls".into(), json!(true));
        assert!(plan_for(&r).bypass().is_none());
        let mut r = base.clone();
        r.extra
            .insert("response_format".into(), json!({"type": "json_object"}));
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::ResponseFormat));
        let mut r = base.clone();
        r.extra
            .insert("response_format".into(), json!({"type": "text"}));
        assert!(plan_for(&r).bypass().is_none());
        let mut r = base.clone();
        r.extra.insert("n".into(), json!(2));
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::MultipleChoices));
        let mut r = base.clone();
        r.extra.insert("n".into(), json!(1));
        assert!(plan_for(&r).bypass().is_none());
        let raw = RawFacts {
            function_has_extra_keys: true,
        };
        assert_eq!(
            plan(&base, &raw, InboundFormat::OpenAi, &cfg(true), true).bypass(),
            Some(Bypass::StrictOrUnknownToolKeys)
        );
        let mut r = base.clone();
        r.tools.as_mut().unwrap()[0]
            .extra
            .insert("cache_control".into(), json!({}));
        assert_eq!(plan_for(&r).bypass(), Some(Bypass::StrictOrUnknownToolKeys));
        let mut r = base.clone();
        r.messages.push(Message {
            role: "tool".into(),
            content: Some(json!("result")),
            name: None,
            tool_calls: None,
            tool_call_id: Some("call_0".into()),
            extra: Map::new(),
        });
        assert_eq!(
            plan_for(&r).bypass(),
            Some(Bypass::HistoryHasToolCallsOrResults)
        );
        let unsupported = request(vec![tool(
            "t",
            json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "x"}}}),
        )]);
        assert_eq!(
            plan_for(&unsupported).bypass(),
            Some(Bypass::UnsupportedSchema)
        );
        let dup = request(vec![weather(), weather()]);
        assert_eq!(plan_for(&dup).bypass(), Some(Bypass::InvalidCatalog));
        // A catalog so small that the framing outweighs the saving stays native.
        let tiny = request(vec![tool("ping", json!({"type": "object"}))]);
        assert_eq!(plan_for(&tiny).bypass(), Some(Bypass::NoByteSaving));
        let single = request(vec![weather()]);
        assert_eq!(plan_for(&single).bypass(), Some(Bypass::NoByteSaving));
    }

    #[test]
    fn inspect_raw_sees_strict_and_other_function_keys() {
        assert!(
            inspect_raw(
                &json!({"tools": [{"type": "function", "function": {"name": "a", "strict": true}}]})
            )
            .function_has_extra_keys
        );
        assert!(
            inspect_raw(
                &json!({"tools": [{"type": "function", "function": {"name": "a", "x": 1}}]})
            )
            .function_has_extra_keys
        );
        assert!(!inspect_raw(&json!({"tools": [{"type": "function", "function": {"name": "a", "description": "d", "parameters": {}}}]})).function_has_extra_keys);
        assert!(!inspect_raw(&json!({"messages": []})).function_has_extra_keys);
    }

    #[test]
    fn apply_inserts_one_system_message_after_the_leading_system_run_and_changes_nothing_else() {
        let mut req = request(vec![weather(), forecast()]);
        req.extra.insert("parallel_tool_calls".into(), json!(true));
        req.extra.insert("top_p".into(), json!(0.5));
        let before = serde_json::to_value(&req).unwrap();
        let compiled = compiled();
        apply(&mut req, &compiled);
        let after = serde_json::to_value(&req).unwrap();
        assert!(after.get("tools").is_none());
        assert!(after.get("tool_choice").is_none());
        assert!(after.get("parallel_tool_calls").is_none());
        assert_eq!(after["top_p"], json!(0.5));
        assert_eq!(after["model"], before["model"]);
        let msgs = after["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0], before["messages"][0]);
        assert_eq!(msgs[1]["role"], "system");
        assert_eq!(msgs[1]["content"], json!(compiled.system_message));
        assert_eq!(msgs[2], before["messages"][1]);
        assert!(
            compiled
                .system_message
                .contains("get_weather(city:str, unit?:c|f) - get_weather description")
        );
        assert!(
            compiled
                .system_message
                .starts_with(nasiko_tool_compact::HEADER)
        );
        assert!(
            compiled
                .system_message
                .ends_with(nasiko_tool_compact::INSTRUCTIONS)
        );

        // No system messages: the catalog goes first. Two leading plus a later one: after the two.
        let mut req: ChatRequest = serde_json::from_value(json!({"messages": [{"role": "user", "content": "x"}], "tools": [serde_json::to_value(weather()).unwrap(), serde_json::to_value(forecast()).unwrap()]})).unwrap();
        apply(&mut req, &compiled);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[1].role, "user");
        let mut req: ChatRequest = serde_json::from_value(json!({"messages": [
            {"role": "system", "content": "a"}, {"role": "system", "content": "b"},
            {"role": "user", "content": "x"}, {"role": "system", "content": "late"}
        ], "tools": [serde_json::to_value(weather()).unwrap(), serde_json::to_value(forecast()).unwrap()]}))
        .unwrap();
        apply(&mut req, &compiled);
        assert_eq!(
            req.messages[2].content,
            Some(json!(compiled.system_message))
        );
        assert_eq!(req.messages[4].content, Some(json!("late")));
    }

    #[test]
    fn textual_calls_need_stop_and_are_rebuilt_natively() {
        let compiled = compiled();
        let mut resp = response(
            json!("Sure.\n<<call get_weather {\"city\":\"Paris\",\"unit\":\"c\"}>>\nDone."),
            Some("stop"),
            None,
        );
        let f = restore(&mut resp, &compiled).unwrap();
        assert_eq!(f.representation, Representation::Text);
        assert_eq!(f.calls.len(), 1);
        let msg = &resp.choices[0].message;
        assert_eq!(msg.content, Some(json!("Sure.\n\nDone.")));
        let calls = msg.tool_calls.as_ref().unwrap();
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(
            calls[0].function.arguments,
            r#"{"city":"Paris","unit":"c"}"#
        );
        assert!(calls[0].id.starts_with("call_") && calls[0].id.len() == 29);
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(resp.model, "gpt-4o");
        assert_eq!(resp.usage.as_ref().unwrap().prompt_tokens, Some(10));

        // Call only: content becomes null.
        let mut resp = response(
            json!("<<call get_weather {\"city\":\"Paris\"}>>"),
            Some("stop"),
            None,
        );
        restore(&mut resp, &compiled).unwrap();
        assert_eq!(resp.choices[0].message.content, None);

        // Two calls get two distinct ids.
        let mut resp = response(
            json!("<<call get_weather {\"city\":\"A\"}>>\n<<call get_weather {\"city\":\"B\"}>>"),
            Some("stop"),
            None,
        );
        restore(&mut resp, &compiled).unwrap();
        let calls = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_ne!(calls[0].id, calls[1].id);

        // Any other finish reason with a marker releases nothing.
        for finish in [
            None,
            Some("length"),
            Some("content_filter"),
            Some("tool_calls"),
        ] {
            let resp = response(
                json!("<<call get_weather {\"city\":\"Paris\"}>>"),
                finish,
                None,
            );
            assert_eq!(
                finalize(&resp, &compiled).unwrap_err().kind,
                "incomplete_completion",
                "{finish:?}"
            );
        }
    }

    #[test]
    fn decode_errors_propagate_and_the_response_is_untouched() {
        let compiled = compiled();
        let cases = [
            ("<<call delete_all {}>>", "unknown_tool"),
            ("<<call get_weather {\"unit\":\"c\"}>>", "invalid_arguments"),
            (
                "<<call get_weather {\"city\":\"P\",\"unit\":\"k\"}>>",
                "invalid_arguments",
            ),
            (
                "<<call get_weather {\"city\":\"P\"}>>\n<<call get_weather {}>>",
                "invalid_arguments",
            ),
            ("<<call get_weather {\"city\":\"P\"}>", "incomplete_call"),
            ("<<call get_weather {\"a\":1,\"a\":2}>>", "malformed_call"),
        ];
        for (text, kind) in cases {
            let mut resp = response(json!(text), Some("stop"), None);
            let before = serde_json::to_value(&resp).unwrap();
            let err = restore(&mut resp, &compiled).unwrap_err();
            assert_eq!(err.kind, kind, "{text}");
            assert_eq!(serde_json::to_value(&resp).unwrap(), before);
        }
    }

    #[test]
    fn plain_text_passes_through_whatever_the_finish_reason() {
        let compiled = compiled();
        for finish in [Some("stop"), Some("length"), Some("content_filter"), None] {
            let mut resp = response(json!("It is sunny."), finish, None);
            let before = serde_json::to_value(&resp).unwrap();
            let f = restore(&mut resp, &compiled).unwrap();
            assert_eq!(f.representation, Representation::None);
            assert!(f.calls.is_empty());
            assert_eq!(serde_json::to_value(&resp).unwrap(), before);
        }
        // Array content with text parts is read; a marker inside it counts.
        let resp = response(
            json!([{"type": "text", "text": "<<call get_weather {\"city\":\"P\"}>>"}]),
            Some("stop"),
            None,
        );
        assert_eq!(finalize(&resp, &compiled).unwrap().calls.len(), 1);
        let resp = response(Value::Null, Some("stop"), None);
        assert_eq!(
            finalize(&resp, &compiled).unwrap().representation,
            Representation::None
        );
    }

    #[test]
    fn native_tool_calls_need_the_tool_calls_finish_reason_and_valid_arguments() {
        let compiled = compiled();
        let native = json!([{"id": "call_up", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]);
        let mut resp = response(Value::Null, Some("tool_calls"), Some(native.clone()));
        let before = serde_json::to_value(&resp).unwrap();
        let f = restore(&mut resp, &compiled).unwrap();
        assert_eq!(f.representation, Representation::Native);
        assert_eq!(f.calls[0].arguments, json!({"city": "Paris"}));
        assert_eq!(
            serde_json::to_value(&resp).unwrap(),
            before,
            "valid native calls are kept as is"
        );

        let resp = response(Value::Null, Some("stop"), Some(native.clone()));
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "incomplete_completion"
        );
        let resp = response(Value::Null, Some("length"), Some(native));
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "incomplete_completion"
        );

        let bad = json!([{"id": "c", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":5}"}}]);
        let resp = response(Value::Null, Some("tool_calls"), Some(bad));
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "invalid_arguments"
        );
        let unknown = json!([{"id": "c", "type": "function", "function": {"name": "nope", "arguments": "{}"}}]);
        let resp = response(Value::Null, Some("tool_calls"), Some(unknown));
        assert_eq!(finalize(&resp, &compiled).unwrap_err().kind, "unknown_tool");
        let dup = json!([{"id": "c", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"a\",\"city\":\"b\"}"}}]);
        let resp = response(Value::Null, Some("tool_calls"), Some(dup));
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "malformed_call"
        );
    }

    #[test]
    fn native_batches_obey_the_decoder_limits_and_release_nothing_on_a_violation() {
        use nasiko_tool_compact::limits;
        let compiled = compiled();
        let call = |args: String| json!({"id": "c", "type": "function", "function": {"name": "get_weather", "arguments": args}});
        let small = || call("{\"city\":\"P\"}".into());

        // Too many calls.
        let many: Vec<Value> = (0..=limits::MAX_CALLS).map(|_| small()).collect();
        let mut resp = response(Value::Null, Some("tool_calls"), Some(Value::Array(many)));
        let before = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            restore(&mut resp, &compiled).unwrap_err().kind,
            "limit_exceeded"
        );
        assert_eq!(serde_json::to_value(&resp).unwrap(), before);
        let exact: Vec<Value> = (0..limits::MAX_CALLS).map(|_| small()).collect();
        let resp = response(Value::Null, Some("tool_calls"), Some(Value::Array(exact)));
        assert_eq!(
            finalize(&resp, &compiled).unwrap().calls.len(),
            limits::MAX_CALLS
        );

        // One oversized call, even after a valid one.
        let oversized = call(format!(
            "{{\"city\":\"{}\"}}",
            "x".repeat(limits::MAX_ARGS_BYTES)
        ));
        let resp = response(
            Value::Null,
            Some("tool_calls"),
            Some(json!([small(), oversized])),
        );
        let err = finalize(&resp, &compiled).unwrap_err();
        assert_eq!(err.kind, "limit_exceeded");

        // Aggregate argument bytes.
        let chunk = call(format!("{{\"city\":\"{}\"}}", "x".repeat(200 * 1024)));
        let resp = response(
            Value::Null,
            Some("tool_calls"),
            Some(json!([chunk.clone(), chunk.clone()])),
        );
        assert_eq!(finalize(&resp, &compiled).unwrap().calls.len(), 2);
        let resp = response(
            Value::Null,
            Some("tool_calls"),
            Some(json!([chunk.clone(), chunk.clone(), chunk])),
        );
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "limit_exceeded"
        );
    }

    #[test]
    fn mixed_representations_and_extra_choices_are_rejected() {
        let compiled = compiled();
        let native = json!([{"id": "c", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"P\"}"}}]);
        let resp = response(
            json!("<<call get_weather {\"city\":\"P\"}>>"),
            Some("tool_calls"),
            Some(native),
        );
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "mixed_call_representations"
        );

        let mut resp = response(json!("hi"), Some("stop"), None);
        resp.choices.push(Choice {
            index: 1,
            message: resp.choices[0].message.clone(),
            finish_reason: Some("stop".into()),
        });
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "unexpected_choice_count"
        );
        resp.choices.clear();
        assert_eq!(
            finalize(&resp, &compiled).unwrap_err().kind,
            "unexpected_choice_count"
        );
    }

    #[test]
    fn metadata_is_absent_when_disabled_and_fully_shaped_otherwise() {
        assert_eq!(
            to_metadata(&Plan::Bypass(Bypass::FeatureDisabled), None),
            None
        );
        let bypass = to_metadata(&Plan::Bypass(Bypass::Streaming), None).unwrap();
        assert_eq!(bypass["applied"], json!(false));
        assert_eq!(bypass["bypass"], json!("streaming"));
        assert_eq!(bypass.as_object().unwrap().len(), 8);

        let compiled = compiled();
        let plan = Plan::Apply(compiled.clone());
        let pending = to_metadata(&plan, None).unwrap();
        assert_eq!(pending["applied"], json!(true));
        assert_eq!(pending["tool_count"], json!(2));
        assert!(
            pending["definitions_bytes_in"].as_u64().unwrap()
                > pending["definitions_bytes_out"].as_u64().unwrap() / 4
        );
        assert_eq!(pending["decode"], Value::Null);

        let ok = finalize(
            &response(
                json!("<<call get_weather {\"city\":\"P\"}>>"),
                Some("stop"),
                None,
            ),
            &compiled,
        );
        let m = to_metadata(&plan, Some(&ok)).unwrap();
        assert_eq!(m["decode"], json!("ok"));
        assert_eq!(m["representation"], json!("text"));
        assert_eq!(m["calls"], json!(1));
        let passed = finalize(&response(json!("hi"), Some("stop"), None), &compiled);
        assert_eq!(
            to_metadata(&plan, Some(&passed)).unwrap()["decode"],
            json!("passed_through")
        );
        let failed = finalize(
            &response(json!("<<call x {}>>"), Some("stop"), None),
            &compiled,
        );
        let m = to_metadata(&plan, Some(&failed)).unwrap();
        assert_eq!(m["decode"], json!("unknown_tool"));
        assert_eq!(m["calls"], Value::Null);
    }
}
