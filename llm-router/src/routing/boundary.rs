//! Boundary signals — the orchestrator's per-request tags that tell the model router
//! *when* it is safe to (re)classify.
//!
//! The orchestrator tags every request before it reaches the resolver:
//! - `phase` (`cold_start` | `switch` | `continue`) — derived from whether a conversation
//!   exists and whether the last agent matches the current one.
//! - `mode` (`free_flowing` | `pinned_flow`) — from the conversation config.
//! - `conv_id` — the conversation id used as the decision-cache key.
//!
//! **The hard invariant:** intra-agent tool-loop turns are always `phase = continue`, so
//! the classifier physically cannot fire mid-loop and cannot corrupt tool-call state.
//!
//! **Primary source (S5): derived at the gateway.** The signals are not set by the
//! (opaque) agent. Instead the gateway reads the W3C `traceparent` the agent forwards,
//! maps its trace id to the platform's flow (conversation) via [`parse_flow_id`] + a
//! `flows` lookup, and builds the signals from that trusted state — see
//! [`BoundarySignals::in_flow`] / [`BoundarySignals::inert`]. No trace context (or an
//! unknown flow) ⇒ `inert` ⇒ the router never fires and behaviour is identical to before.
//!
//! **Explicit alternative.** [`BoundarySignals::from_headers`] parses the `X-Nasiko-*`
//! headers directly; kept for a first-party caller that wants to set signals itself,
//! rather than have the gateway derive them.

use axum::http::HeaderMap;

/// W3C trace context header the agent forwards; the gateway derives the flow from it.
pub const TRACEPARENT_HEADER: &str = "traceparent";

/// Header carrying the conversation id (decision-cache key).
pub const HEADER_CONV_ID: &str = "x-nasiko-conv-id";
/// Header carrying the boundary phase.
pub const HEADER_PHASE: &str = "x-nasiko-phase";
/// Header carrying the conversation flow mode.
pub const HEADER_MODE: &str = "x-nasiko-mode";

/// Deterministic `conv_id` for one turn of a coding-agent session: a hash of `(agent_id,
/// turn_ordinal, latest_user_text)`, not the raw prompt text itself, since `conv_id` rides
/// through logs and the (possibly Redis-backed) decision cache. `DefaultHasher::new()`
/// starts from fixed keys, so this is stable across router instances and process restarts
/// running the same binary — unlike `HashMap`'s per-process-randomized `RandomState`.
/// `turn_ordinal` disambiguates two turns that happen to repeat the same prompt text.
fn coding_agent_conv_id(agent_id: &str, turn_ordinal: usize, latest_user_text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    agent_id.hash(&mut hasher);
    turn_ordinal.hash(&mut hasher);
    latest_user_text.hash(&mut hasher);
    format!("coding-agent-{:016x}", hasher.finish())
}

/// Extract the trace id — our flow id / `conv_id` — from a W3C `traceparent`
/// (`{version}-{trace_id}-{span_id}-{flags}`). Returns the 32-hex-char trace id, or `None`
/// if malformed or all-zero (the W3C "invalid" trace id). Mirrors the flow crate's parser
/// without taking a dependency on it (keeps the router promotable to a standalone binary).
pub fn parse_flow_id(traceparent: &str) -> Option<String> {
    let parts: Vec<&str> = traceparent.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let trace_id = parts[1];
    let valid = trace_id.len() == 32
        && trace_id.bytes().all(|b| b.is_ascii_hexdigit())
        && trace_id.bytes().any(|b| b != b'0');
    valid.then(|| trace_id.to_ascii_lowercase())
}

/// Where a request sits relative to conversation/agent boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// First turn of a brand-new conversation.
    ColdStart,
    /// A different agent has taken over within an existing conversation.
    Switch,
    /// An intra-agent continuation (including every tool-loop turn) — model stays sticky.
    Continue,
}

