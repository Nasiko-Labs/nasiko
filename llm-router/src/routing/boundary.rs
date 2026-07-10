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
        let phase = get(HEADER_PHASE).map(Phase::from_label).unwrap_or(Phase::Continue);
        let mode = get(HEADER_MODE).map(Mode::from_label).unwrap_or(Mode::FreeFlowing);
        Self { conv_id, phase, mode }
    }

    /// Signals that never fire the router — the safe default when there's no usable trace
    /// context or the flow is unknown (behaviour identical to before this layer existed).
    pub fn inert() -> Self {
        Self { conv_id: None, phase: Phase::Continue, mode: Mode::FreeFlowing }
    }

    /// Signals for a call inside a known flow: a **fireable boundary** with the flow's mode.
    /// Stickiness for continuation turns is provided by the decision cache (Level 2), so v1
    /// marks every in-flow call fireable rather than deriving cold_start/switch/continue
    /// from the agent call-chain (that finer phase is deferred — it only changes behaviour
    /// under a cache miss, and needs the flow-guard chain state).
    pub fn in_flow(flow_id: String, mode: Mode) -> Self {
        Self { conv_id: Some(flow_id), phase: Phase::Switch, mode }
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
}
