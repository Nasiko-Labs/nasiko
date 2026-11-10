//! The HITL-backed ACP permission delegate (NAS-842).
//!
//! When a coding agent is `hitl_enabled`, the turn drives it with this delegate
//! instead of auto-approving. For each `session/request_permission` the agent
//! sends mid-turn, the delegate records a pending `hitl_requests` row (origin
//! `coding_agent`, kind `tool_approval`), waits for a human to resolve it via the
//! HITL API (`POST /api/hitl/{id}/resolve`), then answers the agent: approve →
//! select an allow option, reject/expire/cancel → `cancelled`.
//!
//! It runs **off** the ACP event loop — the connection spawns `decide` onto a
//! task holding the responder (see `nasiko_acp_client`) — so the poll-until-
//! resolved wait never stalls the turn's streamed updates, and the turn's lease
//! heartbeat keeps the session alive meanwhile. The per-turn session token's TTL
//! (an hour) bounds the wait: past it the agent's follow-up calls are refused
//! anyway, so a still-pending request is auto-denied at [`MAX_WAIT`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nasiko_acp_client::{
    PermissionDelegate, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, select_allow_option, select_reject_option,
};
use nasiko_hitl::{HitlError, HitlStatus, HitlStore, NewHitlRequest};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::turn::CodingTurnEvent;

/// How often a pending permission row is re-checked.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// The longest one request waits before it is auto-denied — the per-turn session token's TTL
/// (`sessions::identity::SESSION_TOKEN_TTL_SECS`, an hour). In practice the turn's own deadline
/// (`AGENT_CALL_TIMEOUT_SECS`, 600 s by default) ends the turn, and the wait with it, long before
/// this: a human has at most that long to answer. The cap bounds only a deadline set above it.
const MAX_WAIT: Duration = Duration::from_secs(3600);

/// Where a turn records that it is blocked on a human, so a restart or a resume can see the
/// wait (`chat_sessions.pending_permission`). Implemented by the session layer, so this module
/// stays ignorant of the table.
///
/// Only one pending request is tracked per session: an agent asks for one permission at a time,
/// and the record is a pointer (`{hitl_id, tool_call_id}`) to the `hitl_requests` row, not a
/// copy of it. The delegate clears it when the wait ends for any reason, including its future
/// being dropped because the connection was lost; the clear is keyed on `hitl_id`, so it never
/// wipes a newer wait's pointer.
#[async_trait]
pub trait PendingPermissionSink: Send + Sync {
    /// The turn is now waiting on `hitl_id`.
    async fn set_pending(&self, hitl_id: Uuid, tool_call_id: &str);
    /// The wait on `hitl_id` ended.
    async fn clear_pending(&self, hitl_id: Uuid);
}

/// Clears the pending record when the wait ends, even if the waiting future is dropped
/// mid-await (the ACP connection closing drops it): `Drop` cannot await, so it hands the clear
/// to the runtime. [`finish`](Self::finish) is the normal path and awaits the clear in place.
struct PendingGuard {
    sink: Arc<dyn PendingPermissionSink>,
    hitl_id: Uuid,
    cleared: bool,
}

impl PendingGuard {
    async fn finish(mut self) {
        self.sink.clear_pending(self.hitl_id).await;
        self.cleared = true;
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self.cleared {
            return;
        }
        let (sink, hitl_id) = (self.sink.clone(), self.hitl_id);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { sink.clear_pending(hitl_id).await });
        }
    }
}

