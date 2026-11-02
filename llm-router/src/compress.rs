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
//!
//! The precise invariant is narrower than "role only": a message's rendering may depend only on
//! bytes that never change once sent. One input besides the content meets it — the tool call that
//! produced the result. A result whose call asked for an exact line range is sent whole
//! ([`crate::range_view`]): the agent asked for those lines, and cutting them only makes it read
//! them again. Recovery handles meet it too: they are derived from the flow and the content, and
//! minted only when the request offers the recovery tool, which is fixed for an agent.

use std::time::Instant;

use nasiko_compress::{Compressed, ContentType, Policy, compress};
use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::range_view;
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

/// The tool an agent calls to fetch an elided original back (served by the MCP gateway).
const RECOVER_TOOL: &str = "recover_compressed";

/// Mint a recovery handle for any payload at least this large.
///
/// The handle is **derived, not minted**: it is a UUIDv5 over a fixed namespace with the
/// `"{flow}|{content}"` name, so the same payload inside the same flow always yields the same
/// handle — and therefore the same elision marker — and the provider prefix cache keeps holding.
/// A random UUIDv4 per call drifted the marker bytes every turn, which is what re-broke the
/// prefix cache on every compressed message (the P1 regression in the eval handoff).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Recovery<'a> {
    pub min_bytes: usize,
    /// The flow the payload belongs to. Recovery is scoped to the flow, so the handle is too:
    /// identical content in two flows must not land on one row only the first flow can read.
    pub flow_id: &'a str,
}

impl<'a> Recovery<'a> {
    /// Recovery for this request, or `None` when a handle could never be used.
    ///
    /// A handle is only worth minting when the agent can call [`RECOVER_TOOL`] and the request is
    /// flow-scoped (originals are stored per flow). Without the tool the marker advertises a path
    /// the agent cannot take, and every call stores an original nobody reads.
    pub(crate) fn for_request(
        enabled: bool,
        min_bytes: usize,
        flow_id: Option<&'a str>,
        req: &ChatRequest,
    ) -> Option<Self> {
        if !enabled || !offers_recovery_tool(req) {
            return None;
        }
        flow_id.map(|flow_id| Recovery { min_bytes, flow_id })
    }

    /// [`Self::for_request`] for a Responses body, whose tools name themselves at the top level.
    pub(crate) fn for_responses(
        enabled: bool,
        min_bytes: usize,
        flow_id: Option<&'a str>,
        body: &Value,
    ) -> Option<Self> {
        let offered = body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                tools.iter().any(|t| {
                    t.get("name")
                        .or_else(|| t.pointer("/function/name"))
                        .and_then(Value::as_str)
                        .is_some_and(|n| n.ends_with(RECOVER_TOOL))
                })
            });
        if !enabled || !offered {
            return None;
        }
        flow_id.map(|flow_id| Recovery { min_bytes, flow_id })
    }
}

/// Whether the request lists the recovery tool. A suffix match, because MCP clients commonly
/// prefix tool names with their server (`nasiko__recover_compressed`).
fn offers_recovery_tool(req: &ChatRequest) -> bool {
    req.tools.as_deref().is_some_and(|tools| {
        tools
            .iter()
            .any(|t| t.function.name.ends_with(RECOVER_TOOL))
    })
}

