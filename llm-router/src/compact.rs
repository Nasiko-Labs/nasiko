//! Compact tool-schema injection at the egress seam (opt-in).
//!
//! Runs on the normalized [`ChatRequest`] IR at the same seam as [`crate::compress`] and
//! [`crate::brevity`], so one implementation covers every inbound surface. When enabled, it
//! replaces the request's native `tools` (verbose JSON Schema) with the terse signature block
//! produced by [`nasiko_tool_compact`], injected as a trailing `system` message, and clears
//! `tools` so the provider sees only the compact form. The token win is on every request that
//! carries tool definitions.
//!
//! # Scope (documented, per the track brief)
//!
//! This is the **request-side** half: it reduces the tokens spent *defining* tools. The
//! **response-side** half — decoding a model's `<<call …>>` output back into standard
//! `tool_calls` — lives in [`nasiko_tool_compact::decode_calls`] /
//! [`nasiko_tool_compact::StreamDecoder`] and is exercised by the `compact_tools_eval` example;
//! wiring that into the provider response path (so a compacted request's replies are decoded
//! transparently) is the remaining integration step. Because of that, compaction is **off by
//! default** ([`GatewayConfig::compact_tools_enabled`]); with the flag off this function is a
//! no-op and the outbound request is byte-identical to today.
//!
//! # Fail-open on the transform, fail-closed on the data
//!
//! If a tool's schema can't be rendered compactly (an unsupported JSON Schema feature), the
//! whole request is left untouched rather than sent half-compacted — the model must never see a
//! tool whose definition we couldn't faithfully encode. The decoder on the return path is the
//! fail-*closed* half: it refuses invalid calls rather than guessing.

use nasiko_tool_compact::{decode_calls, encode_tools, ToolDef as CompactToolDef};
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::chat::{ChatResponse, FunctionCall, ToolCall};
use crate::ir::ChatRequest;

/// Why compaction did not run (or the fact that it did), for telemetry and tests.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum CompactOutcome {
    /// The flag is off — the request is byte-identical to a build without this seam.
    Disabled,
    /// The request carried no tools, so there was nothing to compact.
    NoTools,
    /// A streaming request: only the non-streaming reply path decodes `<<call>>` back, so
    /// compaction is skipped here to keep streamed tool calls intact.
    Streaming,
    /// A tool used a schema feature we cannot render compactly; the request was left native
    /// rather than sent partially compacted.
    Bypassed,
    /// Compaction applied: native `tools` removed, compact definitions injected. Carries the
    /// number of tools compacted and the measured byte sizes of the native vs compact tool
    /// definitions, so the usage row can report what this layer saved.
    Applied {
        tools: usize,
        native_bytes: usize,
        compact_bytes: usize,
    },
}

impl CompactOutcome {
    fn label(self) -> &'static str {
        match self {
            CompactOutcome::Disabled => "disabled",
            CompactOutcome::NoTools => "no_tools",
            CompactOutcome::Streaming => "streaming_skipped",
            CompactOutcome::Bypassed => "bypassed",
            CompactOutcome::Applied { .. } => "applied",
        }
    }

    /// `None` when nothing ran, so the row's metadata stays byte-identical to what it was before
    /// this seam existed (mirrors [`crate::compress::CompressionStats::to_metadata`]).
    pub(crate) fn to_metadata(self) -> Option<Value> {
        match self {
            CompactOutcome::Disabled | CompactOutcome::NoTools | CompactOutcome::Streaming => None,
            CompactOutcome::Applied { tools, native_bytes, compact_bytes } => {
                Some(serde_json::json!({
                    "applied": true,
                    "tools": tools,
                    "native_bytes": native_bytes,
                    "compact_bytes": compact_bytes,
                    "saved_bytes": native_bytes.saturating_sub(compact_bytes),
                }))
            }
            CompactOutcome::Bypassed => Some(serde_json::json!({
                "applied": false,
                "skipped": self.label(),
            })),
        }
    }
}

