//! Opt-in non-streaming tool compaction at the canonical chat IR seam.
//!
//! Request routing has already resolved before this runs. Provider adapters see a
//! normal message without native tools; decoded responses return canonical tool calls.

use nasiko_tool_compact::{ToolDef as CompactDef, decode_calls, encode_tools, strip_calls};
use serde_json::{Map, Value};

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};

pub(crate) struct Applied {
    original: Vec<CompactDef>,
}

/// Selection runs after route resolution, using the shared outbound client.
/// Streaming keeps native tools because Phase 1 router compaction is non-streaming.
pub(crate) async fn transform(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
    http: &reqwest::Client,
) -> Option<Applied> {
    if !cfg.tool_compaction_enabled {
        return None;
    }
    if let Some(outcome) =
        crate::tool_selection::select_request(req, &cfg.tool_selection, http, None).await
    {
        if outcome.native_fallback {
            return None;
        }
        if let Some(tools) = req.tools.take() {
            req.tools = Some(outcome.indices.iter().map(|&i| tools[i].clone()).collect());
        }
    }
    apply(req, cfg)
}

pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Option<Applied> {
    if !cfg.tool_compaction_enabled || req.is_streaming() {
        return None;
    }
    if req
        .tool_choice
        .as_ref()
        .is_some_and(|choice| choice != "auto")
    {
        return None;
    }
    if req.extra.contains_key("parallel_tool_calls")
        || req
            .messages
            .iter()
            .any(|m| m.role == "tool" || m.tool_calls.is_some())
    {
        return None;
    }
    let native = req.tools.as_ref()?;
    if native.is_empty()
        || native
            .iter()
            .any(|t| t.kind != "function" || !t.extra.is_empty())
    {
        return None;
    }
    let original = native
        .iter()
        .map(|tool| CompactDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        })
        .collect::<Vec<_>>();
    let compact = match encode_tools(&original) {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(%error, "tool compaction bypassed");
            return None;
        }
    };
    let instruction = format!(
        "Tools:\n{}\nUse <<call name {{JSON arguments}}>> for each call; else answer normally.",
        compact.text
    );
    req.messages.push(Message {
        role: "system".into(),
        content: Some(Value::String(instruction)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Map::new(),
    });
    req.tools = None;
    req.tool_choice = None;
    Some(Applied { original })
}

pub(crate) fn decode_response(
    resp: &mut ChatResponse,
    applied: &Applied,
) -> Result<(), GatewayError> {
    for choice in &mut resp.choices {
        if choice.message.tool_calls.is_some() {
            return Err(GatewayError::Upstream(
                "native tool calls on compact request".into(),
            ));
        }
        let content = match choice.message.content.as_ref() {
            Some(Value::String(content)) => content,
            Some(other) if other.to_string().contains("<<call") => {
                return Err(GatewayError::Upstream(
                    "compact marker in non-text response".into(),
                ));
            }
            _ => continue,
        };
        let calls = decode_calls(content, &applied.original).map_err(|error| {
            GatewayError::Upstream(format!("invalid compact tool call: {error}"))
        })?;
        if calls.is_empty() {
            continue;
        }
        let prose = strip_calls(content, &applied.original).map_err(|error| {
            GatewayError::Upstream(format!("invalid compact tool call: {error}"))
        })?;
        choice.message.content = if prose.is_empty() {
            None
        } else {
            Some(Value::String(prose))
        };
        choice.message.tool_calls = Some(
            calls
                .into_iter()
                .enumerate()
                .map(|(index, call)| ToolCall {
                    id: format!("call_compact_{}_{}", choice.index, index),
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> ChatRequest {
        serde_json::from_value(json!({
            "model":"gpt-4o-mini",
            "messages":[{"role":"user","content":"Plan the review"}],
            "tools":[{"type":"function","function":{
                "name":"make_event",
                "description":"Create an event",
                "parameters":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}
            }}]
        })).unwrap()
    }

    #[test]
    fn disabled_path_is_byte_identical() {
        let mut req = request();
        let before = serde_json::to_vec(&req).unwrap();
        assert!(apply(&mut req, &GatewayConfig::default()).is_none());
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    }

    #[test]
    fn unsupported_and_forced_choice_bypass() {
        let cfg = GatewayConfig {
            tool_compaction_enabled: true,
            ..Default::default()
        };
        let mut req = request();
        req.tools.as_mut().unwrap()[0]
            .function
            .parameters
            .as_mut()
            .unwrap()["oneOf"] = json!([]);
        let before = serde_json::to_vec(&req).unwrap();
        assert!(apply(&mut req, &cfg).is_none());
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
        let mut req = request();
        req.tool_choice = Some(json!("required"));
        assert!(apply(&mut req, &cfg).is_none());
        assert!(req.tools.is_some());
    }

    #[test]
    fn response_returns_native_shape_and_preserves_prose() {
        let cfg = GatewayConfig {
            tool_compaction_enabled: true,
            ..Default::default()
        };
        let mut req = request();
        let applied = apply(&mut req, &cfg).unwrap();
        assert!(req.tools.is_none());
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id":"x","model":"gpt-4o-mini",
            "choices":[{"index":0,"message":{"role":"assistant","content":
                "I will schedule it. <<call make_event {\"title\":\"Review\"}>>"
            },"finish_reason":"stop"}]
        }))
        .unwrap();
        decode_response(&mut resp, &applied).unwrap();
        let body = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            body["choices"][0]["message"]["content"],
            "I will schedule it."
        );
        assert_eq!(
            body["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "make_event"
        );
        assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn invalid_compact_response_is_not_sent_to_client() {
        let cfg = GatewayConfig {
            tool_compaction_enabled: true,
            ..Default::default()
        };
        let mut req = request();
        let applied = apply(&mut req, &cfg).unwrap();
        let mut resp: ChatResponse = serde_json::from_value(json!({
            "id":"x","model":"gpt-4o-mini",
            "choices":[{"index":0,"message":{"role":"assistant","content":
                "<<call make_event {}>>"
            },"finish_reason":"stop"}]
        }))
        .unwrap();
        assert!(matches!(
            decode_response(&mut resp, &applied),
            Err(GatewayError::Upstream(_))
        ));
        resp.choices[0].message.content = Some(json!([{"text":"<<call make_event {}>>"}]));
        assert!(matches!(
            decode_response(&mut resp, &applied),
            Err(GatewayError::Upstream(_))
        ));
    }
}
