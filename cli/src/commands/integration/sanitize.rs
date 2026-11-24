//! Cleaning captured content before it is validated: text the event contract refuses (NUL characters,
//! which `CodingAgentEventV1::validate` rejects anywhere in the event) is removed from what a coding
//! agent wrote, so one stray byte in a tool's output doesn't cost the whole turn.
//!
//! Only content is cleaned. Identity fields (ids, names) are left as they are: changing one would
//! change the turn's stable event id, so an id the contract refuses still makes the event invalid.
//!
//! Two accounting slips a coding agent's own records can carry are repaired too, rather than costing
//! the turn (the server refuses either):
//!
//! - **Cache-write TTL counts above the aggregate.** A provider's `cache_creation_input_tokens` is the
//!   cache write it bills; the 5-minute and 1-hour counts only split it by TTL, so they can't add up
//!   to more. The aggregate is kept and the split clamped into it ([`cache_ttl_within`]): raising
//!   the aggregate instead would invent tokens no response reported.
//! - **A tool linked to an LLM call the turn doesn't have** (the call carried no usage, say). The
//!   tool still belongs to the turn, so it is linked to the turn instead ([`tool_link`]).

use nasiko_types::{CodingAgentEventV1, CodingAgentToolAssociation};
use serde_json::Value;
use std::collections::HashSet;

const NUL: char = '\0';

/// `event` with NUL characters removed from its title, prompt, response and tool-call content, and the
/// accounting slips in the module docs repaired. A title left blank by that is dropped (the contract
/// allows none, not an empty one).
pub fn sanitized(mut event: CodingAgentEventV1) -> CodingAgentEventV1 {
    event.session.title = event
        .session
        .title
        .map(|title| without_nul(&title))
        .filter(|title| !title.trim().is_empty());
    event.turn.prompt = event.turn.prompt.map(|text| without_nul(&text));
    event.turn.response = event.turn.response.map(|text| without_nul(&text));
    for tool in &mut event.turn.tool_calls {
        tool.arguments = tool.arguments.take().map(|value| value_without_nul(&value));
        tool.output = tool.output.take().map(|value| value_without_nul(&value));
        tool.raw = tool.raw.take().map(|text| without_nul(&text));
        tool.error = tool.error.take().map(|text| without_nul(&text));
    }
    for call in &mut event.turn.llm_calls {
        if let Some(accounting) = &mut call.accounting {
            (
                accounting.cache_creation_5m_tokens,
                accounting.cache_creation_1h_tokens,
            ) = cache_ttl_within(
                call.cache_creation_tokens,
                accounting.cache_creation_5m_tokens,
                accounting.cache_creation_1h_tokens,
            );
        }
    }
    let calls: HashSet<String> = event.turn.llm_calls.iter().map(|c| c.id.clone()).collect();
    for tool in &mut event.turn.tool_calls {
        (tool.association, tool.model_call_id) =
            tool_link(tool.association, tool.model_call_id.take(), &calls);
    }
    event
}

/// The 5-minute and 1-hour cache-write counts, clamped so together they are at most `aggregate`: the
/// 1-hour count first (the dearer one, so spend is not under-reported), the 5-minute count into what
/// is left.
pub(super) fn cache_ttl_within(
    aggregate: u64,
    five_minute: Option<u64>,
    one_hour: Option<u64>,
) -> (Option<u64>, Option<u64>) {
    let one_hour = one_hour.map(|tokens| tokens.min(aggregate));
    let left = aggregate - one_hour.unwrap_or(0);
    (five_minute.map(|tokens| tokens.min(left)), one_hour)
}

/// A tool's link to the LLM call that made it: exact while `calls` (the turn's call ids) has that
/// call, else to the turn.
pub(super) fn tool_link(
    association: CodingAgentToolAssociation,
    model_call_id: Option<String>,
    calls: &HashSet<String>,
) -> (CodingAgentToolAssociation, Option<String>) {
    match (association, model_call_id) {
        (CodingAgentToolAssociation::Exact, Some(id)) if calls.contains(&id) => {
            (CodingAgentToolAssociation::Exact, Some(id))
        }
        (CodingAgentToolAssociation::Exact, _) => (CodingAgentToolAssociation::Turn, None),
        (association, id) => (association, id),
    }
}

fn without_nul(text: &str) -> String {
    text.replace(NUL, "")
}

