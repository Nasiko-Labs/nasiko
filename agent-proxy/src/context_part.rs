//! Appending admin-injected context (L1A knowledge and org policy/L0 today, at their independent
//! call sites — the edition's forwarding hop and `oss/server/src/router/a2a_dispatch.rs` /
//! `oss/server/src/agent_proxy.rs` respectively; a future L2 belief is the next caller) to an
//! outbound A2A message as its **own part**, rather than splicing it into the text part an agent
//! may parse for the platform's own history-glue marker.
//!
//! `session_history.rs`'s glue convention (`"{history}\n\nCurrent message: {query}"`) is a
//! documented, real pattern this codebase's own `oss/llm-router::latest_user_query` implements:
//! take only the text after `"\n\nCurrent message: "`, treat everything ahead of it as
//! already-handled history to discard. Any agent that follows that same documented convention —
//! which is exactly what it's published for — silently discards whatever text was prepended
//! ahead of that marker. Injected context (L1A facts, org policy, a future L2 belief) is not
//! history; it's meant to inform how the agent answers the *current* turn, every time. Splicing
//! it into the same field the marker convention discards is a category error, not a formatting
//! detail, and it isn't fixable by choosing a different position within that one field: whatever
//! goes ahead of the marker is lost, and whatever goes after it becomes part of what a
//! marker-aware agent treats as the user's literal instruction.
//!
//! The fix is to stop sharing the field. `parts` is already an array for exactly this reason —
//! this platform already sends multi-part messages on every call that carries a file attachment
//! (`file_parts`, threaded through `react-agent`'s `extra_parts`) — so appending a second part is
//! not a new capability, just a new use of one already proven in production. A marker-aware agent
//! still correctly extracts only the query from the one part that has the marker in it (arguably
//! *more* correctly than today, since that part no longer contains anything but the conversation);
//! it gets the injected context too only if it does what the A2A spec already expects of it —
//! consider every part of the message — which is a real protocol-level expectation, not an
//! internal convention specific to this platform.
//!
//! Appended, never inserted at the front: a caller that (incorrectly, but not unheard of for a
//! minimal SDK example) only reads `parts[0]` still gets the unmodified query in that position,
//! same as before this existed — worse for such a caller would be prepending and having it read
//! the injected block instead of the user's message.

use serde_json::Value;

/// Whether an appended part needs an explicit `"kind": "text"` discriminator — this platform's
/// real `a2a` (a2a-lf) `Part` type has **no such field at all**: its `Serialize`/`Deserialize`
/// impls (`a2a::types::Part`) use field-presence union semantics, exactly like
/// `oss/react-agent/src/a2a.rs`'s private `Dialect` enum already accounts for on the
/// request-*building* side (`Dialect::Proto` → untagged `{"text": ...}`; `Dialect::JsonRpc` →
/// tagged `{"kind": "text", ...}`, for the small set of agents — `preferredTransport: JSONRPC`,
/// e.g. the Python a2a-sdk — that expect that shape instead). An appended context part has to
/// match whichever shape the *rest of this specific message* already uses, not a fixed choice —
/// the caller determines that with [`tagging_for_method`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartTagging {
    /// `{"text": "..."}` — this platform's own real `Part` schema, and what every one of its own
    /// `oss/agents/*` example agents (all a2a-lf-based) actually parses.
    Untagged,
    /// `{"kind": "text", "text": "..."}` — for agents on the JSON-RPC-standard `message/send`
    /// dialect that expect an explicit discriminator.
    Tagged,
}

/// Maps a request's own JSON-RPC `method` to the [`PartTagging`] its parts already use — mirrors
/// `oss/react-agent/src/a2a.rs`'s `Dialect::method`/`Dialect::text_part` mapping exactly, so the
/// two can't drift: `"message/send"`/`"message/stream"` (the JSON-RPC-standard names) are tagged;
/// `"SendMessage"`/`"SendStreamingMessage"` (the proto-style names this platform's own builders —
/// `nasiko_types::a2a::build_request` and the MAF executor's outbound call — always use) are not.
/// Defaults to [`PartTagging::Untagged`] for anything else, since that is this platform's own real
/// wire shape and the safer assumption when a method name isn't recognized.
pub fn tagging_for_method(method: &str) -> PartTagging {
    match method {
        "message/send" | "message/stream" => PartTagging::Tagged,
        _ => PartTagging::Untagged,
    }
}

