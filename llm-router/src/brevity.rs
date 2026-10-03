//! Output-token reduction at the egress seam: a brevity directive appended per request.
//!
//! Runs on the normalized [`ChatRequest`] IR at the same seam as [`crate::compress`], so one
//! implementation covers the OpenAI, Anthropic and Gemini inbound surfaces — and, because it
//! lands in the router rather than in an agent's own prompt, it reaches **user-uploaded agents
//! whose system prompts we never see**. That is the only way to reach them.
//!
//! # Why a new trailing `system` message, not an edit to the existing one
//!
//! Anthropic and Gemini hoist every `system` message into one top-level field joined with `\n`,
//! so for them the two forms are identical. OpenAI sends both verbatim. Only the new-message
//! form leaves author-written text byte-identical and stays trivially removable, so that is the
//! form both paths get.
//!
//! Carve-outs and the size floor follow the token-optimization design's IP-2 section.

use serde_json::Value;

use crate::config::GatewayConfig;
use crate::ir::ChatRequest;
use crate::resolver::ResolvedConfig;

/// Nasiko-authored. Not copied from Caveman's skill — the concepts are shared, the wording is
/// ours (§13).
///
/// Every clause is a carve-out as much as an instruction: the directive has to make the model
/// shorter *without* making it drop the things a shorter answer must still carry.
pub(crate) const DIRECTIVE: &str = "\
Answer concisely. Lead with the result, then only the reasoning needed to trust it. \
Drop restatements of the question, self-narration and closing summaries. \
Reproduce code, file paths, commands, identifiers, quoted errors and log lines exactly and in \
full — never abbreviate, elide or reformat them. \
Never shorten a security warning, a caveat about data loss, or a confirmation prompt for an \
irreversible action.";

/// Why the directive was skipped, for telemetry and tests.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Skipped {
    Disabled,
    /// Mid tool-loop. Terseness during tool use is where Caveman's 4.3M-token pathology lives
    /// (§3.3 item 4) — the model gets tersely wrong, retries, and spends more than it saved.
    ToolContinuation,
    /// External coding CLIs (Claude Code, Codex, Cursor) whose users can install the real
    /// Caveman skill themselves. Double-instructing buys the fixed overhead twice.
    CodingAgent,
    /// The directive costs ~60-120 tokens on *every* turn. Below the floor that overhead
    /// outweighs any plausible saving (§3.3 item 1).
    RequestTooSmall,
    /// The agent has token optimization switched off. The per-agent switch governs the whole
    /// stack, not just payload compression, so one control starts and stops every layer.
    AgentOptedOut,
    /// Compact tool schemas are active — the brevity directive fights `<<call>>` emission.
    CompactTools,
    /// Deliberately withheld, to keep a control arm.
    ///
    /// This layer's saving cannot be measured by subtraction — nobody knows what the model would
    /// have written without the directive — so the dashboard reports it from a factor. Withholding
    /// the directive from a small, deterministically-chosen slice of otherwise-eligible calls
    /// gives that factor a real control group inside the same traffic: same workloads, same
    /// models, same agents, concurrent. It is the one production comparison that attributes, and
    /// the only thing that turns a seeded assumption into a measurement.
    Holdout,
}

/// What IP-2 decided for one request, for `token_usage.metadata.brevity`.
///
/// Recorded whether or not it applied. A layer that leaves no trace when it declines is a layer
/// nobody can measure — three test rounds could not tell "off" apart from "nothing to do".
pub(crate) fn to_metadata(outcome: &Result<(), Skipped>, directive_bytes: usize) -> Value {
    match outcome {
        Ok(()) => serde_json::json!({
            "applied": true,
            "directive_bytes": directive_bytes,
        }),
        Err(reason) => serde_json::json!({
            "applied": false,
            "skipped": reason.as_label(),
        }),
    }
}

impl Skipped {
    /// Stable string for the metadata block — changing one of these changes a queryable value,
    /// so they are deliberately spelled out rather than derived from the variant name.
    pub(crate) fn as_label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ToolContinuation => "tool_continuation",
            Self::CodingAgent => "coding_agent",
            Self::RequestTooSmall => "request_too_small",
            Self::AgentOptedOut => "agent_opted_out",
            Self::CompactTools => "compact_tools",
            Self::Holdout => "holdout",
        }
    }
}

