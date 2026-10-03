//! Opt-in compaction at the canonical chat seam; shared with `compact_tools_eval`.
//! No configuration or provider access lives here. Bypasses leave the request unchanged.

use nasiko_tool_compact::{CompactError, StreamDecoder, ToolDef, encode_tools};
use serde_json::{Map, Value};

use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};

/// Why native tool calling was retained. Never includes prompt or argument contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("compact tools bypass: {self:?}")]
pub enum Bypass {
    Disabled,
    AgentOptedOut,
    Surface,
    NoTools,
    Streaming,
    ToolChoice,
    History,
    RequestOptions,
    UnsupportedSchema,
    NotSmaller,
}

impl Bypass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::AgentOptedOut => "agent_opted_out",
            Self::Surface => "unsupported_surface",
            Self::NoTools => "no_tools",
            Self::Streaming => "streaming",
            Self::ToolChoice => "tool_choice",
            Self::History => "tool_history",
            Self::RequestOptions => "request_options",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::NotSmaller => "not_smaller",
        }
    }
}

pub(crate) struct RouterPolicy<'a> {
    pub cfg: &'a crate::config::GatewayConfig,
    pub resolved: &'a crate::resolver::ResolvedConfig,
    pub format: crate::inbound::InboundFormat,
    pub wire_supported: bool,
}

/// Shared application boundary: disabled and bypass paths never touch request contents.
pub(crate) fn apply(
    request: &mut ChatRequest,
    policy: RouterPolicy<'_>,
) -> Result<Prepared, Bypass> {
    if !policy.cfg.compact_tools_enabled
        || !policy.cfg.compress_kill_switch
        || policy.cfg.compress_dry_run
    {
        return Err(Bypass::Disabled);
    }
    if !policy.resolved.compress_enabled {
        return Err(Bypass::AgentOptedOut);
    }
    if policy.format != crate::inbound::InboundFormat::OpenAi
        || policy.resolved.provider != "openai"
        || !policy.resolved.fallback_models.is_empty()
    {
        return Err(Bypass::Surface);
    }
    if !policy.wire_supported {
        return Err(Bypass::UnsupportedSchema);
    }
    prepare_request(request)
}

/// Holds validators compiled for precisely the definitions sent to the model.
pub struct Prepared {
    decoder: StreamDecoder,
}

impl Prepared {
    /// Restore client-facing calls only after the entire response validates.
    pub fn restore(mut self, mut response: ChatResponse) -> Result<ChatResponse, CompactError> {
        if response.choices.len() != 1 {
            return Err(CompactError::MalformedOutput);
        }
        let choice = &mut response.choices[0];
        if choice.finish_reason.as_deref() != Some("stop")
            || choice
                .message
                .tool_calls
                .as_ref()
                .is_some_and(|calls| !calls.is_empty())
        {
            return Err(CompactError::IncompleteCall);
        }
        let text = match &choice.message.content {
            Some(Value::String(text)) => text.as_str(),
            None | Some(Value::Null) => "",
            _ => return Err(CompactError::MalformedOutput),
        };
        self.decoder.push(text)?;
        let output = self.decoder.finish()?;
        if output.calls.is_empty() {
            return Ok(response);
        }
        choice.message.content = if output.text.trim().is_empty() {
            None
        } else {
            Some(Value::String(output.text))
        };
        choice.message.tool_calls = Some(
            output
                .calls
                .into_iter()
                .map(|call| ToolCall {
                    id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: call.name,
                        arguments: call.arguments.to_string(),
                    },
                    extra: Map::new(),
                })
                .collect(),
        );
        choice.finish_reason = Some("tool_calls".into());
        Ok(response)
    }
}

/// Check fields before the permissive IR drops unknown function-level properties.
/// In particular, a provider's `strict` contract must not disappear during compaction.
pub fn supports_wire_tools(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().all(|tool| {
                tool.as_object().is_some_and(|object| {
                    object
                        .keys()
                        .all(|key| matches!(key.as_str(), "type" | "function"))
                }) && tool
                    .get("function")
                    .and_then(Value::as_object)
                    .is_some_and(|function| {
                        function.keys().all(|key| {
                            matches!(key.as_str(), "name" | "description" | "parameters")
                        })
                    })
            })
        })
}

