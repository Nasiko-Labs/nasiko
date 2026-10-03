//! P1 Compact Tool Protocol (`ctp/1`) router integration.
//!
//! Provides deterministic translation of native OpenAI tool definitions into compact
//! schema format, request transformation, and response translation back into native tool calls.

use serde_json::Value;

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::chat::{ChatRequest, ChatResponse, FunctionCall, Message, ToolCall};
use nasiko_tool_compact::types::ToolDef as CompactToolDef;
use nasiko_tool_compact::{decode_response, encode_tools};

/// Request-local preparation state to prevent double-compaction and preserve original schemas.
#[derive(Debug, Clone)]
pub enum CompactPreparation {
    /// Native passthrough — request was not compacted (disabled or ineligible).
    Native,
    /// Compact mode applied — holds original schemas for response decoding & validation.
    Compacted {
        original_tools: Vec<CompactToolDef>,
    },
}

/// Reason why a request was bypassed from compact tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BypassReason {
    FeatureDisabled,
    NoTools,
    StreamingUnsupported,
    MultipleChoices,
    UnsupportedToolChoice,
    NonTextMessages,
    PriorToolHistory,
    StructuredResponseRequested,
    UnsupportedToolSchema(String),
}

impl std::fmt::Display for BypassReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FeatureDisabled => write!(f, "COMPACT_TOOLS_ENABLED is false"),
            Self::NoTools => write!(f, "request has no tools"),
            Self::StreamingUnsupported => write!(f, "streaming is not yet supported for compact tools"),
            Self::MultipleChoices => write!(f, "n > 1 choices requested"),
            Self::UnsupportedToolChoice => write!(f, "tool_choice is not absent or 'auto'"),
            Self::NonTextMessages => write!(f, "request contains non-text message content"),
            Self::PriorToolHistory => write!(f, "request contains prior tool calls or results in message history"),
            Self::StructuredResponseRequested => write!(f, "response_format is set"),
            Self::UnsupportedToolSchema(err) => write!(f, "tool schema unsupported: {err}"),
        }
    }
}

/// Check eligibility of a chat request for compact tool encoding.
pub fn check_eligibility(req: &ChatRequest, cfg: &GatewayConfig) -> Result<(), BypassReason> {
    if !cfg.compact_tools_enabled {
        return Err(BypassReason::FeatureDisabled);
    }

    let tools = match &req.tools {
        Some(t) if !t.is_empty() => t,
        _ => return Err(BypassReason::NoTools),
    };

    if req.is_streaming() {
        return Err(BypassReason::StreamingUnsupported);
    }

    if let Some(n) = req.extra.get("n").and_then(|v| v.as_i64()) {
        if n > 1 {
            return Err(BypassReason::MultipleChoices);
        }
    }

    // tool_choice absent or "auto"
    if let Some(tc) = &req.tool_choice {
        match tc {
            Value::String(s) if s == "auto" => {}
            Value::Null => {}
            _ => return Err(BypassReason::UnsupportedToolChoice),
        }
    }

    if req.extra.contains_key("response_format") {
        return Err(BypassReason::StructuredResponseRequested);
    }

    // Text-only messages, and no prior tool history (role="tool" or assistant tool_calls)
    for msg in &req.messages {
        if msg.role == "tool" || msg.tool_calls.is_some() || msg.tool_call_id.is_some() {
            return Err(BypassReason::PriorToolHistory);
        }

        if let Some(content) = &msg.content {
            match content {
                Value::String(_) | Value::Null => {}
                Value::Array(parts) => {
                    for part in parts {
                        if part.get("type").and_then(|v| v.as_str()) != Some("text")
                            && part.get("text").is_none()
                        {
                            return Err(BypassReason::NonTextMessages);
                        }
                    }
                }
                _ => return Err(BypassReason::NonTextMessages),
            }
        }
    }

    let _ = tools;
    Ok(())
}

/// Convert IR tool definitions to `nasiko-tool-compact` ToolDef.
fn convert_tools(ir_tools: &[crate::ir::chat::ToolDef]) -> Vec<CompactToolDef> {
    ir_tools
        .iter()
        .map(|t| {
            CompactToolDef::new(
                t.function.name.clone(),
                t.function.description.clone(),
                t.function.parameters.clone().unwrap_or_else(|| serde_json::json!({})),
            )
        })
        .collect()
}

