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
    agent_message, completed, failed, status_event, text_part, to_sse_data, working_with_message,
};
use tokio::sync::mpsc;

/// Stream a running turn's events back as A2A `message/stream` SSE.
///
/// Each chunk of the agent's reply becomes a `working` status-update carrying the
/// text; the turn's end becomes a terminal `completed` (or `failed`) status
/// update — which is how an A2A stream signals it's done (there is no separate
/// "final" flag on the event). The client can't tell this apart from an ordinary
/// agent's stream.
pub fn turn_to_sse(
    mut events: mpsc::UnboundedReceiver<TurnEvent>,
    task_id: String,
    context_id: String,
) -> Response {
    let stream = async_stream::stream! {
        let mut saw_terminal = false;
        while let Some(event) = events.recv().await {
            match event {
                TurnEvent::Update(update) => {
                    if let Some(text) = agent_text(&update) {
                        yield Ok::<Event, Infallible>(working_sse(&task_id, &context_id, &text));
                    }
                }
                TurnEvent::Finished(_) => {
                    saw_terminal = true;
                    yield Ok(completed_sse(&task_id, &context_id));
                    break;
                }
                TurnEvent::Failed(err) => {
                    saw_terminal = true;
                    yield Ok(failed_sse(&task_id, &context_id, &err.to_string()));
                    break;
                }
            }
        }
        // The engine dropped the stream without a terminal event (e.g. the turn's
        // connection was lost mid-flight). Close with a `failed` so the client
        // isn't left waiting on a stream that will never complete.
        if !saw_terminal {
            yield Ok(failed_sse(
                &task_id,
                &context_id,
                "coding-agent turn ended without completing",
            ));
        }
    };
    Sse::new(stream).into_response()
}

/// The visible reply text in an agent message chunk, if this update carries one.
/// Thoughts, tool calls, plans and other update kinds aren't part of the
/// user-visible reply, so they're skipped (v1 streams the agent's answer text).
fn agent_text(update: &SessionUpdate) -> Option<String> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(t) if !t.text.is_empty() => Some(t.text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A `working` status-update carrying a chunk of the agent's reply.
fn working_sse(task_id: &str, context_id: &str, text: &str) -> Event {
    let message = agent_message(context_id, task_id, text_part(text));
    let event = status_event(working_with_message(task_id, context_id, message));
    Event::default().data(to_sse_data(&event))
}

/// The terminal `completed` status-update that ends the stream.
fn completed_sse(task_id: &str, context_id: &str) -> Event {
    Event::default().data(to_sse_data(&status_event(completed(task_id, context_id))))
}

/// A terminal `failed` status-update carrying the error text.
fn failed_sse(task_id: &str, context_id: &str, error: &str) -> Event {
    Event::default().data(to_sse_data(&status_event(failed(
        task_id, context_id, error,
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ContentChunk;
    use nasiko_types::a2a::{completed, failed, status_event, to_sse_data};

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
