//! Classifier context — the bounded, role-labelled slice of a transcript that accompanies
//! the latest user query into the classifier.
//!
//! **Included:** the text of up to [`MAX_TURNS`] `user`/`assistant` turns immediately
//! before the latest user turn, newest last, each labelled with its role and cut to
//! [`MAX_TURN_CHARS`]; the whole block is cut to [`MAX_TOTAL_CHARS`] (oldest turns dropped
//! first, so what survives is always the most recent material).
//!
//! **Excluded, deliberately:** `system`/`developer` prompts (hidden instructions, often
//! proprietary); `tool` results and assistant `tool_calls` (large, noisy, and the place
//! credentials and raw data show up); image/audio/file parts (text parts only — a
//! multimodal turn contributes only its text, or nothing); the latest user turn itself
//! (that is the `query`). The packed-history form the A2A dispatcher produces
//! (`…\n\nCurrent message: …`) is handled upstream by [`super::latest_user_query`]; its
//! prefix is **not** mined for context, so a packed transcript yields no context rather
//! than a mis-parsed one.
//!
//! Truncation is deterministic and char-boundary safe. Both wire formats (chat IR messages
//! and Responses-API `input` items) reduce to `(role, text)` pairs and share this one
//! function, so the eval's explicit `context` field and the router's derived context go
//! through identical normalization downstream.

use super::classifier_service::cap_chars;
use crate::ir::Message;

/// Most recent prior turns considered.
pub const MAX_TURNS: usize = 4;
/// Longest single turn kept, in chars.
pub const MAX_TURN_CHARS: usize = 600;
/// Longest context block, in chars (the service caps again at its own limit).
pub const MAX_TOTAL_CHARS: usize = 2000;

/// Build classifier context from `(role, text)` turns in transcript order. The last `user`
/// turn is treated as the query and skipped; everything after it is ignored.
pub fn classifier_context<'a>(
    turns: impl IntoIterator<Item = (&'a str, String)>,
) -> Option<String> {
    let turns: Vec<(&str, String)> = turns.into_iter().collect();
    let last_user = turns.iter().rposition(|(role, _)| *role == "user")?;
    let mut kept: Vec<String> = Vec::new();
    for (role, text) in turns[..last_user].iter().rev() {
        if !matches!(*role, "user" | "assistant") {
            continue;
        }
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let mut truncated = false;
        let body = cap_chars(text, MAX_TURN_CHARS, &mut truncated);
        kept.push(format!(
            "{role}: {body}{}",
            if truncated { "…" } else { "" }
        ));
        if kept.len() == MAX_TURNS {
            break;
        }
    }
    if kept.is_empty() {
        return None;
    }
    kept.reverse();
    // Drop oldest turns until the block fits; never cut a turn in the middle at this level.
    while kept.len() > 1
        && kept.iter().map(|k| k.chars().count() + 1).sum::<usize>() > MAX_TOTAL_CHARS
    {
        kept.remove(0);
    }
    let mut truncated = false;
    let joined = cap_chars(&kept.join("\n"), MAX_TOTAL_CHARS, &mut truncated);
    Some(joined)
}

