//! Translate a coding-agent turn's events into the standard A2A `message/stream`
//! SSE a caller already understands — so a coding agent looks identical to every
//! other agent on the wire, and the chat UI needs no changes to render it.
//!
//! The events are built with the same `nasiko_types::a2a` constructors the
//! orchestrator's own streaming path uses, so the bytes on the wire match what
//! the client already consumes from an ordinary agent.

use std::convert::Infallible;

use agent_client_protocol::schema::v1::{ContentBlock, SessionUpdate};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use nasiko_acp_client::TurnEvent;
use nasiko_types::a2a::{
    AwaitingHumanKind, Part, agent_message, awaiting_human, completed, data_part, failed,
    status_event, text_part, to_sse_data, working_with_message,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::turn::CodingTurnEvent;

/// Stream a running turn's events back as A2A `message/stream` SSE.
pub fn turn_to_sse(
    events: mpsc::UnboundedReceiver<CodingTurnEvent>,
    task_id: String,
    context_id: String,
) -> Response {
    let frames = frames(events, task_id, context_id);
    let stream = async_stream::stream! {
        for await frame in frames {
            yield Ok::<Event, Infallible>(Event::default().data(frame));
        }
    };
    Sse::new(stream).into_response()
}

/// Translate a turn's events into the frames of an A2A stream: the `data` of each SSE event, in
/// order, ending with one terminal frame.
///
/// Each chunk of the agent's reply becomes a `working` status-update carrying the
/// text; the turn's end becomes a terminal `completed` (or `failed`, or `canceled`) status
/// update — which is how an A2A stream signals it's done (there is no separate
/// "final" flag on the event). The client can't tell this apart from an ordinary
/// agent's stream. Separate from [`turn_to_sse`] so a harness turn can publish the same frames
/// to every subscriber (`message/stream` and `tasks/resubscribe`).
pub fn frames(
    mut events: mpsc::UnboundedReceiver<CodingTurnEvent>,
    task_id: String,
    context_id: String,
) -> impl futures::Stream<Item = String> + Send {
    async_stream::stream! {
        let mut saw_terminal = false;
        // Accumulate the reply so the terminal COMPLETED can carry the full text.
        // The streamed chunks ride `working` status-updates, which a consuming UI
        // treats as live *activity*, not the reply; only text on the terminal
        // COMPLETED is read back as the reply (its `outcomeOf`/`replyText`). Without
        // it the turn settles as "no reply" and the UI warns the turn may still be
        // running on the next send, even though it completed.
        let mut reply = String::new();
        while let Some(event) = events.recv().await {
            match event {
                CodingTurnEvent::Turn(TurnEvent::Update(update)) => {
                    if let Some(text) = agent_text(&update) {
                        reply.push_str(&text);
                        yield working_sse(&task_id, &context_id, &text);
                    } else if let Some(activity) = activity_of(&update) {
                        yield activity_sse(&task_id, &context_id, activity);
                    }
                }
                CodingTurnEvent::Turn(TurnEvent::Finished(_)) => {
                    saw_terminal = true;
                    yield completed_sse(&task_id, &context_id, &reply);
                    break;
                }
                CodingTurnEvent::Turn(TurnEvent::Failed(err)) => {
                    saw_terminal = true;
                    yield failed_sse(&task_id, &context_id, &err.to_string());
                    break;
                }
                // The caller cancelled the task (`tasks/cancel`).
                CodingTurnEvent::Canceled => {
                    saw_terminal = true;
                    yield canceled_sse(&task_id, &context_id);
                    break;
                }
                // Non-terminal: a tool call is paused for human approval. Emit the
                // `hitl` data frame so the chat UI renders the inline approve/reject
                // card live, then keep streaming — the turn resumes on its own once
                // the delegate is answered.
                CodingTurnEvent::AwaitingApproval { hitl_id, question } => {
                    yield hitl_frame_sse(&task_id, &context_id, hitl_id, &question);
                }
            }
        }
        // The engine dropped the stream without a terminal event (e.g. the turn's
        // connection was lost mid-flight). Close with a `failed` so the client
        // isn't left waiting on a stream that will never complete.
        if !saw_terminal {
            yield failed_sse(
                &task_id,
                &context_id,
                "coding-agent turn ended without completing",
            );
        }
    }
}

/// A terminal `canceled` status-update.
fn canceled_sse(task_id: &str, context_id: &str) -> String {
    to_sse_data(&status_event(nasiko_types::a2a::canceled(
        task_id, context_id,
    )))
}

/// The visible reply text in an agent message chunk, if this update carries one.
/// Thoughts, tool calls, plans and other update kinds aren't part of the
/// user-visible reply, so they're skipped (v1 streams the agent's answer text).
/// `pub(crate)` so the turn driver can reuse it to accumulate the reply for
/// persistence (see `coding::turn::drive_turn`).
pub(crate) fn agent_text(update: &SessionUpdate) -> Option<String> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(t) if !t.text.is_empty() => Some(t.text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// What the agent is doing besides answering: a tool call, a thought, a plan.
///
/// Streamed as `working` updates tagged `metadata.kind` so a client can show progress without
/// mistaking it for the reply (the reply is only the terminal `completed` text).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Activity {
    pub kind: ActivityKind,
    pub part: Part,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityKind {
    ToolCall,
    Thought,
    Plan,
}

impl ActivityKind {
    /// The `metadata.kind` value on the wire.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ToolCall => "tool_call",
            Self::Thought => "thought",
            Self::Plan => "plan",
        }
    }
}