/// Prepare a chat request for compact tool mode.
///
/// If eligible and all tools are supported, modifies `req` in-place by removing native tools
/// and appending the compact instruction document as a system message.
/// Returns `CompactPreparation::Compacted` on success, or `CompactPreparation::Native` on bypass.
pub fn prepare_chat_request(req: &mut ChatRequest, cfg: &GatewayConfig) -> CompactPreparation {
    if let Err(reason) = check_eligibility(req, cfg) {
        tracing::debug!(
            target: "nasiko::llm_router::compact_tools",
            %reason,
            "compact_tools: request ineligible for compact tool protocol"
        );
        return CompactPreparation::Native;
    }

    let ir_tools = match req.tools.as_ref() {
        Some(t) => t,
        None => return CompactPreparation::Native,
    };

    let compact_defs = convert_tools(ir_tools);

    // Attempt encoding all tools with default limits.
    // If ANY tool fails encoding, bypass the ENTIRE request.
    let compact_tools = match encode_tools(&compact_defs) {
        Ok(ct) => ct,
        Err(err) => {
            tracing::info!(
                target: "nasiko::llm_router::compact_tools",
                %err,
                "compact_tools: unsupported tool schema detected — bypassing entire request"
            );
            return CompactPreparation::Native;
        }
    };

    // Remove native tools and tool_choice from outgoing request
    req.tools = None;
    req.tool_choice = None;

    // Insert compact instruction message
    let instruction_message = Message {
        role: "system".to_string(),
        content: Some(Value::String(compact_tools.text)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: serde_json::Map::new(),
    };

    req.messages.push(instruction_message);

    tracing::info!(
        target: "nasiko::llm_router::compact_tools",
        tool_count = compact_defs.len(),
        "compact_tools: request successfully prepared with compact tool protocol (ctp/1)"
    );

    CompactPreparation::Compacted {
        original_tools: compact_defs,
    }
}

/// Translate model response back into native OpenAI tool calls.
///
/// Decodes and validates any `<<call ...>>` calls against the original tool schemas.
/// On validation error, returns `GatewayError::Upstream`.
/// On success with calls, assigns fresh IDs and sets `finish_reason = "tool_calls"`.
pub fn translate_chat_response(
    resp: &mut ChatResponse,
    original_tools: &[CompactToolDef],
) -> Result<(), GatewayError> {
    for choice in &mut resp.choices {
        let content_text = match choice.message.text() {
            Some(t) => t,
            None => continue,
        };

        match decode_response(&content_text, original_tools) {
            Ok(decoded) => {
                if !decoded.calls.is_empty() {
                    let native_calls: Vec<ToolCall> = decoded
                        .calls
                        .into_iter()
                        .map(|c| ToolCall {
                            id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                            kind: "function".to_string(),
                            function: FunctionCall {
                                name: c.name,
                                arguments: c.arguments_json,
                            },
                            extra: serde_json::Map::new(),
                        })
                        .collect();

                    choice.message.tool_calls = Some(native_calls);
                    choice.finish_reason = Some("tool_calls".to_string());

                    let trimmed = decoded.text.trim();
                    if trimmed.is_empty() {
                        choice.message.content = None;
                    } else {
                        choice.message.content = Some(Value::String(trimmed.to_string()));
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "nasiko::llm_router::compact_tools",
                    %err,
                    "compact_tools: model output failed compact tool validation"
                );
                return Err(GatewayError::Upstream(format!(
                    "Compact tool validation error: {err}"
                )));
            }
        }
    }

    Ok(())
}

/// Strips `<<call ...>>` markers from model text, preserving surrounding user-visible content.
pub fn strip_call_markers(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut last_end = 0;
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i..].starts_with(b"<<call ") {
            let start = i;
            let mut j = i + 7;
            while j < bytes.len() && bytes[j] != b' ' && bytes[j] != b'{' {
                j += 1;
            }
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'{' {
                let mut brace_count = 0;
                let mut in_str = false;
                let mut escape = false;
                while j < bytes.len() {
                    let b = bytes[j];
                    if in_str {
                        if escape {
                            escape = false;
                        } else if b == b'\\' {
                            escape = true;
                        } else if b == b'"' {
                            in_str = false;
                        }
                    } else if b == b'"' {
                        in_str = true;
                    } else if b == b'{' {
                        brace_count += 1;
                    } else if b == b'}' {
                        brace_count -= 1;
                        if brace_count == 0 {
                            j += 1;
                            break;
                        }
                    }
                    j += 1;
                }

                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }

                if bytes[j..].starts_with(b">>") {
                    let end = j + 2;
                    out.push_str(&input[last_end..start]);
                    last_end = end;
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }

    out.push_str(&input[last_end..]);
    out
}