/// Routes each ACP permission request to the Nasiko HITL surface for a human
/// decision. One per turn — it carries that turn's agent/user/session identity so
/// the recorded row is attributed and shown to the right person.
pub struct HitlPermissionDelegate {
    store: Arc<dyn HitlStore>,
    agent_id: Uuid,
    owner_user_id: Uuid,
    session_id: String,
    /// Where to announce a newly-created pending request so the turn's SSE stream
    /// can surface it as `TASK_STATE_INPUT_REQUIRED`. `None` when the turn isn't
    /// streaming to a client (e.g. hermetic tests).
    notice_tx: Option<mpsc::UnboundedSender<CodingTurnEvent>>,
    /// Persists the pending request across the wait; `None` for the exec-based coding path.
    pending_sink: Option<Arc<dyn PendingPermissionSink>>,
    /// Fired when the task is cancelled (`tasks/cancel`): a wait in progress ends at once, answered
    /// `Cancelled`, and its `hitl_requests` row is closed as `canceled`.
    cancel: Option<tokio_util::sync::CancellationToken>,
    /// Where each question this turn asks is noted, so the turn's driver can close the ones still
    /// open when the turn ends.
    questions: Option<super::questions::TurnQuestions>,
}

impl HitlPermissionDelegate {
    pub fn new(
        store: Arc<dyn HitlStore>,
        agent_id: Uuid,
        owner_user_id: Uuid,
        session_id: String,
        notice_tx: Option<mpsc::UnboundedSender<CodingTurnEvent>>,
    ) -> Self {
        Self {
            store,
            agent_id,
            owner_user_id,
            session_id,
            notice_tx,
            pending_sink: None,
            cancel: None,
            questions: None,
        }
    }

    /// End a wait in progress when `cancel` fires (the turn's task was cancelled).
    pub fn with_cancel(mut self, cancel: tokio_util::sync::CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Record each pending request through `sink` for the duration of its wait.
    pub fn with_pending_sink(mut self, sink: Arc<dyn PendingPermissionSink>) -> Self {
        self.pending_sink = Some(sink);
        self
    }

    /// Note each question this delegate asks in `questions`, the turn's record of them.
    pub fn with_questions(mut self, questions: super::questions::TurnQuestions) -> Self {
        self.questions = Some(questions);
        self
    }

    /// The decision, or a `HitlError` if the request can't be recorded or read —
    /// which [`decide`](Self::decide) turns into a fail-closed denial.
    async fn decide_inner(
        &self,
        request: &RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, HitlError> {
        let question = permission_question(request);
        let row = self
            .store
            .create(NewHitlRequest::coding_agent(
                self.agent_id,
                self.owner_user_id,
                self.session_id.clone(),
                tool_label(request),
                question.clone(),
            ))
            .await?;
        let id = row.id;
        if let Some(questions) = &self.questions {
            questions.record(id);
        }
        tracing::info!(hitl_id = %id, agent_id = %self.agent_id, "coding HITL: awaiting human decision");

        // Surface the pending approval on the turn's SSE stream, carrying the
        // question so the chat UI renders the approve/reject card inline. Best-
        // effort: a closed receiver (client hung up) just means no one is watching
        // — the request is still answerable via the HITL API.
        if let Some(tx) = &self.notice_tx {
            let _ = tx.send(CodingTurnEvent::AwaitingApproval {
                hitl_id: id,
                question: question.clone(),
            });
        }

        // Armed before the write is awaited: if that await is cut short the pointer is cleared
        // anyway.
        let guard = self.pending_sink.as_ref().map(|sink| PendingGuard {
            sink: sink.clone(),
            hitl_id: id,
            cleared: false,
        });
        if let Some(sink) = &self.pending_sink {
            sink.set_pending(id, &request.tool_call.tool_call_id.0)
                .await;
        }
        let decision = self.wait_for_decision(id, request).await;
        if let Some(guard) = guard {
            guard.finish().await;
        }
        decision
    }

    /// Poll until the row leaves `pending`, or the cap elapses. The turn's lease heartbeat
    /// runs on its own task, so this wait never jeopardizes it.
    async fn wait_for_decision(
        &self,
        id: Uuid,
        request: &RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, HitlError> {
        let deadline = tokio::time::Instant::now() + MAX_WAIT;
        loop {
            match self.store.get(id).await?.map(|r| r.status) {
                Some(HitlStatus::Resolved) => return Ok(approve(request)),
                // Any other terminal status (rejected/expired/canceled) is a denial.
                Some(HitlStatus::Rejected | HitlStatus::Expired | HitlStatus::Canceled) => {
                    return Ok(deny(request));
                }
                // Still pending, or not yet readable: keep waiting until the cap.
                Some(HitlStatus::Pending) | None => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(deny(request));
            }
            let cancelled = async {
                match &self.cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = tokio::time::sleep(POLL_INTERVAL) => {}
                () = cancelled => {
                    // The task was cancelled while a human was being asked: close the request so
                    // nobody is left approving something that no longer runs, and tell the agent
                    // the turn is over.
                    if let Err(e) = self.store.cancel(id, self.owner_user_id).await {
                        tracing::warn!(error = %e, hitl_id = %id, "coding HITL: could not cancel the request");
                    }
                    return Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
                }
            }
        }
    }
}

#[async_trait]
impl PermissionDelegate for HitlPermissionDelegate {
    async fn decide(&self, request: &RequestPermissionRequest) -> RequestPermissionResponse {
        // Fail closed: any error recording or reading the request denies the tool.
        self.decide_inner(request).await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, agent_id = %self.agent_id, "coding HITL: denying (delegate error)");
            deny(request)
        })
    }
}

