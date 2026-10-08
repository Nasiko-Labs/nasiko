//! Request-size budget for coding-agent uploads.
//!
//! The ingest route rejects bodies over [`CODING_AGENT_BATCH_MAX_BYTES`] with
//! HTTP 413, so batches are sized by their exact serialized request bytes —
//! envelope and separators included — rather than by event count alone. One
//! event can exceed the limit on its own (several bounded tool fields, or JSON
//! escaping inflating bounded text), so such an event is reduced to its usage:
//! tokens, models, timestamps and tool-call metadata survive, while bulk text
//! is truncated or dropped. The event keeps its content capture policy so the
//! server still prices the turn and records it in the chat history.

use nasiko_types::{
    CODING_AGENT_BATCH_MAX_BYTES, CODING_AGENT_BATCH_MAX_EVENTS, CapturePolicy,
    CodingAgentEventBatchRequest, CodingAgentEventV1,
};

/// Prompt and response bytes kept when an oversized event is reduced. Small
/// enough that even worst-case JSON escaping cannot approach the request limit.
const REDUCED_TEXT_MAX_BYTES: usize = 16 * 1024;
const TRUNCATION_MARKER: &str =
    "\n[nasiko: content truncated; this turn exceeded the telemetry upload size limit]";

/// How an event relates to the single-event request budget.
#[derive(Debug, PartialEq)]
pub enum EventFit {
    Fits,
    /// The event was too large; this smaller copy keeps its identity and usage.
    Reduced(Box<CodingAgentEventV1>),
    /// Even the reduced event exceeds the limit; it can never be delivered.
    Oversized {
        request_bytes: usize,
    },
}

/// Decide whether `event` can be uploaded in a request of its own.
pub fn fit(event: &CodingAgentEventV1) -> EventFit {
    if single_request_bytes(event) <= CODING_AGENT_BATCH_MAX_BYTES {
        return EventFit::Fits;
    }
    let reduced = reduce(event);
    match single_request_bytes(&reduced) {
        bytes if bytes <= CODING_AGENT_BATCH_MAX_BYTES => EventFit::Reduced(Box::new(reduced)),
        request_bytes => EventFit::Oversized { request_bytes },
    }
}

/// Actionable reason recorded when an event is [`EventFit::Oversized`].
pub fn oversized_error(request_bytes: usize) -> String {
    format!(
        "event needs {request_bytes} request bytes even with its content removed, over the \
         {CODING_AGENT_BATCH_MAX_BYTES}-byte upload limit; its usage cannot be uploaded"
    )
}

/// Copy of `event` with bulk content truncated or dropped. Identity, usage and
/// timing are untouched, so the event id stays stable. Idempotent.
pub fn reduce(event: &CodingAgentEventV1) -> CodingAgentEventV1 {
    let mut reduced = event.clone();
    if reduced.capture_policy == CapturePolicy::Content {
        reduced.turn.prompt = reduced.turn.prompt.as_deref().map(truncate);
        reduced.turn.response = reduced.turn.response.as_deref().map(truncate);
    }
    for tool in &mut reduced.turn.tool_calls {
        tool.arguments = None;
        tool.output = None;
        tool.raw = None;
        tool.error = None;
    }
    reduced
}

/// Split `items` into consecutive batches whose serialized request bodies stay
/// within both the byte and the event-count limits. An item too large to share
/// a request is sent alone; [`fit`] keeps such items from reaching here.
pub fn batches<T>(items: &[T], event: impl Fn(&T) -> &CodingAgentEventV1) -> Vec<&[T]> {
    let envelope = request_bytes(&[]);
    let mut batches = Vec::new();
    let mut start = 0;
    let mut size = envelope;
    for (index, item) in items.iter().enumerate() {
        let separator = usize::from(index > start);
        let added = event_bytes(event(item)) + separator;
        let full = index - start == CODING_AGENT_BATCH_MAX_EVENTS;
        if index > start && (full || size + added > CODING_AGENT_BATCH_MAX_BYTES) {
            batches.push(&items[start..index]);
            start = index;
            size = envelope + event_bytes(event(item));
        } else {
            size += added;
        }
    }
    if start < items.len() {
        batches.push(&items[start..]);
    }
    batches
}