/// Strings and object keys alike: the server stores tool content as JSON, which can't hold NUL either.
fn value_without_nul(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(without_nul(text)),
        Value::Array(items) => Value::Array(items.iter().map(value_without_nul).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (without_nul(key), value_without_nul(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentCallAccounting, CodingAgentLlmCall,
        CodingAgentSession, CodingAgentSource, CodingAgentTimestampQuality, CodingAgentToolCall,
        CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
    };
    use serde_json::json;

    fn event_with(text: &str) -> CodingAgentEventV1 {
        let at = Utc.timestamp_opt(1, 0).unwrap();
        CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: coding_agent_event_id("claude", "s", "t"),
            captured_at: at,
            source: CodingAgentSource {
                agent_id: "claude".into(),
                agent_name: "claude-code".into(),
            },
            session: CodingAgentSession {
                id: coding_agent_session_id("claude", "s"),
                source_id: "s".into(),
                title: Some(text.into()),
                parent: None,
                place: None,
            },
            turn: CodingAgentTurn {
                id: "t".into(),
                prompt: Some(format!("ask {text}")),
                response: Some(format!("answer {text}")),
                started_at: at,
                ended_at: at,
                llm_calls: vec![],
                tool_calls: vec![CodingAgentToolCall {
                    id: "tool".into(),
                    name: "Read".into(),
                    kind: "read".into(),
                    model_call_id: None,
                    status: CodingAgentToolCallStatus::Succeeded,
                    arguments: Some(json!({ text: [text] })),
                    output: Some(json!(text)),
                    raw: Some(text.into()),
                    error: Some(text.into()),
                    started_at: None,
                    ended_at: None,
                    duration_ms: None,
                    association: CodingAgentToolAssociation::Turn,
                    timestamp_quality: CodingAgentTimestampQuality::Unknown,
                }],
            },
            capture_policy: CapturePolicy::Content,
        }
    }

    #[test]
    fn nul_characters_are_stripped_so_the_event_validates() {
        let event = event_with("a\0b");
        assert!(event.validate().unwrap_err().contains("NUL"));

        let clean = sanitized(event);

        assert_eq!(clean.validate(), Ok(()));
        assert_eq!(clean.session.title.as_deref(), Some("ab"));
        assert_eq!(clean.turn.prompt.as_deref(), Some("ask ab"));
        assert_eq!(clean.turn.response.as_deref(), Some("answer ab"));
        let tool = &clean.turn.tool_calls[0];
        assert_eq!(tool.arguments, Some(json!({"ab": ["ab"]})));
        assert_eq!(tool.output, Some(json!("ab")));
        assert_eq!(tool.raw.as_deref(), Some("ab"));
    }

    #[test]
    fn a_title_of_nothing_but_nul_is_dropped() {
        let clean = sanitized(event_with("\0"));

        assert_eq!(clean.session.title, None);
        assert_eq!(clean.validate(), Ok(()));
    }

    #[test]
    fn clean_content_and_identities_are_left_alone() {
        let event = event_with("plain");
        assert_eq!(sanitized(event.clone()), event);
    }

    fn call(id: &str, aggregate: u64, five_minute: u64, one_hour: u64) -> CodingAgentLlmCall {
        let at = Utc.timestamp_opt(1, 0).unwrap();
        CodingAgentLlmCall {
            id: id.into(),
            provider: "anthropic".into(),
            model: "claude-test".into(),
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_creation_tokens: aggregate,
            accounting: Some(CodingAgentCallAccounting {
                version: 2,
                cache_creation_5m_tokens: Some(five_minute),
                cache_creation_1h_tokens: Some(one_hour),
                ..CodingAgentCallAccounting::default()
            }),
            started_at: at,
            ended_at: at,
        }
    }

    #[test]
    fn cache_ttl_counts_above_the_aggregate_are_clamped_into_it() {
        let mut event = event_with("plain");
        event.turn.llm_calls = vec![call("c", 100, 80, 70)];
        assert_eq!(
            event.validate(),
            Err("cache creation TTL counts exceed aggregate creation".into())
        );

        let clean = sanitized(event);

        assert_eq!(clean.validate(), Ok(()));
        let accounting = clean.turn.llm_calls[0].accounting.as_ref().unwrap();
        assert_eq!(clean.turn.llm_calls[0].cache_creation_tokens, 100);
        assert_eq!(
            (
                accounting.cache_creation_5m_tokens,
                accounting.cache_creation_1h_tokens
            ),
            (Some(30), Some(70))
        );
    }

    #[test]
    fn cache_ttl_counts_within_the_aggregate_are_kept() {
        assert_eq!(
            cache_ttl_within(100, Some(30), Some(70)),
            (Some(30), Some(70))
        );
        assert_eq!(cache_ttl_within(100, Some(20), None), (Some(20), None));
        assert_eq!(cache_ttl_within(5, None, Some(9)), (None, Some(5)));
        assert_eq!(cache_ttl_within(0, Some(3), Some(4)), (Some(0), Some(0)));
    }

    #[test]
    fn a_tool_linked_to_an_unknown_llm_call_is_linked_to_the_turn() {
        let mut event = event_with("plain");
        event.turn.llm_calls = vec![call("known", 0, 0, 0)];
        let linked = event.turn.tool_calls[0].clone();
        event.turn.tool_calls = vec![
            CodingAgentToolCall {
                id: "orphan".into(),
                model_call_id: Some("missing".into()),
                association: CodingAgentToolAssociation::Exact,
                ..linked.clone()
            },
            CodingAgentToolCall {
                id: "linked".into(),
                model_call_id: Some("known".into()),
                association: CodingAgentToolAssociation::Exact,
                ..linked
            },
        ];
        assert_eq!(
            event.validate(),
            Err("exact tool association references an unknown LLM call".into())
        );

        let clean = sanitized(event);

        assert_eq!(clean.validate(), Ok(()));
        let orphan = &clean.turn.tool_calls[0];
        assert_eq!(orphan.association, CodingAgentToolAssociation::Turn);
        assert_eq!(orphan.model_call_id, None);
        let linked = &clean.turn.tool_calls[1];
        assert_eq!(linked.association, CodingAgentToolAssociation::Exact);
        assert_eq!(linked.model_call_id.as_deref(), Some("known"));
    }
}
