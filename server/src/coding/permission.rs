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
    PermissionDelegate, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome,
};
use nasiko_hitl::{HitlError, HitlStatus, HitlStore, NewHitlRequest};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::turn::CodingTurnEvent;

/// How often a pending permission row is re-checked.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// The longest one request waits before it is auto-denied — tracks the per-turn
/// session token's TTL (`CODING_SESSION_TOKEN_TTL_SECS`, an hour).
const MAX_WAIT: Duration = Duration::from_secs(3600);

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
        }
    }

    /// The decision, or a `HitlError` if the request can't be recorded or read —
    /// which [`decide`](Self::decide) turns into a fail-closed denial.
    async fn decide_inner(
        &self,
        request: &RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, HitlError> {
        let row = self
            .store
            .create(NewHitlRequest::coding_agent(
                self.agent_id,
                self.owner_user_id,
                self.session_id.clone(),
                tool_label(request),
                permission_question(request),
            ))
            .await?;
        let id = row.id;
        tracing::info!(hitl_id = %id, agent_id = %self.agent_id, "coding HITL: awaiting human decision");

        // Surface the pending approval on the turn's SSE stream so the client
        // knows a decision is needed and which id to resolve. Best-effort: a
        // closed receiver (client hung up) just means no one is watching — the
        // request is still answerable via the HITL API.
        if let Some(tx) = &self.notice_tx {
            let _ = tx.send(CodingTurnEvent::AwaitingApproval {
                hitl_id: id,
                tool: tool_label(request),
            });
        }

        // Poll until the row leaves `pending`, or the cap elapses. The turn's
        // lease heartbeat runs on its own task, so this wait never jeopardizes it.
        let deadline = tokio::time::Instant::now() + MAX_WAIT;
        loop {
            match self.store.get(id).await?.map(|r| r.status) {
                Some(HitlStatus::Resolved) => return Ok(approve(request)),
                // Any other terminal status (rejected/expired/canceled) is a denial.
                Some(HitlStatus::Rejected | HitlStatus::Expired | HitlStatus::Canceled) => {
                    return Ok(cancelled());
                }
                // Still pending, or not yet readable: keep waiting until the cap.
                Some(HitlStatus::Pending) | None => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(cancelled());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

#[async_trait]
impl PermissionDelegate for HitlPermissionDelegate {
    async fn decide(&self, request: &RequestPermissionRequest) -> RequestPermissionResponse {
        // Fail closed: any error recording or reading the request denies the tool.
        self.decide_inner(request).await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, agent_id = %self.agent_id, "coding HITL: denying (delegate error)");
            cancelled()
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

/// The human-facing question payload: the full tool-call details and the options
/// the agent offered, so the approver sees exactly what will run.
fn permission_question(request: &RequestPermissionRequest) -> serde_json::Value {
    serde_json::json!({
        "kind": "acp_tool_permission",
        "tool_call": request.tool_call,
        "options": request.options,
    })
}

/// Approve by selecting an allow option — prefer an explicit "allow once", else
/// the first offered option; `cancelled` if the agent offered none.
fn approve(request: &RequestPermissionRequest) -> RequestPermissionResponse {
    let option = request
        .options
        .iter()
        .find(|o| matches!(o.kind, PermissionOptionKind::AllowOnce))
        .or_else(|| request.options.first());
    match option {
        Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option.option_id.clone()),
        )),
        None => cancelled(),
    }
}

fn cancelled() -> RequestPermissionResponse {
    RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_acp_client::PermissionOption;
    use nasiko_acp_client::sdk::schema::v1::{ToolCallUpdate, ToolCallUpdateFields};

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
    fn tool_label_uses_the_title_then_a_generic_fallback() {
        assert_eq!(tool_label(&request(Some("Edit /x"), vec![])), "Edit /x");
        assert_eq!(tool_label(&request(None, vec![])), "coding-agent tool");
        assert_eq!(
            tool_label(&request(Some("  "), vec![])),
            "coding-agent tool"
        );
    }
}