/// Chat-IR adapter: text of every message, by role. Tool results, tool calls and non-text
/// parts contribute nothing (see the module docs).
pub fn context_from_messages(messages: &[Message]) -> Option<String> {
    classifier_context(messages.iter().filter_map(|m| {
        if m.role == "tool" || m.tool_calls.as_ref().is_some_and(|t| !t.is_empty()) {
            return None;
        }
        m.text().map(|t| (m.role.as_str(), t))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value, json};

    fn msg(role: &str, content: Value) -> Message {
        Message {
            role: role.into(),
            content: Some(content),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        }
    }
    fn text(role: &str, s: &str) -> Message {
        msg(role, Value::String(s.into()))
    }

    #[test]
    fn takes_recent_user_and_assistant_turns_before_the_query_and_labels_them() {
        let m = vec![
            text("system", "You are a secret system prompt"),
            text("user", "first question"),
            text("assistant", "first answer"),
            text("user", "the actual query"),
        ];
        let ctx = context_from_messages(&m).unwrap();
        assert_eq!(ctx, "user: first question\nassistant: first answer");
        assert!(!ctx.contains("secret system prompt"));
        assert!(!ctx.contains("the actual query"));
    }

    #[test]
    fn excludes_tool_results_tool_calls_and_non_text_parts() {
        let mut call = text("assistant", "calling a tool");
        call.tool_calls = Some(vec![]);
        let mut real_call = text("assistant", "calling");
        real_call.tool_calls = Some(vec![crate::ir::ToolCall {
            id: "c1".into(),
            kind: "function".into(),
            function: crate::ir::FunctionCall {
                name: "read_file".into(),
                arguments: "{\"path\":\"/etc/passwd\"}".into(),
            },
            extra: Map::new(),
        }]);
        let m = vec![
            text("user", "look at the file"),
            real_call,
            text("tool", "AWS_SECRET=abc123 contents of the file"),
            msg(
                "assistant",
                json!([{"type": "image_url", "image_url": {"url": "data:..."}}]),
            ),
            msg(
                "assistant",
                json!([{"type": "text", "text": "it contains"}, {"type":"image_url","image_url":{"url":"x"}}]),
            ),
            text("user", "now fix it"),
        ];
        let ctx = context_from_messages(&m).unwrap();
        assert_eq!(ctx, "user: look at the file\nassistant: it contains");
        assert!(!ctx.contains("abc123"));
        assert!(!ctx.contains("/etc/passwd"));
    }

    #[test]
    fn no_prior_turns_or_no_user_turn_yields_none() {
        assert_eq!(context_from_messages(&[text("user", "only")]), None);
        assert_eq!(
            context_from_messages(&[text("system", "s"), text("user", "q")]),
            None
        );
        assert_eq!(context_from_messages(&[text("assistant", "a")]), None);
        assert_eq!(context_from_messages(&[]), None);
        // Blank prior turns are skipped.
        assert_eq!(
            context_from_messages(&[text("user", "   "), text("user", "q")]),
            None
        );
    }

    #[test]
    fn keeps_only_the_most_recent_turns_and_truncates_deterministically() {
        let mut m: Vec<Message> = (0..10)
            .map(|i| {
                text(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &format!("turn {i}"),
                )
            })
            .collect();
        m.push(text("user", "query"));
        let ctx = context_from_messages(&m).unwrap();
        assert_eq!(
            ctx,
            "user: turn 6\nassistant: turn 7\nuser: turn 8\nassistant: turn 9"
        );
        // A single huge turn is cut at MAX_TURN_CHARS chars (not bytes) with a marker.
        let long = "é".repeat(MAX_TURN_CHARS + 50);
        let m = vec![text("assistant", &long), text("user", "q")];
        let ctx = context_from_messages(&m).unwrap();
        assert_eq!(
            ctx.chars().count(),
            "assistant: ".len() + MAX_TURN_CHARS + 1
        );
        assert!(ctx.ends_with('…'));
        // Total cap: oldest turns are dropped first; the block never exceeds the cap.
        let big = "x".repeat(MAX_TURN_CHARS);
        let m = vec![
            text("user", &big),
            text("assistant", &big),
            text("user", &big),
            text("assistant", &big),
            text("user", "q"),
        ];
        let ctx = context_from_messages(&m).unwrap();
        assert!(ctx.chars().count() <= MAX_TOTAL_CHARS);
        assert!(ctx.starts_with("assistant: ") || ctx.starts_with("user: "));
        // Deterministic.
        assert_eq!(ctx, context_from_messages(&m).unwrap());
    }

    #[test]
    fn unicode_and_noise_survive_intact() {
        let m = vec![
            text("user", "  日本語の質問 🚀 \u{0}\t  "),
            text("assistant", "答え"),
            text("user", "next"),
        ];
        let ctx = context_from_messages(&m).unwrap();
        assert_eq!(ctx, "user: 日本語の質問 🚀 \u{0}\nassistant: 答え");
    }

    #[test]
    fn generic_turn_iterator_matches_message_adapter() {
        let turns = vec![
            ("user", "a".to_string()),
            ("assistant", "b".to_string()),
            ("user", "q".to_string()),
        ];
        assert_eq!(
            classifier_context(turns).as_deref(),
            Some("user: a\nassistant: b")
        );
    }
}