/// A short label for the gated tool, stored as `tool_name` for display. Prefers
/// the tool call's own title, falling back to a generic label.
fn tool_label(request: &RequestPermissionRequest) -> String {
    request
        .tool_call
        .fields
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "coding-agent tool".to_owned())
}

/// The human-facing question payload. Carries three things the chat UI reads to
/// render an approval card the approver can actually judge: `tool_name` (the
/// heading), `action` (a normalized summary of *what it will do* — the file diff,
/// the shell command, or the raw input), and the full `tool_call` + `options` for
/// anything that wants the untouched ACP detail. Normalizing server-side keeps the
/// UI from having to know each agent's tool shapes.
fn permission_question(request: &RequestPermissionRequest) -> serde_json::Value {
    let tool_call = serde_json::to_value(&request.tool_call).unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "kind": "acp_tool_permission",
        "tool_name": tool_display_name(&tool_call),
        "action": permission_action(&tool_call),
        "tool_call": request.tool_call,
        "options": request.options,
    })
}

/// The tool's display name for the card heading: its ACP `title` (e.g. the file
/// path being edited, or the shell command), else a generic label.
fn tool_display_name(tool_call: &serde_json::Value) -> String {
    tool_call
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("coding-agent tool")
        .to_owned()
}

/// Normalize a tool call into *what it will do*, so the approver sees the effect
/// before approving: a file diff (`type` `"diff"`/`"edit"`), a shell command
/// (`"command"`), or the raw input (`"raw"`). Keyed off the fields OpenCode and
/// Codex actually send — a ready-made unified diff, a structured content diff, or
/// a shell command carried in the tool-call title.
fn permission_action(tool_call: &serde_json::Value) -> serde_json::Value {
    let raw = tool_call.get("rawInput");
    // 1. A ready-made unified diff (OpenCode edits carry `rawInput.diff`).
    if let Some(diff) = raw.and_then(|r| r.get("diff")).and_then(|d| d.as_str()) {
        let path = raw
            .and_then(|r| r.get("filepath"))
            .and_then(|p| p.as_str())
            .or_else(|| tool_call.get("title").and_then(|t| t.as_str()));
        return serde_json::json!({ "type": "diff", "path": path, "diff": diff });
    }
    // 2. A structured content diff (path + old/new text).
    if let Some(entry) = tool_call
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| {
            a.iter()
                .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("diff"))
        })
    {
        return serde_json::json!({
            "type": "edit",
            "path": entry.get("path"),
            "oldText": entry.get("oldText"),
            "newText": entry.get("newText"),
        });
    }
    // 3. A shell command (Codex `execute`): the title is the human-readable form.
    let kind = tool_call.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    if (kind == "execute" || raw.and_then(|r| r.get("command")).is_some())
        && let Some(cmd) = tool_call
            .get("title")
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        return serde_json::json!({ "type": "command", "command": cmd });
    }
    // 4. Fallback: the raw input (or the whole tool call) as-is.
    serde_json::json!({ "type": "raw", "value": raw.cloned().unwrap_or_else(|| tool_call.clone()) })
}

