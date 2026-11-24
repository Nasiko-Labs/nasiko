//! A2A `tasks/get`, `tasks/cancel` and `tasks/resubscribe` for harness sessions.
//!
//! A harness agent has no HTTP server of its own to answer these, so the platform does, from the
//! session row and the in-process live registry ([`super::live`]). Both front doors (the agent
//! proxy by UUID and the dispatch route by name or UUID) call [`handle`].
//!
//! A task is a turn: its id is the turn id, its context is the session. Only the live turn and the
//! session's most recent finished turn are known; any other id is `TaskNotFound`, as is a task of
//! someone else's session (the two are indistinguishable on purpose). Errors ride a 200, like
//! every other JSON-RPC error this server returns for an agent.

use std::time::Duration;

use axum::Json;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use nasiko_types::a2a::{
    Message, Task, TaskState, TaskStatus, agent_message, data_part, text_part,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::live::LiveTurn;
use crate::state::AppState;

const TASK_NOT_FOUND: i32 = -32001;
const TASK_NOT_CANCELABLE: i32 = -32002;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
/// How often a task running on another replica is polled for changes when no frames arrive
/// through Redis.
const POLL_EVERY: Duration = Duration::from_secs(1);
/// How often the row is checked while frames do flow through Redis: only a liveness check, for a
/// publisher that died or a terminal frame that never arrived.
const LIVENESS_POLL_EVERY: Duration = Duration::from_secs(5);

/// How long `tasks/cancel` waits for the turn to wind down before answering anyway.
const CANCEL_WAIT: Duration = Duration::from_secs(12);
const CANCEL_POLL: Duration = Duration::from_millis(250);

/// The task methods a harness agent answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskMethod {
    Get,
    Cancel,
    Resubscribe,
}

/// The task method a JSON-RPC method name is, under either spelling (`GetTask` or `tasks/get`).
pub fn task_method(method: &str) -> Option<TaskMethod> {
    match method {
        "GetTask" | "tasks/get" => Some(TaskMethod::Get),
        "CancelTask" | "tasks/cancel" => Some(TaskMethod::Cancel),
        "SubscribeToTask" | "tasks/resubscribe" => Some(TaskMethod::Resubscribe),
        _ => None,
    }
}

/// What the row says about a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskView {
    pub task_id: String,
    pub session_id: String,
    pub live: bool,
    pub state: TaskState,
    /// The replica holding the turn's lease, when it is live.
    pub owner: Option<String>,
    /// The pending approval pointer, when the turn waits on a human.
    pub pending: Option<Value>,
}

/// The state of a turn from its session row. A turn is live only while its lease is: a row whose
/// lease lapsed (a crashed replica) reports `failed`, not `working`. While live it is `canceled`
/// once a cancel was requested, `input-required` while it waits on a human, `working` otherwise.
/// Once over it is the recorded outcome.
pub fn state_of(
    live: bool,
    cancel_requested: bool,
    pending: bool,
    outcome: Option<&str>,
) -> TaskState {
    if live {
        return if cancel_requested {
            TaskState::Canceled
        } else if pending {
            TaskState::InputRequired
        } else {
            TaskState::Working
        };
    }
    match outcome {
        Some("completed") => TaskState::Completed,
        Some("canceled") => TaskState::Canceled,
        _ => TaskState::Failed,
    }
}

#[derive(sqlx::FromRow)]
struct TaskRow {
    session_id: String,
    active_turn_id: Option<String>,
    last_turn_id: Option<String>,
    last_turn_state: Option<String>,
    pending_permission: Option<Value>,
    lease_live: bool,
    cancel_requested: bool,
    turn_owner: Option<String>,
}

const TASK_COLUMNS: &str = "session_id, active_turn_id, last_turn_id, last_turn_state, \
    pending_permission, COALESCE(lease_expires_at > now(), false) AS lease_live, \
    cancel_requested_at IS NOT NULL AS cancel_requested, turn_owner";

impl TaskRow {
    fn into_view(self, task_id: &str) -> TaskView {
        let live = self.active_turn_id.as_deref() == Some(task_id) && self.lease_live;
        // The outcome is the recorded one only for the turn it was recorded for.
        let outcome = (self.last_turn_id.as_deref() == Some(task_id))
            .then_some(self.last_turn_state.as_deref())
            .flatten();
        TaskView {
            task_id: task_id.to_owned(),
            session_id: self.session_id,
            live,
            state: state_of(
                live,
                self.cancel_requested,
                self.pending_permission.is_some(),
                outcome,
            ),
            owner: live.then_some(self.turn_owner).flatten(),
            pending: if live { self.pending_permission } else { None },
        }
    }
}