/// Build and validate a candidate, mutating the caller's request only after all gates pass.
/// The byte-size check is conservative engineering policy, not a token-savings claim.
pub fn prepare_request(request: &mut ChatRequest) -> Result<Prepared, Bypass> {
    check_request(request)?;
    let tools = to_tool_defs(request)?;
    let compact = encode_tools(&tools).map_err(|_| Bypass::UnsupportedSchema)?;
    let decoder = StreamDecoder::new(&tools).map_err(|_| Bypass::UnsupportedSchema)?;
    let before = serde_json::to_vec(request)
        .map_err(|_| Bypass::RequestOptions)?
        .len();
    let mut candidate = request.clone();
    candidate.tools = None;
    candidate.tool_choice = None;
    let insert_at = candidate
        .messages
        .iter()
        .take_while(|message| matches!(message.role.as_str(), "system" | "developer"))
        .count();
    candidate.messages.insert(
        insert_at,
        Message {
            role: "system".into(),
            content: Some(Value::String(compact.prompt())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        },
    );
    let after = serde_json::to_vec(&candidate)
        .map_err(|_| Bypass::RequestOptions)?
        .len();
    if after >= before {
        return Err(Bypass::NotSmaller);
    }
    *request = candidate;
    Ok(Prepared { decoder })
}

/// Convert at the boundary so the codec stays independent of router/provider types.
pub fn to_tool_defs(request: &ChatRequest) -> Result<Vec<ToolDef>, Bypass> {
    request
        .tools
        .as_ref()
        .ok_or(Bypass::NoTools)?
        .iter()
        .map(|tool| {
            if tool.kind != "function" || !tool.extra.is_empty() {
                return Err(Bypass::UnsupportedSchema);
            }
            Ok(ToolDef {
                name: tool.function.name.clone(),
                description: tool.function.description.clone(),
                parameters: tool.function.parameters.clone(),
            })
        })
        .collect()
}

fn check_request(request: &ChatRequest) -> Result<(), Bypass> {
    if request.tools.as_ref().is_none_or(Vec::is_empty) {
        return Err(Bypass::NoTools);
    }
    if request.is_streaming() {
        return Err(Bypass::Streaming);
    }
    if request
        .tool_choice
        .as_ref()
        .is_some_and(|choice| choice.as_str() != Some("auto"))
    {
        return Err(Bypass::ToolChoice);
    }
    if request.messages.iter().any(|message| {
        message.role == "tool"
            || message.role == "function"
            || message.tool_calls.is_some()
            || message.tool_call_id.is_some()
            || message.extra.contains_key("function_call")
    }) {
        return Err(Bypass::History);
    }
    // Preserve response-format, parallel-call, stop, logprob and multi-choice guarantees by bypassing.
    if request.extra.keys().any(|key| {
        !matches!(
            key.as_str(),
            "top_p"
                | "seed"
                | "presence_penalty"
                | "frequency_penalty"
                | "user"
                | "store"
                | "metadata"
                | "max_completion_tokens"
                | "service_tier"
        )
    }) {
        return Err(Bypass::RequestOptions);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GatewayConfig, ResolvedConfig, inbound::InboundFormat};
    use serde_json::json;

    fn resolved() -> ResolvedConfig {
        ResolvedConfig {
            provider: "openai".into(),
            model: "test".into(),
            litellm_model: "openai/test".into(),
            api_key: String::new(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: false,
            custom_endpoint: None,
            is_coding_agent: false,
            compress_enabled: true,
        }
    }

    #[test]
    fn disabled_is_byte_identical_including_passthrough_fields() {
        let mut request: ChatRequest = serde_json::from_value(json!({
            "messages":[{"role":"user","content":"Hi","custom_message":"retained"}],
            "tools":[{"type":"function","function":{"name":"f","parameters":{"type":"object"}}}],
            "tool_choice":"required", "stream":true, "custom_request":{"x":1}
        }))
        .unwrap();
        let before = serde_json::to_vec(&request).unwrap();
        let cfg = GatewayConfig::default();
        assert!(!cfg.compact_tools_enabled);
        assert_eq!(
            apply(
                &mut request,
                RouterPolicy {
                    cfg: &cfg,
                    resolved: &resolved(),
                    format: InboundFormat::OpenAi,
                    wire_supported: true
                }
            )
            .err(),
            Some(Bypass::Disabled)
        );
        assert_eq!(serde_json::to_vec(&request).unwrap(), before);
    }

    #[test]
    fn fleet_opt_in_does_not_override_agent_opt_out_or_dry_run() {
        let mut request: ChatRequest = serde_json::from_value(json!({"messages":[]})).unwrap();
        let mut cfg = GatewayConfig {
            compact_tools_enabled: true,
            ..Default::default()
        };
        let mut resolved = resolved();
        resolved.compress_enabled = false;
        assert_eq!(
            apply(
                &mut request,
                RouterPolicy {
                    cfg: &cfg,
                    resolved: &resolved,
                    format: InboundFormat::OpenAi,
                    wire_supported: true
                }
            )
            .err(),
            Some(Bypass::AgentOptedOut)
        );
        resolved.compress_enabled = true;
        cfg.compress_dry_run = true;
        assert_eq!(
            apply(
                &mut request,
                RouterPolicy {
                    cfg: &cfg,
                    resolved: &resolved,
                    format: InboundFormat::OpenAi,
                    wire_supported: true
                }
            )
            .err(),
            Some(Bypass::Disabled)
        );
    }
}