/// The activity an update reports, if it is one. Thoughts carry their text; tool calls and
/// plans carry a small data object (identity and status, never the tool's raw input or
/// output, which can be large and sensitive).
pub(crate) fn activity_of(update: &SessionUpdate) -> Option<Activity> {
    let (kind, part) = match update {
        SessionUpdate::AgentThoughtChunk(chunk) => match &chunk.content {
            ContentBlock::Text(t) if !t.text.is_empty() => {
                (ActivityKind::Thought, text_part(t.text.clone()))
            }
            _ => return None,
        },
        SessionUpdate::ToolCall(call) => (
            ActivityKind::ToolCall,
            data_part(serde_json::json!({
                "id": call.tool_call_id.0,
                "title": call.title,
                "tool_kind": call.kind,
                "status": call.status,
            })),
        ),
        SessionUpdate::ToolCallUpdate(update) => (
            ActivityKind::ToolCall,
            data_part(serde_json::json!({
                "id": update.tool_call_id.0,
                "title": update.fields.title,
                "tool_kind": update.fields.kind,
                "status": update.fields.status,
            })),
        ),
        SessionUpdate::Plan(plan) => (
            ActivityKind::Plan,
            data_part(serde_json::json!({
                "entries": plan.entries.iter().map(|e| serde_json::json!({
                    "content": e.content,
                    "priority": e.priority,
                    "status": e.status,
                })).collect::<Vec<_>>(),
            })),
        ),
        _ => return None,
    };
    Some(Activity { kind, part })
}

/// A `working` status-update announcing one [`Activity`].
fn activity_sse(task_id: &str, context_id: &str, activity: Activity) -> String {
    let message = agent_message(context_id, task_id, activity.part);
    let mut event = working_with_message(task_id, context_id, message);
    event.metadata = Some(std::collections::HashMap::from_iter([(
        "kind".to_owned(),
        serde_json::Value::from(activity.kind.as_str()),
    )]));
    to_sse_data(&status_event(event))
}

/// A `working` status-update carrying a chunk of the agent's reply.
fn working_sse(task_id: &str, context_id: &str, text: &str) -> String {
    let message = agent_message(context_id, task_id, text_part(text));
    let event = status_event(working_with_message(task_id, context_id, message));
    to_sse_data(&event)
}