impl Phase {
    /// Parse the `X-Nasiko-Phase` header value (case-insensitive). Unknown/missing ⇒
    /// [`Phase::Continue`] — the safe default that never triggers classification.
    pub fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "cold_start" => Phase::ColdStart,
            "switch" => Phase::Switch,
            _ => Phase::Continue,
        }
    }
}

/// The conversation's flow mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Model may be (re)selected at boundaries.
    FreeFlowing,
    /// The flow is pinned end-to-end; the router must not re-select.
    PinnedFlow,
}

impl Mode {
    /// Parse the `X-Nasiko-Mode` header value (case-insensitive). Unknown/missing ⇒
    /// [`Mode::FreeFlowing`] (the common orchestrator case). Note that firing still
    /// additionally requires an explicit boundary `phase`, so this default alone never
    /// causes classification.
    pub fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "pinned_flow" => Mode::PinnedFlow,
            _ => Mode::FreeFlowing,
        }
    }
}

/// The per-request boundary tags, parsed from headers.
#[derive(Debug, Clone)]
pub struct BoundarySignals {
    /// Conversation id; `None` when the request isn't part of an orchestrated conversation.
    pub conv_id: Option<String>,
    pub phase: Phase,
    pub mode: Mode,
}

impl BoundarySignals {
    /// Extract signals from request headers. Missing headers fall back to the safe
    /// defaults (`conv_id = None`, `phase = Continue`, `mode = FreeFlowing`) so a request
    /// with no orchestrator tags behaves exactly as it did before the router existed.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        let conv_id = get(HEADER_CONV_ID)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let phase = get(HEADER_PHASE)
            .map(Phase::from_label)
            .unwrap_or(Phase::Continue);
        let mode = get(HEADER_MODE)
            .map(Mode::from_label)
            .unwrap_or(Mode::FreeFlowing);
        Self {
            conv_id,
            phase,
            mode,
        }
    }

    /// Signals that never fire the router — the safe default when there's no usable trace
    /// context or the flow is unknown (behaviour identical to before this layer existed).
    pub fn inert() -> Self {
        Self {
            conv_id: None,
            phase: Phase::Continue,
            mode: Mode::FreeFlowing,
        }
    }

    /// Signals for a call inside a known flow: a **fireable boundary** with the flow's mode.
    /// Stickiness for continuation turns is provided by the decision cache (Level 2), so v1
    /// marks every in-flow call fireable rather than deriving cold_start/switch/continue
    /// from the agent call-chain (that finer phase is deferred — it only changes behaviour
    /// under a cache miss, and needs the flow-guard chain state).
    pub fn in_flow(flow_id: String, mode: Mode) -> Self {
        Self {
            conv_id: Some(flow_id),
            phase: Phase::Switch,
            mode,
        }
    }

    /// Signals for a coding-agent CLI session (Claude Code, Codex, OpenCode, Cursor).
    ///
    /// These sessions are never dispatched through the orchestrator, so they never get a
    /// `traceparent` tied to a real `flows` row — the gateway's flow-lookup derivation is a
    /// permanent dead end for them, not a transient miss. This is the coding-agent
    /// equivalent of [`in_flow`], built from the transcript instead of a flow row.
    ///
    /// `conv_id` anchors on `(turn_ordinal, latest_user_text)` — the *current* top-level
    /// prompt, not the whole session. That matters because the decision cache (Level 2)
    /// checks `conv_id` unconditionally, before phase is even considered: a `conv_id` that
    /// stayed constant for the whole session would let turn 1's classification decide every
    /// later turn too, since every later turn would just be a cache hit on that same key.
    /// Anchoring on the current turn instead means a genuinely new prompt gets a fresh
    /// `conv_id` (cache miss → re-classify), while every tool-loop turn that follows it
    /// keeps referring to the same `(turn_ordinal, latest_user_text)` — since no new `user`
    /// message has appeared yet — so it stays a cache hit on *that* turn's decision.
    ///
    /// `is_tool_continuation` marks a transcript whose last turn is a tool result: that
    /// keeps a mid-tool-loop turn `Phase::Continue` (sticky), mirroring the hard invariant
    /// that intra-agent tool-loop turns never reclassify. Anything else — including the
    /// first turn — is `Phase::Switch`, a fireable boundary, so the classifier gets to pick
    /// a model for this prompt instead of always falling through to the agent's pinned
    /// `llm_config`.
    pub fn for_coding_agent(
        agent_id: &str,
        turn_ordinal: usize,
        latest_user_text: Option<&str>,
        is_tool_continuation: bool,
    ) -> Self {
        Self {
            conv_id: Some(coding_agent_conv_id(
                agent_id,
                turn_ordinal,
                latest_user_text.unwrap_or_default(),
            )),
            phase: if is_tool_continuation {
                Phase::Continue
            } else {
                Phase::Switch
            },
            mode: Mode::FreeFlowing,
        }
    }

    /// Whether this request sits at a boundary where re-selecting the model is safe:
    /// a `switch` or `cold_start` **in free-flowing mode**. Tool-loop `continue` turns
    /// and any `pinned_flow` conversation return `false`.
    ///
    /// Note: the precedence table (§ "Resolver precedence") lists Level 3 as
    /// `switch && free_flowing`; per "The Big Idea", `cold_start` is the other safe
    /// fire moment, so it is included here. Flagged for confirmation.
    pub fn is_fireable_boundary(&self) -> bool {
        matches!(self.phase, Phase::Switch | Phase::ColdStart) && self.mode == Mode::FreeFlowing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn absent_headers_are_safe_defaults() {
        let s = BoundarySignals::from_headers(&HeaderMap::new());
        assert!(s.conv_id.is_none());
        assert_eq!(s.phase, Phase::Continue);
        assert_eq!(s.mode, Mode::FreeFlowing);
        assert!(!s.is_fireable_boundary(), "no explicit phase ⇒ never fires");
    }

    #[test]
    fn parses_all_three_and_is_case_insensitive() {
        let s = BoundarySignals::from_headers(&headers(&[
            (HEADER_CONV_ID, "conv-1"),
            (HEADER_PHASE, "SwItCh"),
            (HEADER_MODE, "FREE_FLOWING"),
        ]));
        assert_eq!(s.conv_id.as_deref(), Some("conv-1"));
        assert_eq!(s.phase, Phase::Switch);
        assert_eq!(s.mode, Mode::FreeFlowing);
        assert!(s.is_fireable_boundary());
    }

    #[test]
    fn blank_conv_id_is_none() {
        let s = BoundarySignals::from_headers(&headers(&[(HEADER_CONV_ID, "   ")]));
        assert!(s.conv_id.is_none());
    }

    #[test]
    fn unknown_labels_fall_back_to_safe_defaults() {
        let s = BoundarySignals::from_headers(&headers(&[
            (HEADER_PHASE, "banana"),
            (HEADER_MODE, "banana"),
        ]));
        assert_eq!(s.phase, Phase::Continue);
        assert_eq!(s.mode, Mode::FreeFlowing);
    }

    #[test]
    fn pinned_flow_never_fires_even_at_a_boundary() {
        let s = BoundarySignals::from_headers(&headers(&[
            (HEADER_PHASE, "switch"),
            (HEADER_MODE, "pinned_flow"),
        ]));
        assert!(!s.is_fireable_boundary());
    }

    #[test]
    fn cold_start_in_free_flowing_fires() {
        let s = BoundarySignals::from_headers(&headers(&[(HEADER_PHASE, "cold_start")]));
        assert!(s.is_fireable_boundary());
    }

    #[test]
    fn continue_never_fires() {
        let s = BoundarySignals::from_headers(&headers(&[(HEADER_PHASE, "continue")]));
        assert!(!s.is_fireable_boundary());
    }

    #[test]
    fn parses_flow_id_from_valid_traceparent() {
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert_eq!(
            parse_flow_id(tp).as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
    }

    #[test]
    fn rejects_malformed_or_invalid_traceparent() {
        assert!(parse_flow_id("garbage").is_none());
        assert!(parse_flow_id("00-tooshort-span-01").is_none());
        // all-zero trace id is the W3C "invalid" sentinel
        assert!(parse_flow_id("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none());
        // non-hex
        assert!(parse_flow_id("00-zzf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_none());
    }

    #[test]
    fn in_flow_is_fireable_and_inert_is_not() {
        let f = BoundarySignals::in_flow("flow-1".into(), Mode::FreeFlowing);
        assert_eq!(f.conv_id.as_deref(), Some("flow-1"));
        assert!(f.is_fireable_boundary());

        let i = BoundarySignals::inert();
        assert!(i.conv_id.is_none());
        assert!(!i.is_fireable_boundary());
    }

    #[test]
    fn in_flow_pinned_mode_does_not_fire() {
        let f = BoundarySignals::in_flow("flow-1".into(), Mode::PinnedFlow);
        assert!(!f.is_fireable_boundary());
    }

    #[test]
    fn coding_agent_new_prompt_is_a_fireable_boundary() {
        let s = BoundarySignals::for_coding_agent("agent-1", 1, Some("fix the bug"), false);
        assert!(s.conv_id.is_some());
        assert_eq!(s.phase, Phase::Switch);
        assert_eq!(s.mode, Mode::FreeFlowing);
        assert!(s.is_fireable_boundary());
    }

    #[test]
    fn coding_agent_tool_continuation_stays_sticky() {
        let s = BoundarySignals::for_coding_agent("agent-1", 1, Some("fix the bug"), true);
        assert_eq!(s.phase, Phase::Continue);
        assert!(!s.is_fireable_boundary());
    }

    #[test]
    fn coding_agent_conv_id_is_stable_across_one_turns_tool_loop() {
        // Same turn ordinal + same latest user text (unchanged while a tool loop runs)
        // ⇒ same conv_id, so the Level-2 decision cache stays sticky mid tool-loop.
        let cold_start =
            BoundarySignals::for_coding_agent("agent-1", 1, Some("fix the bug"), false);
        let tool_loop_turn =
            BoundarySignals::for_coding_agent("agent-1", 1, Some("fix the bug"), true);
        assert_eq!(cold_start.conv_id, tool_loop_turn.conv_id);
    }

    #[test]
    fn coding_agent_conv_id_changes_on_the_next_prompt_in_the_same_session() {
        // Regression: conv_id must NOT stay constant for the whole session. The decision
        // cache (Level 2) is checked unconditionally before phase is considered, so a
        // session-wide conv_id would make turn 1's classification decide every later turn
        // too — the classifier would never re-fire for a new prompt. Anchoring on the
        // current turn's ordinal + text instead means the second prompt in the same
        // session gets a fresh conv_id (cache miss ⇒ the classifier runs again for it).
        let turn1 = BoundarySignals::for_coding_agent("agent-1", 1, Some("fix the bug"), false);
        let turn2 = BoundarySignals::for_coding_agent("agent-1", 2, Some("now add a test"), false);
        assert_ne!(turn1.conv_id, turn2.conv_id);
    }

    #[test]
    fn coding_agent_conv_id_differs_across_agents_and_conversations() {
        let base = BoundarySignals::for_coding_agent("agent-1", 1, Some("fix the bug"), false);
        let other_agent =
            BoundarySignals::for_coding_agent("agent-2", 1, Some("fix the bug"), false);
        let other_conversation =
            BoundarySignals::for_coding_agent("agent-1", 1, Some("add a feature"), false);
        assert_ne!(base.conv_id, other_agent.conv_id);
        assert_ne!(base.conv_id, other_conversation.conv_id);
    }

    #[test]
    fn coding_agent_missing_latest_user_text_still_produces_a_conv_id() {
        let s = BoundarySignals::for_coding_agent("agent-1", 1, None, false);
        assert!(s.conv_id.is_some());
    }
}
