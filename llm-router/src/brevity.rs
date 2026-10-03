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
        }
    }
}

/// Append the directive unless a carve-out applies. `Ok(())` means the request was modified.
pub(crate) fn apply(
    req: &mut ChatRequest,
    cfg: &GatewayConfig,
    resolved: &ResolvedConfig,
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

/// Size of the transcript in bytes. A proxy for tokens, and deliberately a cheap one: this
/// decides whether to spend ~100 tokens, so it does not warrant a tokenizer.
fn estimated_bytes(req: &ChatRequest) -> usize {
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
            apply(&mut r, &cfg(false), &resolved(false)),
            Err(Skipped::Disabled)
        );

        assert_eq!(serde_json::to_string(&r).unwrap(), before);
    }

    #[test]
    fn appends_a_new_trailing_system_message_and_edits_nothing() {
        let mut r = plain();
        let original: Vec<Message> = r.messages.clone();

        apply(&mut r, &cfg(true), &resolved(false)).unwrap();

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
            apply(&mut r, &cfg(true), &opted_out),
            Err(Skipped::AgentOptedOut)
        );
        assert_eq!(r.messages.len(), 2, "the request must go out untouched");
    }

    #[test]
    fn skips_a_coding_agent() {
        let mut r = plain();
        assert_eq!(
            apply(&mut r, &cfg(true), &resolved(true)),
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
            apply(&mut r, &cfg(true), &resolved(false)),
            Err(Skipped::ToolContinuation)
        );
        assert!(r.messages.last().unwrap().role == "tool");
    }

    #[test]
    fn tools_without_a_trailing_tool_result_still_get_the_directive() {
        // Having tools is not mid-loop. Only a trailing tool result is.
        let mut r = plain();
        r.tools = Some(vec![a_tool()]);

        apply(&mut r, &cfg(true), &resolved(false)).unwrap();

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

        apply(&mut r, &cfg(true), &resolved(false)).unwrap();

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
            apply(&mut r, &c, &resolved(false)),
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
}
