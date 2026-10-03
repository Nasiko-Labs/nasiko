//! Payload compression at the egress seam.
//!
//! Runs on the normalized [`ChatRequest`] IR, so one implementation covers the OpenAI, Anthropic
//! and Gemini inbound surfaces. Operating on the IR rather than raw bytes means tool-call JSON,
//! `tool_call_id` threading and multimodal parts are structurally out of reach.
//!
//! # Why only `role == "tool"`
//!
//! The selection predicate must not depend on message index, turn count, or neighbouring
//! messages. Providers cache on a *stable prefix*, and that prefix grows every turn — so a
//! position-based rule ("last K messages") sends a given message uncompressed on turn T and
//! compressed on turn T+1, diverging the prefix and missing the cache every single turn.
//!
//! Selecting on role alone, with compression a pure function of the message's own content, makes
//! each message render to identical bytes on every turn it appears in. The prefix stays stable
//! from message 1, and tool results are where the JSON/logs/diffs live anyway.

use std::time::Instant;

use nasiko_compress::{Compressed, ContentType, Policy, compress};
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::resolver::ResolvedConfig;

/// Build the policy for one request.
///
/// Enablement is **per agent** (`agents.compress_enabled`), with the gateway flag acting only as
/// a fleet-wide kill switch — an operator can stop every agent's compression without touching
/// each agent's row. Everything else is tuning, which stays global.
pub(crate) fn policy_for(cfg: &GatewayConfig, resolved: &ResolvedConfig) -> Policy<'static> {
    Policy {
        enabled: cfg.compress_kill_switch && resolved.compress_enabled,
        min_bytes: cfg.compress_min_bytes,
        types: cfg.compress_types,
        level: cfg.compress_level,
        dry_run: cfg.compress_dry_run,
        ..Default::default()
    }
}

/// An original held back for recovery (IP-5). Minted before compression so the handle can be
/// interpolated into the elision marker; persisted by the caller, because this module is sync
/// and the store is not.
#[derive(Debug, Clone)]
pub(crate) struct Original {
    pub handle: uuid::Uuid,
    pub content: String,
    pub content_type: &'static str,
}

/// Mint a recovery handle for any payload at least this large.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Recovery {
    pub min_bytes: usize,
}

/// What ran, for `token_usage.metadata` and the span.
#[derive(Debug, Default, Clone)]
pub(crate) struct CompressionStats {
    pub applied: bool,
    pub dry_run: bool,
    pub level: &'static str,
    pub bytes_in: usize,
    pub bytes_out: usize,
    pub messages_touched: usize,
    pub by_type: [u16; ContentType::COUNT],
    pub elapsed_us: u64,
    /// Non-empty only when recovery is on and a handle was actually used.
    pub originals: Vec<Original>,
}

impl CompressionStats {
    /// `None` when nothing ran, so the row's metadata stays byte-identical to what it was before
    /// compression existed.
    pub(crate) fn to_metadata(&self) -> Option<Value> {
        if self.messages_touched == 0 {
            return None;
        }
        let mut types = serde_json::Map::new();
        for kind in ContentType::ALL {
            let n = self.by_type[kind.index()];
            if n > 0 {
                types.insert(kind.as_label().to_string(), Value::from(n));
            }
        }
        Some(serde_json::json!({
            "applied": self.applied,
            "dry_run": self.dry_run,
            "level": self.level,
            "bytes_in": self.bytes_in,
            "bytes_out": self.bytes_out,
            "messages_touched": self.messages_touched,
            "types": Value::Object(types),
            "elapsed_us": self.elapsed_us,
        }))
    }
}

/// Compress the request in place. Leaves it untouched when the policy is off or nothing shrank.
///
/// `recovery` mints a handle per compressed message and returns the original in
/// [`CompressionStats::originals`] for the caller to persist (IP-5).
pub(crate) fn apply(
    req: &mut ChatRequest,
    policy: &Policy<'_>,
    recovery: Option<Recovery>,
) -> CompressionStats {
    let mut stats = CompressionStats {
        level: policy.level.as_label(),
        dry_run: policy.dry_run,
        ..Default::default()
    };
    if !policy.enabled {
        return stats;
    }

    let started = Instant::now();
    for msg in req.messages.iter_mut() {
        // Assistant turns carrying tool calls are never tool results, whatever their role says.
        if msg.role != "tool" || msg.tool_calls.is_some() {
            continue;
        }
        if let Some(content) = msg.content.as_mut() {
            compress_content(content, policy, recovery, &mut stats);
        }
    }
    stats.applied = !policy.dry_run && stats.bytes_out < stats.bytes_in;
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    stats
}