/// Append the directive unless a carve-out applies. `Ok(())` means the request was modified.
pub(crate) fn apply(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
    resolved: &ResolvedConfig,
    flow_id: Option<&str>,
) -> Result<(), Skipped> {
    if !cfg.brevity_enabled {
        return Err(Skipped::Disabled);
    }
    if !resolved.compress_enabled {
        return Err(Skipped::AgentOptedOut);
    }
    if resolved.is_coding_agent {
        return Err(Skipped::CodingAgent);
    }
    // `req.tools` alone is not the signal — an agent that merely *has* tools still benefits on a
    // plain turn. It is tools plus a trailing tool result, i.e. actually mid-loop, that hurts.
    if req.tools.is_some() && crate::routing::is_tool_continuation(&req.messages) {
        return Err(Skipped::ToolContinuation);
    }
    if estimated_bytes(req) < cfg.brevity_min_bytes {
        return Err(Skipped::RequestTooSmall);
    }
    // Last, so the control arm is drawn only from calls that would otherwise have been treated.
    // Checking it earlier would put requests below the size floor into the holdout, where the
    // directive would never have run anyway, and dilute the comparison with non-events.
    if in_holdout(flow_id, cfg.brevity_holdout_pct) {
        return Err(Skipped::Holdout);
    }

    req.messages.push(crate::ir::chat::Message {
        role: "system".into(),
        content: Some(serde_json::Value::String(DIRECTIVE.into())),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    });
    Ok(())
}

/// Whether this flow falls in the withheld slice.
///
/// Keyed on the flow id and hashed, so assignment is stable: a flow does not change arms between
/// its turns, which would mix treated and untreated turns inside one comparison. A call with no
/// flow id is never withheld — it cannot be attributed to a flow later either, so withholding it
/// would spend the cost of a control sample without buying one.
fn in_holdout(flow_id: Option<&str>, holdout_pct: u8) -> bool {
    if holdout_pct == 0 {
        return false;
    }
    let Some(flow_id) = flow_id else {
        return false;
    };
    // FNV-1a: stable across processes and releases, unlike `DefaultHasher`, whose output is not
    // guaranteed between Rust versions. An arm assignment that moves under a compiler upgrade
    // would silently re-randomise the experiment mid-flight.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in flow_id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    (hash % 100) < holdout_pct.min(100) as u64
}