/// The caller's task, by id: live (`active_turn_id` with a live lease) or the last finished one.
/// `None` for an unknown id or another user's session.
pub async fn find_task(
    state: &AppState,
    agent_id: Uuid,
    user_id: Uuid,
    task_id: &str,
) -> sqlx::Result<Option<TaskView>> {
    let row: Option<TaskRow> = sqlx::query_as(&format!(
        "SELECT {TASK_COLUMNS} FROM chat_sessions \
         WHERE agent_id = $1 AND user_id = $2 AND deleted_at IS NULL \
           AND (active_turn_id = $3 OR last_turn_id = $3) \
         LIMIT 1"
    ))
    .bind(agent_id)
    .bind(user_id)
    .bind(task_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|row| row.into_view(task_id)))
}

/// A task by id and session, for following one already found.
async fn reload(state: &AppState, view: &TaskView) -> sqlx::Result<Option<TaskView>> {
    let row: Option<TaskRow> = sqlx::query_as(&format!(
        "SELECT {TASK_COLUMNS} FROM chat_sessions WHERE session_id = $1 AND deleted_at IS NULL"
    ))
    .bind(&view.session_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|row| row.into_view(&view.task_id)))
}

/// The message a Task carries in its status: the reply of a completed turn (the last assistant
/// message of the session), or the approval a waiting turn is blocked on.
async fn status_message(
    state: &AppState,
    view: &TaskView,
    task_state: &TaskState,
) -> Option<Message> {
    let part = match task_state {
        TaskState::Completed => {
            let reply: Option<String> = sqlx::query_scalar(
                "SELECT content FROM chat_messages WHERE session_id = $1 AND role = 'assistant' \
                 ORDER BY timestamp DESC LIMIT 1",
            )
            .bind(&view.session_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            text_part(reply.filter(|r| !r.trim().is_empty())?)
        }
        TaskState::InputRequired => {
            let pointer = view.pending.as_ref()?;
            data_part(json!({
                "type": "hitl",
                "id": pointer.get("hitl_id"),
                "tool_call_id": pointer.get("tool_call_id"),
            }))
        }
        _ => return None,
    };
    Some(agent_message(&view.session_id, &view.task_id, part))
}

async fn task_json(state: &AppState, view: &TaskView, task_state: TaskState) -> Value {
    let message = status_message(state, view, &task_state).await;
    let task = Task {
        id: view.task_id.clone(),
        context_id: view.session_id.clone(),
        status: TaskStatus {
            state: task_state,
            message,
            timestamp: Some(chrono::Utc::now()),
        },
        artifacts: None,
        history: None,
        metadata: None,
    };
    serde_json::to_value(task).unwrap_or(Value::Null)
}

fn rpc_result(id: &Value, result: Value) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn rpc_error(id: &Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
        .into_response()
}

fn internal_error(id: &Value) -> Response {
    rpc_error(id, INTERNAL_ERROR, "internal error")
}

/// The task id a request names: `params.id`, or `params.name` as `tasks/{id}`.
pub fn requested_task_id(rpc: &Value) -> Option<String> {
    let params = rpc.get("params")?;
    let id = params.get("id").and_then(Value::as_str).or_else(|| {
        params
            .get("name")
            .and_then(Value::as_str)
            .map(|n| n.strip_prefix("tasks/").unwrap_or(n))
    })?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// Answer one task request for the caller. `rpc` is the whole JSON-RPC request.
pub async fn handle(
    state: &AppState,
    agent_id: Uuid,
    user_id: Uuid,
    method: TaskMethod,
    rpc: &Value,
) -> Response {
    let id = rpc.get("id").cloned().unwrap_or(Value::Null);
    let Some(task_id) = requested_task_id(rpc) else {
        return rpc_error(&id, INVALID_PARAMS, "params.id is required");
    };
    let view = match find_task(state, agent_id, user_id, &task_id).await {
        Ok(Some(view)) => view,
        Ok(None) => return rpc_error(&id, TASK_NOT_FOUND, &format!("Task not found: {task_id}")),
        Err(e) => {
            tracing::error!(error = %e, %task_id, "task lookup failed");
            return internal_error(&id);
        }
    };
    match method {
        TaskMethod::Get => rpc_result(&id, task_json(state, &view, view.state.clone()).await),
        TaskMethod::Cancel => cancel(state, &id, view).await,
        TaskMethod::Resubscribe => resubscribe(state, &id, view).await,
    }
}

fn not_cancelable(id: &Value, task_id: &str) -> Response {
    rpc_error(
        id,
        TASK_NOT_CANCELABLE,
        &format!("Task cannot be canceled: {task_id}"),
    )
}

/// `tasks/cancel`: stop the live turn. In this process the turn is told directly; the row flag is
/// set as well, which is how a turn on another replica finds out at its next heartbeat. The reply
/// waits (bounded) for the turn to end so the Task it returns says `canceled`.
async fn cancel(state: &AppState, id: &Value, view: TaskView) -> Response {
    // A cancel already requested, or a turn already over, leaves nothing to cancel.
    if !view.live || view.state == TaskState::Canceled {
        return not_cancelable(id, &view.task_id);
    }
    let flagged = sqlx::query(
        "UPDATE chat_sessions SET cancel_requested_at = now() \
         WHERE session_id = $1 AND active_turn_id = $2 AND lease_expires_at > now()",
    )
    .bind(&view.session_id)
    .bind(&view.task_id)
    .execute(&state.db)
    .await;
    match flagged {
        Ok(done) if done.rows_affected() == 1 => {}
        // The turn ended between the lookup and now.
        Ok(_) => return not_cancelable(id, &view.task_id),
        Err(e) => {
            tracing::error!(error = %e, task_id = %view.task_id, "could not flag the cancel");
            return internal_error(id);
        }
    }
    let local = LiveTurn::get(&view.task_id);
    if let Some(live) = &local {
        live.cancel.cancel();
    }
    wait_for_end(state, &view, local).await;
    // Both "ended as canceled" and "still winding down" are canceled to the caller, who asked for
    // it; a turn that finished first reports what it really was.
    let reported = match reload(state, &view).await {
        Ok(Some(now)) if !now.live && now.state != TaskState::Canceled => now.state,
        _ => TaskState::Canceled,
    };
    rpc_result(id, task_json(state, &view, reported).await)
}

/// Wait for the turn to end: on its in-process watch when this process runs it, else by polling
/// the row (another replica runs it).
async fn wait_for_end(state: &AppState, view: &TaskView, local: Option<LiveTurn>) {
    if let Some(live) = local {
        let mut finished = live.finished();
        let _ = tokio::time::timeout(CANCEL_WAIT, async {
            while !*finished.borrow() {
                if finished.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        return;
    }
    let deadline = tokio::time::Instant::now() + CANCEL_WAIT;
    while tokio::time::Instant::now() < deadline {
        match reload(state, view).await {
            Ok(Some(now)) if now.live => {}
            _ => return,
        }
        tokio::time::sleep(CANCEL_POLL).await;
    }
}

/// `tasks/resubscribe`: the live turn's frames from now on, as the same SSE `message/stream`
/// produces. A turn running on another replica has no in-process stream to join, so the caller
/// gets a Task snapshot and then the turn's frames relayed from Redis ([`super::live`]); without
/// Redis the row is polled until the turn ends (state changes and the final reply, not every
/// chunk).
async fn resubscribe(state: &AppState, id: &Value, view: TaskView) -> Response {
    if !view.live {
        return rpc_error(
            id,
            TASK_NOT_FOUND,
            &format!("Task not found: {}", view.task_id),
        );
    }
    if let Some(live) = LiveTurn::get(&view.task_id) {
        return super::live::sse(live.subscribe());
    }
    if view.owner.as_deref() == Some(crate::coding::turn::replica_id()) {
        // This replica holds the lease but has no handle yet (the turn is starting or just
        // ended): nothing to join, but asking again will work.
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": INTERNAL_ERROR,
                "message": "the task is not streamable yet; retry",
                "data": {"retryable": true, "reason": "task-not-registered"},
            },
        }))
        .into_response();
    }
    follow_remote_sse(state.clone(), view).await
}

/// The next relayed frame, or never when there is no Redis stream to read.
async fn next_frame(frames: &mut Option<RemoteFrames>) -> Option<String> {
    match frames {
        Some(frames) => frames.next().await,
        None => std::future::pending().await,
    }
}

type RemoteFrames = std::pin::Pin<Box<dyn futures::Stream<Item = String> + Send>>;

/// SSE for a turn another replica runs: subscribe to its frames first, then send the Task
/// snapshot, then relay frames until the terminal one. The row is polled throughout, slowly while
/// frames flow (liveness only) and every second without them; when the row shows the turn over
/// before a terminal frame arrived, the final state is reported from the row and the stream ends.
async fn follow_remote_sse(state: AppState, view: TaskView) -> Response {
    use axum::response::sse::{Event, KeepAlive};
    // Before the snapshot, so nothing published in between is missed; the snapshot covers the rest.
    let mut frames = super::live::subscribe_frames(state.redis.client(), &view.task_id).await;
    let stream = async_stream::stream! {
        let snapshot = task_json(&state, &view, view.state.clone()).await;
        yield Ok::<Event, std::convert::Infallible>(
            Event::default().data(json!({"task": snapshot}).to_string()),
        );
        let mut last = view.state.clone();
        loop {
            let wait = if frames.is_some() { LIVENESS_POLL_EVERY } else { POLL_EVERY };
            tokio::select! {
                frame = next_frame(&mut frames) => match frame {
                    Some(frame) => {
                        let terminal = super::live::is_terminal_frame(&frame);
                        yield Ok(Event::default().data(frame));
                        if terminal {
                            break;
                        }
                        continue;
                    }
                    // The Redis connection ended: carry on by polling.
                    None => { frames = None; continue; }
                },
                _ = tokio::time::sleep(wait) => {}
            }
            let Ok(Some(now)) = reload(&state, &view).await else { break };
            // With frames flowing, a state change reaches the caller as a frame; the row speaks
            // only once the turn is over (its terminal frame was missed or never published).
            if now.state != last && (frames.is_none() || !now.live) {
                last = now.state.clone();
                let message = status_message(&state, &now, &now.state).await;
                let update = json!({"statusUpdate": {
                    "taskId": view.task_id,
                    "contextId": view.session_id,
                    "status": {"state": serde_json::to_value(&now.state).unwrap_or(Value::Null), "message": message},
                }});
                yield Ok(Event::default().data(update.to_string()));
            }
            if !now.live {
                break;
            }
        }
    };
    axum::response::sse::Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_method_spellings_name_the_same_task_methods() {
        assert_eq!(task_method("tasks/get"), Some(TaskMethod::Get));
        assert_eq!(task_method("GetTask"), Some(TaskMethod::Get));
        assert_eq!(task_method("tasks/cancel"), Some(TaskMethod::Cancel));
        assert_eq!(task_method("CancelTask"), Some(TaskMethod::Cancel));
        assert_eq!(
            task_method("tasks/resubscribe"),
            Some(TaskMethod::Resubscribe)
        );
        assert_eq!(
            task_method("SubscribeToTask"),
            Some(TaskMethod::Resubscribe)
        );
        assert_eq!(task_method("message/send"), None);
    }

    #[test]
    fn a_live_turn_is_working_waiting_or_canceling_and_a_finished_one_reports_its_outcome() {
        assert_eq!(state_of(true, false, false, None), TaskState::Working);
        assert_eq!(state_of(true, false, true, None), TaskState::InputRequired);
        assert_eq!(state_of(true, true, true, None), TaskState::Canceled);
        assert_eq!(
            state_of(false, false, false, Some("completed")),
            TaskState::Completed
        );
        assert_eq!(
            state_of(false, false, false, Some("canceled")),
            TaskState::Canceled
        );
        assert_eq!(
            state_of(false, false, false, Some("failed")),
            TaskState::Failed
        );
        // A turn whose lease lapsed has no recorded outcome: it failed, it is not working.
        assert_eq!(state_of(false, false, false, None), TaskState::Failed);
    }

    #[test]
    fn the_task_id_comes_from_id_or_a_tasks_name() {
        let id = |v: Value| requested_task_id(&v);
        assert_eq!(id(json!({"params": {"id": "t1"}})).as_deref(), Some("t1"));
        assert_eq!(
            id(json!({"params": {"name": "tasks/t2"}})).as_deref(),
            Some("t2")
        );
        assert_eq!(id(json!({"params": {"id": ""}})), None);
        assert_eq!(id(json!({})), None);
    }
}