/// Approve by selecting an allow option (prefer "allow once"). Shared with the
/// auto-approve policy via the ACP client, so the two paths can't diverge.
fn approve(request: &RequestPermissionRequest) -> RequestPermissionResponse {
    select_allow_option(request)
}

/// Deny by selecting a reject option (prefer "reject once"), falling back to a
/// whole-turn `Cancelled` only when the agent offers no reject option — refusing
/// one tool should not abort the turn.
fn deny(request: &RequestPermissionRequest) -> RequestPermissionResponse {
    select_reject_option(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_acp_client::sdk::schema::v1::{ToolCallUpdate, ToolCallUpdateFields};
    use nasiko_acp_client::{PermissionOption, PermissionOptionKind, RequestPermissionOutcome};

    fn request(title: Option<&str>, options: Vec<PermissionOption>) -> RequestPermissionRequest {
        // `ToolCallUpdateFields` is #[non_exhaustive]: build via Default + field set.
        let mut fields = ToolCallUpdateFields::default();
        fields.title = title.map(str::to_owned);
        RequestPermissionRequest::new(
            "ses_1".to_owned(),
            ToolCallUpdate::new("call_1".to_owned(), fields),
            options,
        )
    }

    fn opt(id: &str, kind: PermissionOptionKind) -> PermissionOption {
        PermissionOption::new(id.to_owned(), id.to_owned(), kind)
    }

    #[test]
    fn approve_prefers_an_allow_once_option_regardless_of_order() {
        let req = request(
            None,
            vec![
                opt("always", PermissionOptionKind::AllowAlways),
                opt("once", PermissionOptionKind::AllowOnce),
                opt("reject", PermissionOptionKind::RejectOnce),
            ],
        );
        match approve(&req).outcome {
            RequestPermissionOutcome::Selected(sel) => {
                assert_eq!(sel.option_id.to_string(), "once")
            }
            other => panic!("expected the allow-once option, got {other:?}"),
        }
    }

    #[test]
    fn approve_falls_back_to_the_first_option_when_none_is_allow_once() {
        let req = request(None, vec![opt("always", PermissionOptionKind::AllowAlways)]);
        match approve(&req).outcome {
            RequestPermissionOutcome::Selected(sel) => {
                assert_eq!(sel.option_id.to_string(), "always")
            }
            other => panic!("expected the first option, got {other:?}"),
        }
    }

    #[test]
    fn approve_with_no_options_is_cancelled() {
        let req = request(None, vec![]);
        assert!(matches!(
            approve(&req).outcome,
            RequestPermissionOutcome::Cancelled
        ));
    }

    #[test]
    fn deny_selects_a_reject_option_rather_than_cancelling_the_turn() {
        let req = request(
            None,
            vec![
                opt("allow", PermissionOptionKind::AllowOnce),
                opt("reject", PermissionOptionKind::RejectOnce),
            ],
        );
        match deny(&req).outcome {
            RequestPermissionOutcome::Selected(sel) => {
                assert_eq!(sel.option_id.to_string(), "reject")
            }
            other => panic!("expected the reject option, got {other:?}"),
        }
    }

    #[test]
    fn deny_with_no_reject_option_cancels_the_turn() {
        let req = request(None, vec![opt("allow", PermissionOptionKind::AllowOnce)]);
        assert!(matches!(
            deny(&req).outcome,
            RequestPermissionOutcome::Cancelled
        ));
    }

    #[test]
    fn tool_label_uses_the_title_then_a_generic_fallback() {
        assert_eq!(tool_label(&request(Some("Edit /x"), vec![])), "Edit /x");
        assert_eq!(tool_label(&request(None, vec![])), "coding-agent tool");
        assert_eq!(
            tool_label(&request(Some("  "), vec![])),
            "coding-agent tool"
        );
    }

    #[test]
    fn tool_display_name_uses_title_then_a_fallback() {
        assert_eq!(
            tool_display_name(&serde_json::json!({ "title": "Edit /x" })),
            "Edit /x"
        );
        assert_eq!(
            tool_display_name(&serde_json::json!({ "title": "  " })),
            "coding-agent tool"
        );
        assert_eq!(
            tool_display_name(&serde_json::json!({})),
            "coding-agent tool"
        );
    }

    #[test]
    fn action_summarizes_an_opencode_edit_as_a_diff() {
        // OpenCode edits carry a ready-made unified diff in `rawInput`.
        let tc = serde_json::json!({
            "kind": "edit",
            "title": "/workspace/ganesh.txt",
            "rawInput": { "filepath": "/workspace/ganesh.txt", "diff": "@@ -0,0 +1 @@\n+hi\n" },
        });
        let action = permission_action(&tc);
        assert_eq!(action["type"], "diff");
        assert_eq!(action["path"], "/workspace/ganesh.txt");
        assert!(action["diff"].as_str().unwrap().contains("+hi"));
    }

    #[test]
    fn action_summarizes_a_content_diff_edit() {
        // Some agents send the diff as structured content (old/new text) instead.
        let tc = serde_json::json!({
            "kind": "edit",
            "title": "/f.txt",
            "content": [{ "type": "diff", "path": "/f.txt", "oldText": "a", "newText": "b" }],
        });
        let action = permission_action(&tc);
        assert_eq!(action["type"], "edit");
        assert_eq!(action["oldText"], "a");
        assert_eq!(action["newText"], "b");
    }

    #[test]
    fn action_summarizes_a_codex_execute_as_a_command() {
        // Codex `execute` carries the human-readable command as the title.
        let tc = serde_json::json!({
            "kind": "execute",
            "title": "echo 'hi' > f.txt",
            "rawInput": { "command": ["/bin/bash", "-lc", "echo 'hi' > f.txt"] },
        });
        let action = permission_action(&tc);
        assert_eq!(action["type"], "command");
        assert_eq!(action["command"], "echo 'hi' > f.txt");
    }

    #[test]
    fn action_falls_back_to_raw_for_an_unknown_tool() {
        let tc = serde_json::json!({ "kind": "other", "rawInput": { "foo": "bar" } });
        let action = permission_action(&tc);
        assert_eq!(action["type"], "raw");
        assert_eq!(action["value"]["foo"], "bar");
    }

    #[derive(Default)]
    struct RecordingSink(std::sync::Mutex<Vec<(&'static str, Uuid)>>);

    #[async_trait]
    impl PendingPermissionSink for RecordingSink {
        async fn set_pending(&self, hitl_id: Uuid, _tool_call_id: &str) {
            self.0.lock().unwrap().push(("set", hitl_id));
        }
        async fn clear_pending(&self, hitl_id: Uuid) {
            self.0.lock().unwrap().push(("clear", hitl_id));
        }
    }

    #[tokio::test]
    async fn the_pending_record_is_cleared_when_the_wait_is_abandoned() {
        let sink = Arc::new(RecordingSink::default());
        let id = Uuid::from_u128(7);
        let guard = PendingGuard {
            sink: sink.clone(),
            hitl_id: id,
            cleared: false,
        };
        // The waiting future is dropped mid-await, as when the ACP connection closes.
        let waiting = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        waiting.abort();
        let _ = waiting.await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(*sink.0.lock().unwrap(), vec![("clear", id)]);
    }

    #[tokio::test]
    async fn a_finished_wait_clears_the_record_once() {
        let sink = Arc::new(RecordingSink::default());
        let id = Uuid::from_u128(8);
        PendingGuard {
            sink: sink.clone(),
            hitl_id: id,
            cleared: false,
        }
        .finish()
        .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(*sink.0.lock().unwrap(), vec![("clear", id)]);
    }
}
