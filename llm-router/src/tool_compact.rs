//! Opt-in compact tool schemas at the egress seam.
//!
//! Off by default. When disabled, [`apply`] is a no-op and the request is untouched
//! (byte-identical to pre-feature behaviour). When enabled, native `tools` are replaced
//! with a system message carrying the compact definitions + call-format instructions;
//! the model must emit `<<call ...>>` markers which the caller decodes with
//! [`nasiko_tool_compact::decode_calls`] / [`nasiko_tool_compact::StreamDecoder`].
//!
//! Partial coverage: request rewriting for OpenAI-shaped IR only (all inbound spokes
//! normalize here). Response / stream decoding through the router is not wired yet —
//! the eval example exercises the crate decoder directly.

use nasiko_tool_compact::{CompactError, ToolDef as CompactToolDef, encode_tools};
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::chat::{Message, ToolDef};
use crate::ir::ChatRequest;

/// Why compaction was skipped.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    NoTools,
    /// `tool_choice` forces a shape we cannot guarantee with compact markers.
    ForcedToolChoice,
    UnsupportedSchema,
}

impl Skipped {
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoTools => "no_tools",
            Self::ForcedToolChoice => "forced_tool_choice",
            Self::UnsupportedSchema => "unsupported_schema",
        }
    }
}

/// Metadata for `token_usage` / tests.
pub(crate) fn to_metadata(outcome: &Result<(), Skipped>) -> Value {
    match outcome {
        Ok(()) => serde_json::json!({ "applied": true }),
        Err(reason) => serde_json::json!({
            "applied": false,
            "skipped": reason.as_label(),
        }),
    }
}

/// Replace native tools with a compact system message when the flag is on.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> Result<(), Skipped> {
    if !cfg.tool_compact_enabled {
        return Err(Skipped::Disabled);
    }
    let Some(tools) = req.tools.as_ref() else {
        return Err(Skipped::NoTools);
    };
    if tools.is_empty() {
        return Err(Skipped::NoTools);
    }
    if forces_native_tools(req.tool_choice.as_ref()) {
        return Err(Skipped::ForcedToolChoice);
    }

    let compact_defs: Vec<CompactToolDef> = tools.iter().map(to_compact_def).collect();
    let encoded = match encode_tools(&compact_defs) {
        Ok(c) => c,
        Err(CompactError::UnsupportedSchema(_)) => return Err(Skipped::UnsupportedSchema),
        Err(_) => return Err(Skipped::UnsupportedSchema),
    };

    // Inject definitions; strip native tools so the provider does not also send them.
    req.messages.push(Message {
        role: "system".into(),
        content: Some(Value::String(encoded.prompt)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    req.tools = None;
    // Native tool_choice is meaningless without tools; clear soft "auto".
    if matches!(
        req.tool_choice.as_ref(),
        Some(Value::String(s)) if s == "auto" || s == "none"
    ) {
        req.tool_choice = None;
    }
    Ok(())
}

fn forces_native_tools(tool_choice: Option<&Value>) -> bool {
    match tool_choice {
        None => false,
        Some(Value::String(s)) => s != "auto" && s != "none",
        Some(Value::Object(m)) => m.get("type").and_then(Value::as_str) == Some("function")
            || m.contains_key("function"),
        _ => true,
    }
}

fn to_compact_def(t: &ToolDef) -> CompactToolDef {
    CompactToolDef {
        name: t.function.name.clone(),
        description: t.function.description.clone(),
        parameters: t.function.parameters.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::FunctionDef;
    use serde_json::json;

    fn sample_req() -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o-mini".into()),
            messages: vec![Message {
                role: "user".into(),
                content: Some(Value::String("hi".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "ping".into(),
                    description: Some("Ping".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {"x": {"type": "string"}},
                        "required": ["x"]
                    })),
                },
                extra: Default::default(),
            }]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_is_byte_identical() {
        let cfg = GatewayConfig {
            tool_compact_enabled: false,
            ..Default::default()
        };
        let original = sample_req();
        let mut req = original.clone();
        let outcome = apply(&mut req, &cfg);
        assert_eq!(outcome, Err(Skipped::Disabled));
        let before = serde_json::to_string(&original).unwrap();
        let after = serde_json::to_string(&req).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn enabled_strips_tools_and_injects_system() {
        let cfg = GatewayConfig {
            tool_compact_enabled: true,
            ..Default::default()
        };
        let mut req = sample_req();
        apply(&mut req, &cfg).unwrap();
        assert!(req.tools.is_none());
        assert!(
            req.messages
                .iter()
                .any(|m| m.role == "system" && m.text().is_some_and(|t| t.contains("<<call")))
        );
    }
}