fn compress_content(
    content: &mut Value,
    policy: &Policy<'_>,
    recovery: Option<Recovery>,
    stats: &mut CompressionStats,
) {
    match content {
        // Anthropic and Gemini inbound always flatten to this shape.
        Value::String(s) => compress_str(s, policy, recovery, stats),

        // OpenAI inbound preserves whatever the caller sent, including multimodal part arrays.
        // Each text part is compressed independently — compressing `Message::text()` and writing
        // the result back as a string would collapse the array and silently drop every
        // `image_url`/`input_audio` part, which the OpenAI provider would otherwise have sent
        // verbatim. `text()` joins parts with no separator, so per-part compression is what
        // Anthropic/Gemini egress sees too.
        Value::Array(parts) => {
            for part in parts.iter_mut() {
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(Value::String(text)) = part.get_mut("text") {
                    compress_str(text, policy, recovery, stats);
                }
            }
        }

        // Null, or a shape we do not model.
        _ => {}
    }
}

fn compress_str(
    text: &mut String,
    policy: &Policy<'_>,
    recovery: Option<Recovery>,
    stats: &mut CompressionStats,
) {
    // The handle has to exist *before* compression, because the marker builder interpolates it
    // while eliding. It is discarded below if nothing was actually elided, so an unused handle
    // never reaches the store.
    let minted = recovery
        .filter(|r| text.len() >= r.min_bytes)
        .map(|_| uuid::Uuid::new_v4());
    let handle_ref = minted.map(|h| h.to_string());

    let policy = match handle_ref.as_deref() {
        Some(handle) => Policy {
            recovery_ref: Some(handle),
            ..*policy
        },
        None => *policy,
    };

    let out: Compressed<'_> = compress(text, &policy);
    if out.saved_bytes() == 0 {
        return;
    }

    stats.messages_touched += 1;
    stats.bytes_in += out.original_bytes();
    stats.bytes_out += out.projected_bytes();
    stats.by_type[out.content_type().index()] += 1;

    if out.is_changed() {
        // Only now is the original unrecoverable from the wire, so only now is it worth storing.
        if let Some(handle) = minted {
            stats.originals.push(Original {
                handle,
                content: text.clone(),
                content_type: out.content_type().as_label(),
            });
        }
        *text = out.into_text();
    }
}


// ── IP-Compact-Tools: Sub-token Tool Schema Compaction ─────────────────────