/// The terminal `completed` status-update that ends the stream, carrying the full
/// reply text. A consumer reads this text as the turn's reply (its streamed
/// `working` chunks are treated as activity only), so without it the turn settles
/// as "no reply." Empty when the agent produced no visible text.
fn completed_sse(task_id: &str, context_id: &str, reply: &str) -> String {
    let mut event = completed(task_id, context_id);
    if !reply.trim().is_empty() {
        event.status.message = Some(agent_message(context_id, task_id, text_part(reply)));
    }
    to_sse_data(&status_event(event))
}

/// A terminal `failed` status-update carrying the error text.
fn failed_sse(task_id: &str, context_id: &str, error: &str) -> String {
    // (see `canceled_sse` for the user-cancel counterpart)
    to_sse_data(&status_event(failed(task_id, context_id, error)))
}

/// A non-terminal `input-required` status carrying the `hitl` data frame for a
/// pending tool approval. The frame (`{type:"hitl", id, kind, context_id,
/// question}`) is what the chat UI reads to render the inline approve/reject card
/// and to resolve it via `POST /api/hitl/{id}/resolve` — the same shape the
/// orchestrator's `build_hitl_stream_data` emits. `question` carries the ACP
/// tool-call and the offered options.
fn hitl_frame_sse(
    task_id: &str,
    context_id: &str,
    hitl_id: Uuid,
    question: &serde_json::Value,
) -> String {
    let payload = serde_json::json!({
        "type": "hitl",
        "id": hitl_id.to_string(),
        "kind": "tool_approval",
        "task_id": task_id,
        "context_id": context_id,
        "question": question,
    });
    let message = agent_message(context_id, task_id, data_part(payload));
    to_sse_data(&status_event(awaiting_human(
        task_id,
        context_id,
        AwaitingHumanKind::InputRequired,
        message,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ContentChunk;
    use nasiko_types::a2a::{completed, failed, status_event, to_sse_data};

    #[test]
    fn thought_tool_call_and_plan_are_activities_tagged_by_kind() {
        use agent_client_protocol::schema::v1::{
            Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, ToolCall, ToolCallStatus,
        };
        let thought = SessionUpdate::AgentThoughtChunk(ContentChunk::new("hmm".into()));
        let a = activity_of(&thought).expect("thought");
        assert_eq!(a.kind.as_str(), "thought");
        assert_eq!(a.part, text_part("hmm"));

        let call = SessionUpdate::ToolCall(
            ToolCall::new("t1", "read file").status(ToolCallStatus::InProgress),
        );
        let a = activity_of(&call).expect("tool call");
        assert_eq!(a.kind.as_str(), "tool_call");
        let json = serde_json::to_string(&a.part).unwrap();
        assert!(json.contains("t1") && json.contains("read file"), "{json}");

        let plan = SessionUpdate::Plan(Plan::new(vec![PlanEntry::new(
            "write tests",
            PlanEntryPriority::High,
            PlanEntryStatus::Pending,
        )]));
        let a = activity_of(&plan).expect("plan");
        assert_eq!(a.kind.as_str(), "plan");
        assert!(
            serde_json::to_string(&a.part)
                .unwrap()
                .contains("write tests")
        );
    }

    #[test]
    fn reply_text_is_not_an_activity_and_activity_is_not_reply() {
        let reply = SessionUpdate::AgentMessageChunk(ContentChunk::new("hi".into()));
        assert_eq!(activity_of(&reply), None);
        let thought = SessionUpdate::AgentThoughtChunk(ContentChunk::new("hmm".into()));
        assert_eq!(agent_text(&thought), None);
    }

    #[test]
    fn activity_frame_is_a_working_update_with_metadata_kind() {
        let a = activity_of(&SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            "hmm".into(),
        )))
        .unwrap();
        let message = agent_message("c1", "t1", a.part);
        let mut event = working_with_message("t1", "c1", message);
        event.metadata = Some(std::collections::HashMap::from_iter([(
            "kind".to_owned(),
            serde_json::Value::from("thought"),
        )]));
        let data = to_sse_data(&status_event(event));
        assert!(data.contains("TASK_STATE_WORKING"), "{data}");
        assert!(data.contains("\"kind\":\"thought\""), "{data}");
    }

    #[test]
    fn agent_message_text_is_extracted() {
        let update = SessionUpdate::AgentMessageChunk(ContentChunk::new("hello".into()));
        assert_eq!(agent_text(&update).as_deref(), Some("hello"));
    }

    #[test]
    fn non_reply_updates_are_skipped() {
        // Agent thoughts are internal reasoning, not the visible reply.
        let thought = SessionUpdate::AgentThoughtChunk(ContentChunk::new("thinking".into()));
        assert_eq!(agent_text(&thought), None);
        // An empty chunk carries nothing to show.
        let empty = SessionUpdate::AgentMessageChunk(ContentChunk::new("".into()));
        assert_eq!(agent_text(&empty), None);
    }

    #[test]
    fn awaiting_approval_emits_a_hitl_frame_the_ui_can_render() {
        // The pause is a non-terminal input-required status whose message carries
        // the `hitl` data frame: id + kind + question, which the chat UI reads to
        // render the inline approve/reject card and to call the resolve API.
        let id = uuid::Uuid::from_u128(0x1234_5678);
        let question = serde_json::json!({
            "kind": "acp_tool_permission",
            "tool_call": { "title": "Edit /etc/hosts" },
            "options": [{ "optionId": "once", "kind": "allow_once" }],
        });
        let data = to_sse_data(&status_event(nasiko_types::a2a::awaiting_human(
            "t1",
            "c1",
            nasiko_types::a2a::AwaitingHumanKind::InputRequired,
            nasiko_types::a2a::agent_message(
                "c1",
                "t1",
                nasiko_types::a2a::data_part(serde_json::json!({
                    "type": "hitl",
                    "id": id.to_string(),
                    "kind": "tool_approval",
                    "context_id": "c1",
                    "question": question,
                })),
            ),
        )));
        assert!(data.contains("TASK_STATE_INPUT_REQUIRED"), "got: {data}");
        assert!(
            data.contains("\"type\":\"hitl\""),
            "must carry a hitl frame: {data}"
        );
        assert!(
            data.contains("tool_approval"),
            "frame kind must be tool_approval"
        );
        assert!(data.contains(&id.to_string()), "must carry the hitl id");
        assert!(
            data.contains("Edit /etc/hosts"),
            "must carry the tool-call question"
        );
    }

    #[test]
    fn completed_carries_the_reply_so_the_turn_settles_as_a_reply() {
        // The reply rides the terminal COMPLETED (mirrors `completed_sse`); a
        // consumer reads the streamed `working` chunks as activity, so only this
        // text settles the turn as a reply rather than "no reply".
        let mut event = completed("t1", "c1");
        event.status.message = Some(agent_message("c1", "t1", text_part("the answer")));
        let data = to_sse_data(&status_event(event));
        assert!(data.contains("TASK_STATE_COMPLETED"), "got: {data}");
        assert!(
            data.contains("the answer"),
            "reply text must ride the completed event: {data}"
        );
    }

    #[test]
    fn terminal_events_serialize_with_the_right_a2a_state() {
        // completed → the stream's "done, ok" signal.
        let done = to_sse_data(&status_event(completed("t1", "c1")));
        assert!(done.contains("TASK_STATE_COMPLETED"), "got: {done}");
        assert!(done.contains("c1") && done.contains("t1"));

        // failed → carries the error text so the client can show it.
        let err = to_sse_data(&status_event(failed("t1", "c1", "boom")));
        assert!(err.contains("TASK_STATE_FAILED"), "got: {err}");
        assert!(
            err.contains("boom"),
            "error text must ride the event: {err}"
        );
    }
}