/// Inject compact tool definitions in place of native `tools`, unless a carve-out applies.
///
/// Off by default: with [`GatewayConfig::compact_tools_enabled`] unset this returns
/// [`CompactOutcome::Disabled`] and leaves `req` untouched.
pub(crate) fn apply(req: &mut ChatRequest, cfg: &GatewayConfig) -> CompactOutcome {
    if !cfg.compact_tools_enabled {
        return CompactOutcome::Disabled;
    }
    // Only the non-streaming reply path decodes `<<call>>` markers back into tool calls, so a
    // streaming request must keep its native tools or its tool call would never be reassembled.
    if req.is_streaming() {
        return CompactOutcome::Streaming;
    }
    let Some(tools) = req.tools.as_ref() else {
        return CompactOutcome::NoTools;
    };
    if tools.is_empty() {
        return CompactOutcome::NoTools;
    }
    // A forced `tool_choice` (a specific tool, or "required") cannot be guaranteed once native
    // `tools` is removed — the provider has nothing left to force a call against, and (worse)
    // some providers reject "required" with no `tools` outright rather than silently ignoring
    // it. Bypass compaction rather than send a request the provider will refuse.
    if let Some(choice) = req.tool_choice.as_ref() {
        let forced = match choice {
            Value::String(s) => s == "required",
            Value::Object(_) => true,
            _ => false,
        };
        if forced {
            return CompactOutcome::Bypassed;
        }
    }

    // Convert router tool defs → the crate's own type at the seam (the crate never depends on
    // the router, so the router owns this direction of the conversion).
    let compact_defs = to_compact_defs(tools);

    let compact = match encode_tools(&compact_defs) {
        Ok(c) => c,
        // Unsupported/malformed schema → leave the request native rather than send it
        // half-compacted. Fail open on the transform; the model still gets correct tools.
        Err(_) => return CompactOutcome::Bypassed,
    };

    let count = compact_defs.len();
    // Measured sizes so the usage row can report what this layer saved: the native tool-schema
    // JSON vs the injected compact block (bytes, a cheap proxy the savings read path calibrates).
    let native_bytes = serde_json::to_string(tools).map(|s| s.len()).unwrap_or(0);
    let rendered = compact.render();
    let compact_bytes = rendered.len();
    req.tools = None;
    req.messages.push(crate::ir::chat::Message {
        role: "system".into(),
        content: Some(Value::String(rendered)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    CompactOutcome::Applied { tools: count, native_bytes, compact_bytes }
}

/// Convert router [`crate::ir::chat::ToolDef`]s into the crate's own `ToolDef`. The crate never
/// depends on the router, so this seam owns the conversion in both directions.
fn to_compact_defs(tools: &[crate::ir::chat::ToolDef]) -> Vec<CompactToolDef> {
    tools
        .iter()
        .map(|t| CompactToolDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect()
}

/// Decode a model's compact reply back into standard OpenAI `tool_calls` (the response-side half
/// of compaction). For each choice whose assistant content contains `<<call …>>` markers that
/// validate against `original_tools`, the markers are replaced by structured `tool_calls` and the
/// finish reason becomes `tool_calls`. Returns the number of calls decoded.
///
/// Fail-open on the *reply*: a plain text answer (no marker) or a malformed/unknown call (decode
/// error) leaves the choice's text untouched rather than inventing a call — the fail-*closed*
/// guarantee (no guessed call) lives in [`nasiko_tool_compact::decode_calls`], which returns an
/// error we simply decline to act on here.
pub(crate) fn decode_response(resp: &mut ChatResponse, original_tools: &[crate::ir::chat::ToolDef]) -> usize {
    let compact_defs = to_compact_defs(original_tools);
    let mut decoded_total = 0usize;
    for choice in &mut resp.choices {
        let Some(text) = choice.message.text() else {
            continue;
        };
        let Ok(calls) = decode_calls(&text, &compact_defs) else {
            continue; // unknown tool / invalid args → keep the raw text, never a guessed call
        };
        if calls.is_empty() {
            continue; // a plain answer with no tool call
        }
        let tool_calls: Vec<ToolCall> = calls
            .iter()
            .enumerate()
            .map(|(i, c)| ToolCall {
                id: format!("call_{i}"),
                kind: "function".into(),
                function: FunctionCall {
                    name: c.name.clone(),
                    arguments: serde_json::to_string(&c.arguments).unwrap_or_else(|_| "{}".into()),
                },
                extra: Default::default(),
            })
            .collect();
        decoded_total += tool_calls.len();
        choice.message.tool_calls = Some(tool_calls);
        choice.message.content = None; // the markers are now structured calls
        choice.finish_reason = Some("tool_calls".into());
    }
    decoded_total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{FunctionDef, Message, ToolDef};
    use serde_json::json;

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            compact_tools_enabled: enabled,
            ..GatewayConfig::default()
        }
    }

    fn calendar_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event.".into()),
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

    fn req_with_tools() -> ChatRequest {
        ChatRequest {
            model: None,
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!("Book a review")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![calendar_tool()]),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn disabled_is_byte_identical() {
        let mut req = req_with_tools();
        let before = serde_json::to_string(&req).unwrap();
        let outcome = apply(&mut req, &cfg(false));
        assert_eq!(outcome, CompactOutcome::Disabled);
        assert_eq!(serde_json::to_string(&req).unwrap(), before, "off must not mutate the request");
        assert!(outcome.to_metadata().is_none());
    }

    #[test]
    fn enabled_removes_native_tools_and_injects_compact_system_message() {
        let mut req = req_with_tools();
        let outcome = apply(&mut req, &cfg(true));
        assert!(matches!(outcome, CompactOutcome::Applied { tools: 1, .. }));
        assert!(req.tools.is_none(), "native tools must be removed");
        let last = req.messages.last().unwrap();
        assert_eq!(last.role, "system");
        let text = last.content.as_ref().unwrap().as_str().unwrap();
        assert!(text.contains("create_calendar_event("), "compact signature injected: {text}");
        assert!(text.contains("visibility?:public|private"), "enum preserved: {text}");
        let meta = outcome.to_metadata().unwrap();
        assert_eq!(meta["applied"], json!(true));
        assert_eq!(meta["tools"], json!(1));
        // Byte sizes are recorded for the savings read path. A single tiny tool does not amortize
        // the fixed call-instruction block, so `saved_bytes` can legitimately be zero here; the
        // win shows at scale (see `demo_router_compacts_a_real_request`).
        assert!(meta["native_bytes"].as_u64().is_some());
        assert!(meta["compact_bytes"].as_u64().is_some());
        assert!(meta["saved_bytes"].as_u64().is_some());
    }

    #[test]
    fn no_tools_is_a_noop() {
        let mut req = req_with_tools();
        req.tools = None;
        let outcome = apply(&mut req, &cfg(true));
        assert_eq!(outcome, CompactOutcome::NoTools);
        assert!(outcome.to_metadata().is_none());
    }

    /// A forced `tool_choice` cannot be guaranteed once native `tools` is removed — some
    /// providers reject `"required"` outright when `tools` is empty/missing, which previously
    /// surfaced as a live 400 from the upstream provider instead of a graceful bypass.
    #[test]
    fn forced_tool_choice_bypasses_compaction() {
        for choice in [json!("required"), json!({"type": "function", "function": {"name": "create_calendar_event"}})] {
            let mut req = req_with_tools();
            req.tool_choice = Some(choice.clone());
            let before = serde_json::to_string(&req).unwrap();
            let outcome = apply(&mut req, &cfg(true));
            assert_eq!(outcome, CompactOutcome::Bypassed, "forced tool_choice {choice:?} must bypass");
            assert_eq!(serde_json::to_string(&req).unwrap(), before, "bypass must not mutate the request");
        }
    }

    /// `tool_choice: "auto"` (or unset) is not a forced choice — compaction still applies.
    #[test]
    fn auto_tool_choice_does_not_bypass() {
        let mut req = req_with_tools();
        req.tool_choice = Some(json!("auto"));
        let outcome = apply(&mut req, &cfg(true));
        assert!(matches!(outcome, CompactOutcome::Applied { .. }));
    }

    /// Visible end-to-end demo of the real router seam. Run it with:
    /// `cargo test -p nasiko-llm-router --lib compact::tests::demo -- --nocapture`
    /// to watch the actual router code turn a native tool request into the compact form.
    #[test]
    fn demo_router_compacts_a_real_request() {
        let email = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: Some("Send an email.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "body"]
                })),
            },
            extra: Default::default(),
        };
        let mut req = req_with_tools();
        req.tools.as_mut().unwrap().push(email);

        let before = serde_json::to_string_pretty(&req).unwrap();
        let outcome = apply(&mut req, &cfg(true));
        let after = serde_json::to_string_pretty(&req).unwrap();

        println!("\n================ ROUTER compact::apply (TOKEN_COMPACT_TOOLS=on) ================");
        println!("\n----- BEFORE (native tool schemas on the request) -----\n{before}");
        println!("\n----- AFTER (compact definitions injected, native tools removed) -----\n{after}");
        println!("\noutcome = {outcome:?}");
        println!("================================================================================\n");

        assert!(matches!(outcome, CompactOutcome::Applied { tools: 2, .. }));
        assert!(req.tools.is_none());
        let injected = req.messages.last().unwrap().content.as_ref().unwrap().as_str().unwrap();
        assert!(injected.contains("send_email("));
        assert!(injected.contains("create_calendar_event("));
    }

    #[test]
    fn decode_response_turns_compact_reply_into_tool_calls() {
        use crate::ir::chat::{ChatResponse, Choice, Message};
        let tools = vec![calendar_tool()];
        let mut resp = ChatResponse {
            id: "x".into(),
            object: "chat.completion".into(),
            created: None,
            model: "m".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(json!(
                        "<<call create_calendar_event {\"title\":\"Review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>"
                    )),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        };
        let decoded = decode_response(&mut resp, &tools);
        assert_eq!(decoded, 1);
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        assert!(choice.message.content.is_none());
        let calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], json!("Review"));
    }

    #[test]
    fn decode_response_leaves_plain_answer_untouched() {
        use crate::ir::chat::{ChatResponse, Choice, Message};
        let tools = vec![calendar_tool()];
        let mut resp = ChatResponse {
            id: "x".into(),
            object: "chat.completion".into(),
            created: None,
            model: "m".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(json!("It is sunny today.")),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: None,
            extra: Default::default(),
        };
        let decoded = decode_response(&mut resp, &tools);
        assert_eq!(decoded, 0);
        assert_eq!(resp.choices[0].message.content, Some(json!("It is sunny today.")));
        assert!(resp.choices[0].message.tool_calls.is_none());
    }
}