/// Compacts tool schemas in a ChatRequest to sub-token micro-grammar.
/// Injects compact definitions and call instructions into the system message,
/// eliding verbose JSON Schema overhead and saving ~50% of prompt tokens.
#[allow(dead_code)]
pub(crate) fn apply_tool_compaction(
    req: &mut ChatRequest,
) -> Option<nasiko_tool_compact::CompactTools> {
    let tools = req.tools.as_ref()?;
    if tools.is_empty() {
        return None;
    }

    let tools_json = serde_json::to_value(tools).ok()?;
    let compact_defs: Vec<nasiko_tool_compact::ToolDef> = serde_json::from_value(tools_json).ok()?;
    let compact = nasiko_tool_compact::encode_tools(&compact_defs).ok()?;

    let instruction = format!(
        "\n\n# Tools Available (Compact Schema)\n{}\n\n{}",
        compact.compact_definitions,
        compact.call_instructions
    );

    let mut injected = false;
    for msg in req.messages.iter_mut() {
        if msg.role == "system" {
            if let Some(Value::String(s)) = &mut msg.content {
                s.push_str(&instruction);
                injected = true;
                break;
            }
        }
    }

    if !injected {
        req.messages.insert(0, crate::ir::chat::Message {
            role: "system".into(),
            content: Some(Value::String(instruction)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });
    }

    req.tools = None;
    Some(compact)
}

/// Decodes compact tool call markers (e.g. `<<call name {...}>>`) from model completion output
/// into standard OpenAI ToolCall IR structures, fail-closing on unknown or corrupt arguments.
#[allow(dead_code)]
pub(crate) fn restore_compact_tool_calls(
    response_text: &str,
    tools: &[crate::ir::chat::ToolDef],
) -> nasiko_tool_compact::Result<Vec<crate::ir::chat::ToolCall>> {
    let tools_json = serde_json::to_value(tools).map_err(|e| {
        nasiko_tool_compact::ToolCompactError::MalformedSyntax(e.to_string())
    })?;
    let compact_defs: Vec<nasiko_tool_compact::ToolDef> = serde_json::from_value(tools_json)
        .map_err(|e| nasiko_tool_compact::ToolCompactError::MalformedSyntax(e.to_string()))?;

    let decoded = nasiko_tool_compact::decode_calls(response_text, &compact_defs)?;
    let decoded_json = serde_json::to_value(&decoded).map_err(|e| {
        nasiko_tool_compact::ToolCompactError::MalformedSyntax(e.to_string())
    })?;
    let ir_tool_calls: Vec<crate::ir::chat::ToolCall> = serde_json::from_value(decoded_json)
        .map_err(|e| nasiko_tool_compact::ToolCompactError::MalformedSyntax(e.to_string()))?;

    Ok(ir_tool_calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{Message, ToolCall};
    use nasiko_compress::TypeMask;
    use serde_json::json;

    fn on() -> Policy<'static> {
        Policy {
            enabled: true,
            min_bytes: 0,
            types: TypeMask::ALL,
            ..Default::default()
        }
    }

    fn noisy_log() -> String {
        (0..300)
            .map(|i| format!("2026-01-01T00:00:00Z INFO handled request {i}\n"))
            .collect()
    }

    fn msg(role: &str, content: Value) -> Message {
        Message {
            role: role.into(),
            content: Some(content),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    fn req(messages: Vec<Message>) -> ChatRequest {
        ChatRequest {
            model: Some("gpt-4o".into()),
            messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    // ── IP-5: recovery handles ──────────────────────────────────────────────

    fn recovery() -> Option<Recovery> {
        Some(Recovery { min_bytes: 0 })
    }

    #[test]
    fn no_recovery_policy_mints_no_handles() {
        let mut r = req(vec![msg("tool", json!(noisy_log()))]);

        let stats = apply(&mut r, &on(), None);

        assert!(stats.originals.is_empty());
        let sent = r.messages[0].content.as_ref().unwrap().as_str().unwrap();
        assert!(
            !sent.contains("nasiko://c/"),
            "marker names a handle nobody stored"
        );
    }

    #[test]
    fn a_compressed_message_yields_its_original_and_a_marker_naming_the_handle() {
        let original = noisy_log();
        let mut r = req(vec![msg("tool", json!(original.clone()))]);

        let stats = apply(&mut r, &on(), recovery());

        assert_eq!(stats.originals.len(), 1);
        assert_eq!(
            stats.originals[0].content, original,
            "must store the pre-compression text"
        );

        let sent = r.messages[0].content.as_ref().unwrap().as_str().unwrap();
        assert!(
            sent.contains(&stats.originals[0].handle.to_string()),
            "the marker must name the handle the caller is about to persist: {sent}"
        );
    }

    #[test]
    fn payloads_below_the_floor_mint_nothing() {
        // A small elision is cheaper to re-send than to recover, and every handle is a row.
        let mut r = req(vec![msg("tool", json!(noisy_log()))]);

        let stats = apply(
            &mut r,
            &on(),
            Some(Recovery {
                min_bytes: usize::MAX,
            }),
        );

        assert!(stats.originals.is_empty());
        assert_eq!(stats.messages_touched, 1, "compression itself still ran");
    }

    #[test]
    fn dry_run_mints_nothing_because_nothing_was_elided() {
        // The request goes out whole, so there is nothing to recover — storing a copy would be
        // pure cost.
        let mut r = req(vec![msg("tool", json!(noisy_log()))]);
        let policy = Policy {
            dry_run: true,
            ..on()
        };

        let stats = apply(&mut r, &policy, recovery());

        assert!(stats.originals.is_empty());
        assert!(stats.bytes_out < stats.bytes_in, "but it still measured");
    }

    #[test]
    fn each_compressed_message_gets_its_own_handle() {
        let mut r = req(vec![
            msg("tool", json!(noisy_log())),
            msg("tool", json!(noisy_log())),
        ]);

        let stats = apply(&mut r, &on(), recovery());

        assert_eq!(stats.originals.len(), 2);
        assert_ne!(
            stats.originals[0].handle, stats.originals[1].handle,
            "one handle for two payloads would make recovery ambiguous"
        );
    }

    #[test]
    fn disabled_policy_leaves_the_request_byte_identical() {
        let mut r = req(vec![msg("tool", json!(noisy_log()))]);
        let before = serde_json::to_string(&r).unwrap();

        let stats = apply(&mut r, &Policy::default(), None);

        assert_eq!(serde_json::to_string(&r).unwrap(), before);
        assert_eq!(stats.messages_touched, 0);
        assert!(stats.to_metadata().is_none());
    }

    #[test]
    fn compresses_tool_results_only() {
        let log = noisy_log();
        let mut r = req(vec![
            msg("system", json!(log.clone())),
            msg("user", json!(log.clone())),
            msg("assistant", json!(log.clone())),
            msg("tool", json!(log.clone())),
        ]);

        let stats = apply(&mut r, &on(), None);

        assert_eq!(stats.messages_touched, 1);
        for i in 0..3 {
            assert_eq!(
                r.messages[i].content.as_ref().unwrap().as_str().unwrap(),
                log,
                "message {i} ({}) was compressed",
                r.messages[i].role
            );
        }
        let tool = r.messages[3].content.as_ref().unwrap().as_str().unwrap();
        assert!(tool.len() < log.len());
    }

    #[test]
    fn never_touches_the_last_user_message_that_routing_reads() {
        // `routing::latest_user_query` strips this marker; reflowing across it would silently
        // hand the classifier the whole transcript.
        let query = format!("{}\n\nCurrent message: what failed?", noisy_log());
        let mut r = req(vec![msg("user", json!(query.clone()))]);

        apply(&mut r, &on(), None);

        assert_eq!(
            crate::routing::latest_user_query(&r.messages).as_deref(),
            Some("what failed?")
        );
    }

    #[test]
    fn preserves_multimodal_parts_and_array_shape() {
        let mut r = req(vec![msg(
            "tool",
            json!([
                {"type": "text", "text": noisy_log()},
                {"type": "image_url", "image_url": {"url": "https://example.test/a.png"}},
                {"type": "input_audio", "input_audio": {"data": "AAAA", "format": "wav"}},
            ]),
        )]);

        apply(&mut r, &on(), None);

        let parts = r.messages[0].content.as_ref().unwrap().as_array().unwrap();
        assert_eq!(parts.len(), 3, "array length changed");
        assert_eq!(parts[1]["image_url"]["url"], "https://example.test/a.png");
        assert_eq!(parts[2]["input_audio"]["format"], "wav");
        assert!(parts[0]["text"].as_str().unwrap().len() < noisy_log().len());
    }

    #[test]
    fn skips_assistant_turns_carrying_tool_calls() {
        let mut m = msg("tool", json!(noisy_log()));
        m.tool_calls = Some(vec![ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            function: crate::ir::chat::FunctionCall {
                name: "search".into(),
                arguments: "{}".into(),
            },
            extra: Default::default(),
        }]);
        let mut r = req(vec![m]);

        let stats = apply(&mut r, &on(), None);

        assert_eq!(stats.messages_touched, 0);
    }

    #[test]
    fn preserves_tool_call_id_threading() {
        let mut m = msg("tool", json!(noisy_log()));
        m.tool_call_id = Some("call_42".into());
        let mut r = req(vec![m]);

        apply(&mut r, &on(), None);

        assert_eq!(r.messages[0].tool_call_id.as_deref(), Some("call_42"));
    }

    #[test]
    fn dry_run_reports_without_mutating() {
        let mut r = req(vec![msg("tool", json!(noisy_log()))]);
        let before = serde_json::to_string(&r).unwrap();
        let policy = Policy {
            dry_run: true,
            ..on()
        };

        let stats = apply(&mut r, &policy, None);

        assert_eq!(serde_json::to_string(&r).unwrap(), before);
        assert!(!stats.applied);
        assert!(stats.dry_run);
        assert!(stats.bytes_out < stats.bytes_in);
        assert_eq!(stats.to_metadata().unwrap()["dry_run"], true);
    }

    #[test]
    fn metadata_carries_counts_per_content_type() {
        let items: Vec<String> = (0..80)
            .map(|i| format!(r#"{{"id":{i},"ok":true}}"#))
            .collect();
        let mut r = req(vec![
            msg("tool", json!(noisy_log())),
            msg(
                "tool",
                json!(format!(r#"{{"items":[{}]}}"#, items.join(","))),
            ),
        ]);

        let stats = apply(&mut r, &on(), None);
        let meta = stats.to_metadata().unwrap();

        assert_eq!(meta["messages_touched"], 2);
        assert_eq!(meta["types"]["log"], 1);
        assert_eq!(meta["types"]["json"], 1);
        assert!(meta["bytes_out"].as_u64().unwrap() < meta["bytes_in"].as_u64().unwrap());
    }

    #[test]
    fn a_second_pass_changes_nothing() {
        let mut r = req(vec![msg("tool", json!(noisy_log()))]);
        apply(&mut r, &on(), None);
        let once = serde_json::to_string(&r).unwrap();

        apply(&mut r, &on(), None);

        assert_eq!(serde_json::to_string(&r).unwrap(), once);
    }
}