/// Deterministic recovery handle: UUIDv5 (SHA-1) over the payload, namespaced by flow.
fn mint_handle(flow_id: &str, content: &str) -> uuid::Uuid {
    let name = format!("{flow_id}|{content}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, name.as_bytes())
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
    recovery: Option<Recovery<'_>>,
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
    let requested_ranges = range_view::requested_range_call_ids(&req.messages);
    for msg in req.messages.iter_mut() {
        // Assistant turns carrying tool calls are never tool results, whatever their role says.
        if msg.role != "tool" || msg.tool_calls.is_some() {
            continue;
        }
        if msg
            .tool_call_id
            .as_ref()
            .is_some_and(|id| requested_ranges.contains(id))
        {
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

/// What a compression pass leaves for the usage row: its metadata block and, when it really reduced
/// the payload, the `(bytes_in, bytes_out)` the savings ledger credits.
#[derive(Debug, Default, Clone)]
pub(crate) struct Ledger {
    pub metadata: Option<Value>,
    pub bytes: Option<(usize, usize)>,
}

impl CompressionStats {
    pub(crate) fn ledger(&self) -> Ledger {
        Ledger {
            metadata: self.to_metadata(),
            bytes: self.applied.then_some((self.bytes_in, self.bytes_out)),
        }
    }
}

/// The Responses item types that carry a tool's result: Codex's function tools, and its freeform
/// (`apply_patch`-style) custom tools.
const RESPONSES_TOOL_OUTPUTS: [&str; 2] = ["function_call_output", "custom_tool_call_output"];

/// [`apply`] for the Responses surface (`/v1/responses`, Codex), on the raw `input` array: its
/// tool results are items, not `role: "tool"` messages. The same rule as [`apply`] holds — an item
/// is picked by its type alone and compressed as a pure function of its own content — so the
/// stable prefix providers cache on stays stable. `output` is a string, or an array of text parts.
pub(crate) fn apply_responses_input(
    input: &mut Value,
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
    let Some(items) = input.as_array_mut() else {
        return stats;
    };

    let started = Instant::now();
    for item in items.iter_mut() {
        let is_output = item
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| RESPONSES_TOOL_OUTPUTS.contains(&kind));
        if !is_output {
            continue;
        }
        match item.get_mut("output") {
            Some(Value::String(text)) => compress_str(text, policy, recovery, &mut stats),
            Some(Value::Array(parts)) => {
                for part in parts.iter_mut() {
                    // `input_text`/`output_text`; images and files pass untouched.
                    let is_text = part
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.ends_with("text"));
                    if !is_text {
                        continue;
                    }
                    if let Some(Value::String(text)) = part.get_mut("text") {
                        compress_str(text, policy, recovery, &mut stats);
                    }
                }
            }
            _ => {}
        }
    }
    stats.applied = !policy.dry_run && stats.bytes_out < stats.bytes_in;
    stats.elapsed_us = started.elapsed().as_micros() as u64;
    stats
}

fn compress_content(
    content: &mut Value,
    policy: &Policy<'_>,
    recovery: Option<Recovery<'_>>,
    stats: &mut CompressionStats,
) {
    match content {
        // Anthropic and Gemini inbound flatten to this shape, unless the agent set a cache marker.
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
    recovery: Option<Recovery<'_>>,
    stats: &mut CompressionStats,
) {
    // The handle has to exist *before* compression, because the marker builder interpolates it
    // while eliding. It is discarded below if nothing was actually elided, so an unused handle
    // never reaches the store.
    let minted = recovery
        .filter(|r| text.len() >= r.min_bytes)
        .map(|r| mint_handle(r.flow_id, text));
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

    fn recovery() -> Option<Recovery<'static>> {
        Some(Recovery {
            min_bytes: 0,
            flow_id: "flow-1",
        })
    }

    // ── the Responses surface (Codex) ───────────────────────────────────────

    #[test]
    fn responses_tool_outputs_are_compressed_and_everything_else_is_left_alone() {
        let log = noisy_log();
        let mut input = json!([
            {"role": "user", "content": [{"type": "input_text", "text": log}]},
            {"type": "function_call", "call_id": "c1", "name": "shell", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": log},
            {"type": "custom_tool_call_output", "call_id": "c2",
             "output": [{"type": "input_text", "text": log}, {"type": "input_image", "image_url": "x"}]},
        ]);

        let stats = apply_responses_input(&mut input, &on(), None);

        assert_eq!(stats.messages_touched, 2);
        assert!(stats.applied && stats.bytes_out < stats.bytes_in);
        assert!(input[2]["output"].as_str().unwrap().len() < log.len());
        assert!(input[3]["output"][0]["text"].as_str().unwrap().len() < log.len());
        assert_eq!(input[3]["output"][1]["image_url"], "x");
        // The user's own words are never compressed, whatever they contain.
        assert_eq!(input[0]["content"][0]["text"], log.as_str());
        let ledger = stats.ledger();
        assert_eq!(ledger.bytes, Some((stats.bytes_in, stats.bytes_out)));
        assert!(ledger.metadata.is_some());
    }

    #[test]
    fn responses_compression_is_off_unless_the_agent_opted_in() {
        let mut input =
            json!([{"type": "function_call_output", "call_id": "c1", "output": noisy_log()}]);
        let before = input.clone();

        let stats = apply_responses_input(&mut input, &Policy::default(), None);

        assert_eq!(input, before);
        assert_eq!(stats.messages_touched, 0);
        assert_eq!(stats.ledger().bytes, None);
    }

    #[test]
    fn responses_compression_is_the_same_bytes_every_turn() {
        // The stable prefix: the same output item renders identically however much follows it.
        let item = json!({"type": "function_call_output", "call_id": "c1", "output": noisy_log()});
        let mut one = json!([item.clone()]);
        let mut two = json!([item, {"role": "user", "content": "next"}]);

        apply_responses_input(&mut one, &on(), None);
        apply_responses_input(&mut two, &on(), None);

        assert_eq!(one[0], two[0]);
    }

    #[test]
    fn a_marked_tool_result_compresses_to_the_same_bytes_as_an_unmarked_one() {
        // An agent moves its marker to the latest tool result each turn, so the same result
        // arrives marked (a text part) one turn and unmarked (a string) the next. The cached
        // prefix holds only if both compress to the same text.
        let marker = json!({ "type": "ephemeral" });
        let mut marked = req(vec![msg(
            "tool",
            json!([{ "type": "text", "text": noisy_log(), "cache_control": marker }]),
        )]);
        let mut unmarked = req(vec![msg("tool", json!(noisy_log()))]);

        apply(&mut marked, &on(), recovery());
        apply(&mut unmarked, &on(), recovery());

        let part = &marked.messages[0].content.as_ref().unwrap()[0];
        assert_eq!(part["text"], unmarked.messages[0].content.clone().unwrap());
        assert_eq!(part["cache_control"], marker);
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
                flow_id: "flow-1",
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
    fn identical_payloads_in_one_flow_mint_the_same_stable_handle() {
        // The handoff plan (Remedy 1 / P1): the handle is derived from the payload, so identical
        // content compresses to byte-identical markers and the provider prefix cache holds across
        // calls. A fresh UUIDv4 per call made the marker — and the compressed prefix — drift on
        // every turn, so the cache broke after the first compression.
        let mut r = req(vec![
            msg("tool", json!(noisy_log())),
            msg("tool", json!(noisy_log())),
        ]);

        let stats = apply(&mut r, &on(), recovery());

        assert_eq!(stats.originals.len(), 2, "two elisions recorded");
        assert_eq!(
            stats.originals[0].handle, stats.originals[1].handle,
            "identical content in one flow must derive one stable handle"
        );
        let sent = r.messages[0].content.as_ref().unwrap().as_str().unwrap();
        let sent2 = r.messages[1].content.as_ref().unwrap().as_str().unwrap();
        assert_eq!(
            sent, sent2,
            "markers must be byte-identical for identical content"
        );
    }

    #[test]
    fn handles_are_scoped_by_flow_so_content_does_not_leak_across_flows() {
        let in_flow = |flow_id| {
            let mut r = req(vec![msg("tool", json!(noisy_log()))]);
            apply(
                &mut r,
                &on(),
                Some(Recovery {
                    min_bytes: 0,
                    flow_id,
                }),
            )
        };
        let stats_a = in_flow("flow-a");
        let stats_b = in_flow("flow-b");

        assert_eq!(stats_a.originals.len(), 1);
        assert_eq!(stats_b.originals.len(), 1);
        assert_ne!(
            stats_a.originals[0].handle, stats_b.originals[0].handle,
            "the same content in two flows must not share a handle"
        );
    }

    #[test]
    fn different_payloads_in_one_flow_get_different_handles() {
        let mut r = req(vec![
            msg("tool", json!(noisy_log())),
            msg(
                "tool",
                json!(noisy_log().replace("handled request", "served page")),
            ),
        ]);

        let stats = apply(&mut r, &on(), recovery());

        assert_eq!(stats.originals.len(), 2);
        assert_ne!(
            stats.originals[0].handle, stats.originals[1].handle,
            "distinct payloads must never alias each other's handle"
        );
    }

    fn tool(name: &str) -> crate::ir::chat::ToolDef {
        serde_json::from_value(json!({"type": "function", "function": {"name": name}})).unwrap()
    }

    #[test]
    fn recovery_needs_the_recovery_tool_and_a_flow() {
        let mut with_tool = req(vec![]);
        with_tool.tools = Some(vec![tool("bash"), tool("nasiko__recover_compressed")]);
        let mut bash_only = req(vec![]);
        bash_only.tools = Some(vec![tool("bash")]);

        assert!(Recovery::for_request(true, 0, Some("f"), &with_tool).is_some());
        assert!(Recovery::for_request(true, 0, Some("f"), &bash_only).is_none());
        assert!(Recovery::for_request(true, 0, None, &with_tool).is_none());
        assert!(Recovery::for_request(false, 0, Some("f"), &with_tool).is_none());
    }

    #[test]
    fn a_responses_body_needs_the_recovery_tool_and_a_flow_too() {
        let with_tool = json!({"tools": [
            {"type": "function", "name": "bash"},
            {"type": "function", "name": "nasiko__recover_compressed"},
        ]});
        let bash_only = json!({"tools": [{"type": "function", "name": "bash"}]});

        assert!(Recovery::for_responses(true, 0, Some("f"), &with_tool).is_some());
        assert!(Recovery::for_responses(true, 0, Some("f"), &bash_only).is_none());
        assert!(Recovery::for_responses(true, 0, Some("f"), &json!({})).is_none());
        assert!(Recovery::for_responses(true, 0, None, &with_tool).is_none());
        assert!(Recovery::for_responses(false, 0, Some("f"), &with_tool).is_none());
    }

    fn call(id: &str, command: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: crate::ir::chat::FunctionCall {
                name: "bash".into(),
                arguments: json!({ "command": command }).to_string(),
            },
            extra: Default::default(),
        }
    }

    fn result_of(id: &str, content: &str) -> Message {
        let mut m = msg("tool", json!(content));
        m.tool_call_id = Some(id.into());
        m
    }

    #[test]
    fn a_requested_line_range_is_sent_whole() {
        let log = noisy_log();
        let mut assistant = msg("assistant", Value::Null);
        assistant.tool_calls = Some(vec![
            call("ranged", "cd /repo && sed -n '1,300p' app.log"),
            call("whole", "cat app.log"),
        ]);
        let mut r = req(vec![
            assistant,
            result_of("ranged", &log),
            result_of("whole", &log),
        ]);

        let stats = apply(&mut r, &on(), None);

        assert_eq!(
            stats.messages_touched, 1,
            "only the whole-file read is compressed"
        );
        assert_eq!(
            r.messages[1].content.as_ref().unwrap().as_str().unwrap(),
            log
        );
        assert!(
            r.messages[2]
                .content
                .as_ref()
                .unwrap()
                .as_str()
                .unwrap()
                .len()
                < log.len()
        );
    }

    #[test]
    fn json_for_an_agent_that_cannot_recover_keeps_every_item() {
        // No recovery handle: cutting 96 of 100 search results would lose them for good.
        let items: Vec<String> = (0..100)
            .map(|i| format!(r#"{{"repo": "org/r{i}", "stars": {i}}}"#))
            .collect();
        let payload = format!(r#"{{"items": [{}]}}"#, items.join(", "));
        let mut r = req(vec![msg("tool", json!(payload.clone()))]);

        apply(&mut r, &on(), None);

        let sent = r.messages[0].content.as_ref().unwrap().as_str().unwrap();
        let before: Value = serde_json::from_str(&payload).unwrap();
        let after = nasiko_compress::expand_tables(sent).unwrap();
        assert_eq!(after, before, "an item went missing");
        assert!(
            sent.contains("table: 100 rows"),
            "the list should be tabled: {sent}"
        );
        assert!(
            sent.len() < payload.len() / 2,
            "keys written once: {} bytes",
            sent.len()
        );
    }

    #[test]
    fn json_with_a_recovery_handle_may_collapse_its_list() {
        let items: Vec<String> = (0..100)
            .map(|i| format!(r#"{{"repo": "org/r{i}", "stars": {i}}}"#))
            .collect();
        let payload = format!(r#"{{"items": [{}]}}"#, items.join(", "));
        let mut r = req(vec![msg("tool", json!(payload))]);

        let stats = apply(&mut r, &on(), recovery());

        let sent = r.messages[0].content.as_ref().unwrap().as_str().unwrap();
        assert!(sent.contains("items elided"), "{sent}");
        assert_eq!(stats.originals.len(), 1, "the dropped items are stored");
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
        // Spaced out, so the lossless rewrite (no recovery handle here) has whitespace to drop.
        let items: Vec<String> = (0..80)
            .map(|i| format!(r#"{{"id": {i}, "ok": true}}"#))
            .collect();
        let mut r = req(vec![
            msg("tool", json!(noisy_log())),
            msg(
                "tool",
                json!(format!(r#"{{"items": [{}]}}"#, items.join(", "))),
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