/// Size of the transcript in bytes. A proxy for tokens, and deliberately a cheap one: this
/// decides whether to spend ~100 tokens, so it does not warrant a tokenizer.
///
/// Also the numerator the savings ledger calibrates chars-per-token with (`savings.rs`), called
/// once more after this seam has run. Shared rather than reimplemented so the two cannot disagree
/// about what counts as request text — a divergence there would bias every saved-token figure.
pub(crate) fn estimated_bytes(req: &ChatRequest) -> usize {
    req.messages
        .iter()
        .filter_map(|m| m.text())
        .map(|t| t.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::{FunctionDef, Message, ToolCall, ToolDef};
    use serde_json::{Value, json};

    fn cfg(enabled: bool) -> GatewayConfig {
        GatewayConfig {
            brevity_enabled: enabled,
            brevity_min_bytes: 0,
            ..Default::default()
        }
    }

    fn resolved(is_coding_agent: bool) -> ResolvedConfig {
        ResolvedConfig {
            is_coding_agent,
            compress_enabled: true,
            ..test_resolved()
        }
    }

    fn test_resolved() -> ResolvedConfig {
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
            compress_enabled: false,
        }
    }

    fn a_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "search".into(),
                description: None,
                parameters: None,
            },
            extra: Default::default(),
        }
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
            model: Some("gpt-4o-mini".into()),
            messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Default::default(),
        }
    }

    fn plain() -> ChatRequest {
        req(vec![
            msg("system", json!("You are a careful assistant.")),
            msg("user", json!("why did the deploy fail?")),
        ])
    }

    #[test]
    fn disabled_leaves_the_request_byte_identical() {
        let mut r = plain();
        let before = serde_json::to_string(&r).unwrap();

        assert_eq!(
            apply(&mut r, &cfg(false), &resolved(false), None),
            Err(Skipped::Disabled)
        );

        assert_eq!(serde_json::to_string(&r).unwrap(), before);
    }

    #[test]
    fn appends_a_new_trailing_system_message_and_edits_nothing() {
        let mut r = plain();
        let original: Vec<Message> = r.messages.clone();

        apply(&mut r, &cfg(true), &resolved(false), None).unwrap();

        assert_eq!(r.messages.len(), original.len() + 1);
        for (i, before) in original.iter().enumerate() {
            assert_eq!(
                serde_json::to_value(&r.messages[i]).unwrap(),
                serde_json::to_value(before).unwrap(),
                "message {i} was rewritten"
            );
        }
        let last = r.messages.last().unwrap();
        assert_eq!(last.role, "system");
        assert_eq!(last.content.as_ref().unwrap().as_str().unwrap(), DIRECTIVE);
    }

    #[test]
    fn every_outcome_is_recorded_even_when_the_layer_declines() {
        // The reason this exists: IP-1 writes metadata only when it acted, so its absence is
        // ambiguous between "switched off" and "nothing in scope". IP-2 always states which,
        // so a run can be audited from the row alone.
        let applied = to_metadata(&Ok(()), DIRECTIVE.len());
        assert_eq!(applied["applied"], serde_json::json!(true));
        assert_eq!(
            applied["directive_bytes"],
            serde_json::json!(DIRECTIVE.len())
        );

        for (reason, label) in [
            (Skipped::Disabled, "disabled"),
            (Skipped::ToolContinuation, "tool_continuation"),
            (Skipped::CodingAgent, "coding_agent"),
            (Skipped::RequestTooSmall, "request_too_small"),
            (Skipped::AgentOptedOut, "agent_opted_out"),
            (Skipped::CompactTools, "compact_tools"),
        ] {
            let m = to_metadata(&Err(reason), DIRECTIVE.len());
            assert_eq!(m["applied"], serde_json::json!(false));
            assert_eq!(
                m["skipped"],
                serde_json::json!(label),
                "label drifted for {reason:?}"
            );
        }
    }

    #[test]
    fn the_per_agent_switch_stops_this_layer_too() {
        // The switch is one control over the whole stack, not just payload compression: an agent
        // with token optimization off must not have its prompt rewritten either.
        let mut r = plain();
        let opted_out = ResolvedConfig {
            compress_enabled: false,
            ..test_resolved()
        };

        assert_eq!(
            apply(&mut r, &cfg(true), &opted_out, None),
            Err(Skipped::AgentOptedOut)
        );
        assert_eq!(r.messages.len(), 2, "the request must go out untouched");
    }

    #[test]
    fn skips_a_coding_agent() {
        let mut r = plain();
        assert_eq!(
            apply(&mut r, &cfg(true), &resolved(true), None),
            Err(Skipped::CodingAgent)
        );
        assert_eq!(r.messages.len(), 2);
    }

    #[test]
    fn skips_a_tool_continuation_turn() {
        let mut r = plain();
        r.tools = Some(vec![a_tool()]);
        r.messages.push(Message {
            tool_call_id: Some("call_1".into()),
            ..msg("tool", json!("{\"ok\":true}"))
        });

        assert_eq!(
            apply(&mut r, &cfg(true), &resolved(false), None),
            Err(Skipped::ToolContinuation)
        );
        assert!(r.messages.last().unwrap().role == "tool");
    }

    #[test]
    fn tools_without_a_trailing_tool_result_still_get_the_directive() {
        // Having tools is not mid-loop. Only a trailing tool result is.
        let mut r = plain();
        r.tools = Some(vec![a_tool()]);

        apply(&mut r, &cfg(true), &resolved(false), None).unwrap();

        assert_eq!(r.messages.last().unwrap().role, "system");
    }

    #[test]
    fn assistant_tool_calls_are_not_a_continuation() {
        let mut r = plain();
        r.tools = Some(vec![a_tool()]);
        let mut m = msg("assistant", json!(""));
        m.tool_calls = Some(vec![ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            function: crate::ir::chat::FunctionCall {
                name: "search".into(),
                arguments: "{}".into(),
            },
            extra: Default::default(),
        }]);
        r.messages.push(m);

        apply(&mut r, &cfg(true), &resolved(false), None).unwrap();

        assert_eq!(r.messages.last().unwrap().role, "system");
    }

    #[test]
    fn skips_a_request_below_the_size_floor() {
        let mut r = plain();
        let c = GatewayConfig {
            brevity_min_bytes: 1_000_000,
            ..cfg(true)
        };

        assert_eq!(
            apply(&mut r, &c, &resolved(false), None),
            Err(Skipped::RequestTooSmall)
        );
        assert_eq!(r.messages.len(), 2);
    }

    #[test]
    fn the_directive_protects_what_a_shorter_answer_must_still_carry() {
        // Guards against someone "tightening" the wording into a pure terseness instruction —
        // the carve-outs are the difference between IP-2 and the §3.3 pathology.
        for required in [
            "exactly and in full",
            "security warning",
            "irreversible action",
        ] {
            assert!(
                DIRECTIVE.contains(required),
                "directive dropped: {required}"
            );
        }
    }
    // ── holdout ──────────────────────────────────────────────────────────────

    /// A flow that lands in the withheld slice at a 100% rate, for the ordering tests below.
    fn holdout_cfg(pct: u8) -> GatewayConfig {
        GatewayConfig {
            brevity_holdout_pct: pct,
            ..cfg(true)
        }
    }

    #[test]
    fn a_withheld_call_goes_out_untouched_and_says_why() {
        let mut r = plain();

        assert_eq!(
            apply(&mut r, &holdout_cfg(100), &resolved(false), Some("flow-1")),
            Err(Skipped::Holdout)
        );

        assert_eq!(
            r.messages.len(),
            2,
            "the control arm must be a real control"
        );
        assert_eq!(Skipped::Holdout.as_label(), "holdout");
    }

    #[test]
    fn arm_assignment_is_stable_for_a_given_flow() {
        // A flow that changed arms between turns would mix treated and untreated turns inside one
        // comparison, which is worse than having no control at all.
        for flow in ["flow-a", "flow-b", "0af7651916cd43dd8448eb211c80319c"] {
            let first = in_holdout(Some(flow), 50);
            for _ in 0..100 {
                assert_eq!(in_holdout(Some(flow), 50), first, "{flow} changed arms");
            }
        }
    }

    #[test]
    fn a_zero_percent_holdout_withholds_nothing() {
        // The off switch has to be exact: with it off, this layer must behave as though the
        // holdout had never been written.
        let mut r = plain();
        apply(&mut r, &holdout_cfg(0), &resolved(false), Some("flow-1")).unwrap();
        assert_eq!(r.messages.last().unwrap().role, "system");
    }

    #[test]
    fn a_call_without_a_flow_id_is_never_withheld() {
        // It could not be attributed to an arm afterwards either, so withholding it would spend
        // the cost of a control sample without buying one.
        let mut r = plain();
        apply(&mut r, &holdout_cfg(100), &resolved(false), None).unwrap();
        assert_eq!(r.messages.last().unwrap().role, "system");
    }

    #[test]
    fn the_holdout_is_drawn_only_from_calls_that_would_have_been_treated() {
        // Order matters: a request below the size floor would never have had the directive, so
        // counting it as a control sample would dilute the comparison with non-events.
        let mut r = plain();
        let c = GatewayConfig {
            brevity_min_bytes: 1_000_000,
            brevity_holdout_pct: 100,
            ..cfg(true)
        };

        assert_eq!(
            apply(&mut r, &c, &resolved(false), Some("flow-1")),
            Err(Skipped::RequestTooSmall),
            "the size floor must be decided before the holdout"
        );
    }

    #[test]
    fn the_withheld_share_is_near_the_configured_rate() {
        let withheld = (0..10_000)
            .filter(|i| in_holdout(Some(&format!("flow-{i}")), 5))
            .count();
        // Hashing is not a perfect splitter; the band is wide enough not to flake and tight enough
        // to catch a bucket that is systematically wrong.
        assert!(
            (300..=700).contains(&withheld),
            "expected ~500 of 10000 withheld at 5%, got {withheld}"
        );
    }
}