/// Exact byte length of the request body for `events`, as sent on the wire.
pub fn request_bytes(events: &[CodingAgentEventV1]) -> usize {
    serialized_len(&CodingAgentEventBatchRequest {
        events: events.to_vec(),
    })
}

fn single_request_bytes(event: &CodingAgentEventV1) -> usize {
    request_bytes(&[]) + event_bytes(event)
}

fn event_bytes(event: &CodingAgentEventV1) -> usize {
    serialized_len(event)
}

fn serialized_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn truncate(text: &str) -> String {
    if text.len() <= REDUCED_TEXT_MAX_BYTES {
        return text.to_string();
    }
    let mut end = REDUCED_TEXT_MAX_BYTES - TRUNCATION_MARKER.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &text[..end])
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use nasiko_types::{
        CODING_AGENT_CONTENT_MAX_BYTES, CODING_AGENT_EVENT_VERSION, CodingAgentLlmCall,
        CodingAgentSession, CodingAgentSource, CodingAgentTimestampQuality,
        CodingAgentToolAssociation, CodingAgentToolCall, CodingAgentToolCallStatus,
        CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
    };

    const MIB: usize = 1024 * 1024;

    pub(in super::super) fn event(
        turn: &str,
        prompt: &str,
        tool_outputs: Vec<String>,
    ) -> CodingAgentEventV1 {
        let at = Utc.timestamp_opt(1, 0).unwrap();
        CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: coding_agent_event_id("claude", "session", turn),
            captured_at: at,
            source: CodingAgentSource {
                agent_id: "claude".into(),
                agent_name: "coding-agent".into(),
            },
            session: CodingAgentSession {
                id: coding_agent_session_id("claude", "session"),
                source_id: "session".into(),
                title: Some("title".into()),
            },
            turn: CodingAgentTurn {
                id: turn.into(),
                prompt: Some(prompt.into()),
                response: Some("response".into()),
                started_at: at,
                ended_at: at,
                llm_calls: vec![CodingAgentLlmCall {
                    id: format!("call-{turn}"),
                    provider: "provider".into(),
                    model: "model".into(),
                    input_tokens: 1_234,
                    output_tokens: 567,
                    cache_read_tokens: 89,
                    cache_creation_tokens: 10,
                    accounting: None,
                    started_at: at,
                    ended_at: at,
                }],
                tool_calls: tool_outputs
                    .into_iter()
                    .enumerate()
                    .map(|(index, output)| CodingAgentToolCall {
                        id: format!("tool-{index}"),
                        name: "Read".into(),
                        kind: "tool".into(),
                        model_call_id: None,
                        status: CodingAgentToolCallStatus::Succeeded,
                        arguments: None,
                        output: Some(serde_json::Value::String(output)),
                        raw: None,
                        error: None,
                        started_at: Some(at),
                        ended_at: Some(at),
                        duration_ms: Some(1),
                        association: CodingAgentToolAssociation::Turn,
                        timestamp_quality: CodingAgentTimestampQuality::Exact,
                    })
                    .collect(),
            },
            capture_policy: CapturePolicy::Content,
        }
    }

    /// Valid per-field, yet over the request limit once serialized: nine tool
    /// outputs at the content cap.
    pub(in super::super) fn oversized_event(turn: &str) -> CodingAgentEventV1 {
        event(
            turn,
            "prompt",
            // Two bytes of each cap go to the JSON string quotes.
            vec!["x".repeat(CODING_AGENT_CONTENT_MAX_BYTES - 2); 9],
        )
    }

    fn assert_within_limits(batches: &[&[CodingAgentEventV1]]) {
        for batch in batches {
            assert!(batch.len() <= CODING_AGENT_BATCH_MAX_EVENTS);
            assert!(request_bytes(batch) <= CODING_AGENT_BATCH_MAX_BYTES);
        }
    }

    #[test]
    fn request_size_counts_the_envelope_and_separators_exactly() {
        let events = vec![
            event("a", "short", vec![]),
            event("b", "\"quoted\"\n", vec!["\u{1}".into()]),
        ];
        let expected = serde_json::to_vec(&CodingAgentEventBatchRequest {
            events: events.clone(),
        })
        .unwrap()
        .len();
        assert_eq!(request_bytes(&events), expected);
        assert_eq!(
            expected,
            request_bytes(&[]) + event_bytes(&events[0]) + 1 + event_bytes(&events[1])
        );
    }

    #[test]
    fn json_escaping_is_measured_not_raw_length() {
        // 768 KiB of control characters stays under the per-field cap but
        // serializes as six bytes per character.
        let escaped = event("escape", &"\u{1}".repeat(3 * MIB / 4), vec![]);
        assert!(escaped.turn.prompt.as_ref().unwrap().len() < CODING_AGENT_CONTENT_MAX_BYTES);
        assert!(event_bytes(&escaped) > 4 * MIB);
        let events = vec![escaped.clone(), escaped.clone(), escaped];
        let packed = batches(&events, |event| event);
        assert_eq!(packed.len(), 3, "raw length would have packed all three");
        assert_within_limits(&packed);
    }

    #[test]
    fn mixed_sizes_pack_in_order_within_byte_and_count_limits() {
        let mut events = Vec::new();
        for index in 0..250 {
            let prompt = match index % 50 {
                0 => "y".repeat(CODING_AGENT_CONTENT_MAX_BYTES - 1),
                7 => "\"".repeat(CODING_AGENT_CONTENT_MAX_BYTES / 2),
                _ => "small".into(),
            };
            events.push(event(&format!("turn-{index}"), &prompt, vec![]));
        }
        let packed = batches(&events, |event| event);
        assert_within_limits(&packed);
        assert_eq!(packed.iter().map(|batch| batch.len()).sum::<usize>(), 250);
        let order: Vec<_> = packed.iter().flat_map(|batch| batch.iter()).collect();
        assert!(
            order
                .iter()
                .zip(&events)
                .all(|(left, right)| *left == right)
        );
        assert_eq!(packed.len(), 3, "count limit, not bytes, binds here");

        let heavy: Vec<_> = (0..20)
            .map(|index| {
                event(
                    &format!("heavy-{index}"),
                    &"z".repeat(CODING_AGENT_CONTENT_MAX_BYTES),
                    vec![],
                )
            })
            .collect();
        let heavy_packed = batches(&heavy, |event| event);
        assert_within_limits(&heavy_packed);
        assert_eq!(heavy_packed[0].len(), 7, "bytes, not count, bind here");
    }

    #[test]
    fn events_within_the_limit_are_untouched() {
        assert_eq!(
            fit(&event("small", "prompt", vec!["out".into()])),
            EventFit::Fits
        );
    }

    #[test]
    fn oversized_event_is_reduced_to_its_usage_with_a_stable_id() {
        let original = oversized_event("big");
        assert!(original.validate().is_ok());
        assert!(single_request_bytes(&original) > CODING_AGENT_BATCH_MAX_BYTES);
        let EventFit::Reduced(reduced) = fit(&original) else {
            panic!("an event over the limit must be reduced");
        };
        assert!(single_request_bytes(&reduced) <= CODING_AGENT_BATCH_MAX_BYTES);
        assert!(reduced.validate().is_ok());
        assert_eq!(reduced.event_id, original.event_id);
        assert_eq!(reduced.capture_policy, CapturePolicy::Content);
        assert_eq!(reduced.turn.llm_calls, original.turn.llm_calls);
        assert_eq!(reduced.turn.started_at, original.turn.started_at);
        assert_eq!(reduced.turn.tool_calls.len(), 9);
        assert!(
            reduced
                .turn
                .tool_calls
                .iter()
                .all(|tool| tool.output.is_none())
        );
        assert_eq!(reduce(&reduced), *reduced, "reduction is idempotent");
    }

    #[test]
    fn long_text_is_truncated_on_a_char_boundary_with_a_marker() {
        let truncated = truncate(&"é".repeat(REDUCED_TEXT_MAX_BYTES));
        assert!(truncated.len() <= REDUCED_TEXT_MAX_BYTES);
        assert!(truncated.ends_with(TRUNCATION_MARKER));
        assert_eq!(truncate("short"), "short");
    }
}