/// Append `text` as a new part onto `message`'s `parts` array, shaped per `tagging`.
///
/// `message` is the A2A `params.message` object (the value at `/params/message` in a
/// `message/send`/`message/stream` JSON-RPC body) — not the whole request, and not one specific
/// part. Returns `None`, leaving `message` untouched, when there's no `parts` array to append to;
/// callers already treat a `None` here as "nothing to inject" and relay the original body
/// unmodified, so this composes with the existing fail-open posture without any special casing.
pub fn append_context_part(message: &mut Value, text: &str, tagging: PartTagging) -> Option<()> {
    let parts = message.get_mut("parts")?.as_array_mut()?;
    let part = match tagging {
        PartTagging::Untagged => serde_json::json!({ "text": text }),
        PartTagging::Tagged => serde_json::json!({ "kind": "text", "text": text }),
    };
    parts.push(part);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_names_map_to_the_tagging_dialect_text_part_already_uses() {
        // Mirrors `oss/react-agent/src/a2a.rs`'s `Dialect::method`/`Dialect::text_part` mapping —
        // a regression here would silently desync the two.
        assert_eq!(tagging_for_method("message/send"), PartTagging::Tagged);
        assert_eq!(tagging_for_method("message/stream"), PartTagging::Tagged);
        assert_eq!(tagging_for_method("SendMessage"), PartTagging::Untagged);
        assert_eq!(
            tagging_for_method("SendStreamingMessage"),
            PartTagging::Untagged
        );
        // Unrecognized methods default to this platform's own real wire shape.
        assert_eq!(tagging_for_method("whatever"), PartTagging::Untagged);
    }

    #[test]
    fn untagged_matches_the_real_a2a_lf_part_schema() {
        // a2a-lf's own `Part` type has no `"kind"` field at all (field-presence union
        // serialization) — this is the shape this platform's own agents actually parse, and the
        // one every call site built on `nasiko_types::a2a::build_request` (proto-style method
        // names) must use.
        let mut message = serde_json::json!({
            "parts": [{"text": "Current message: hello"}],
            "contextId": "ctx-1"
        });
        append_context_part(
            &mut message,
            "## Known facts about this agent\nfact one",
            PartTagging::Untagged,
        )
        .unwrap();

        let parts = message["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        // The original part is untouched, in place, at index 0 — a caller that only reads
        // `parts[0]` gets exactly what it got before this function was ever called.
        assert_eq!(parts[0]["text"], "Current message: hello");
        assert_eq!(
            parts[1]["text"],
            "## Known facts about this agent\nfact one"
        );
        assert!(
            parts[1].get("kind").is_none(),
            "an untagged part must not carry a `kind` field a real a2a-lf agent would reject: {parts:?}"
        );
    }

    #[test]
    fn tagged_carries_the_explicit_kind_discriminator() {
        let mut message = serde_json::json!({
            "parts": [{"kind": "text", "text": "Current message: hello"}]
        });
        append_context_part(
            &mut message,
            "## Known facts about this agent\nfact one",
            PartTagging::Tagged,
        )
        .unwrap();

        let parts = message["parts"].as_array().unwrap();
        assert_eq!(parts[1]["kind"], "text");
        assert_eq!(
            parts[1]["text"],
            "## Known facts about this agent\nfact one"
        );
    }

    #[test]
    fn appended_part_never_contains_the_glue_marker() {
        // The whole point: a marker-aware extractor (`text.find("\n\nCurrent message: ")`,
        // then take the tail) must still find exactly one marker, in the original part only.
        let mut message = serde_json::json!({
            "parts": [{"text": "Current message: hello"}]
        });
        append_context_part(
            &mut message,
            "## Known facts about this agent\nfact one",
            PartTagging::Untagged,
        )
        .unwrap();
        let injected_text = message["parts"][1]["text"].as_str().unwrap();
        assert!(!injected_text.contains("Current message: "));
    }

    #[test]
    fn no_parts_array_is_a_clean_miss() {
        let mut message = serde_json::json!({"contextId": "ctx-1"});
        assert!(append_context_part(&mut message, "facts", PartTagging::Untagged).is_none());
        // Untouched, not partially mutated.
        assert_eq!(message, serde_json::json!({"contextId": "ctx-1"}));
    }

    #[test]
    fn file_only_message_still_gets_context_appended() {
        // A message with no text part at all (a file-only send) still has a `parts` array, and
        // still gets context appended — there is no text part to find-and-replace, only a new
        // part to append, so this needs no special-casing at the call site.
        let mut message = serde_json::json!({
            "parts": [{"filename": "report.pdf", "media_type": "application/pdf", "data": "..."}]
        });
        append_context_part(
            &mut message,
            "## Organization Policy\n\nrule one",
            PartTagging::Untagged,
        )
        .unwrap();

        let parts = message["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["text"], "## Organization Policy\n\nrule one");
    }

    #[test]
    fn multiple_appends_stack_in_call_order() {
        // The shape org policy (in-process) and L1A (injection proxy) take on one forward: each
        // caller appends its own part, independently, with no knowledge of whether another has
        // already run.
        let mut message = serde_json::json!({
            "parts": [{"text": "Current message: hello"}]
        });
        append_context_part(
            &mut message,
            "## Organization policy\nrule one",
            PartTagging::Untagged,
        )
        .unwrap();
        append_context_part(
            &mut message,
            "## Known facts about this agent\nfact one",
            PartTagging::Untagged,
        )
        .unwrap();

        let parts = message["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["text"], "Current message: hello");
        assert_eq!(parts[1]["text"], "## Organization policy\nrule one");
        assert_eq!(
            parts[2]["text"],
            "## Known facts about this agent\nfact one"
        );
    }
}
