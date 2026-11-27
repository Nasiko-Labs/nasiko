use uuid::Uuid;

use crate::types::{HitlOrigin, HitlRequest};

/// The caller, reduced to exactly what `authorize_hitl_action` needs. Deliberately not
/// `crate::auth::Claims` or `nasiko_auth::Identity` — this crate has no dependency on either
/// (OSS/EE boundary: `oss/hitl` must stay usable from both editions without pulling in a
/// server-specific auth type); the caller builds this from whatever identity type it has.
#[derive(Debug, Clone, Copy)]
pub struct HitlIdentity {
    pub user_id: Uuid,
    pub is_superuser: bool,
}

/// What the caller is trying to do to a `hitl_requests` row. They authorize identically (§10:
/// "one rule for every kind, not two") except on a coding agent's question, which a superuser
/// may view but not answer (see [`authorize_hitl_action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitlAction {
    View,
    Resolve,
    Cancel,
}

impl HitlAction {
    fn as_str(self) -> &'static str {
        match self {
            HitlAction::View => "view",
            HitlAction::Resolve => "resolve",
            HitlAction::Cancel => "cancel",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HitlAuthzError {
    // `{action:?}` (Debug) rendered the Rust variant's own capitalization ("Resolve") straight
    // into an API-facing error message — `as_str()` instead, matching every other enum-to-response
    // conversion in this codebase (found in review).
    #[error("not authorized to {} this HITL request", action.as_str())]
    Forbidden { action: HitlAction },
}

/// The single choke point for HITL authorization (§10): authorized iff the caller is the
/// request's `owner_user_id` or a superuser. Explicitly **not** agent-access-based — a user with
/// grant-based access to a shared agent must never see or answer a *different* user's paused
/// question on that same agent, including `tool_approval` rows (connector credentials are always
/// the delegating user's own, so the exposure is theirs alone).
///
/// A coding agent's question (a harness session's, or a local coding agent's) is answered or
/// cancelled by its owner only: it asks whether to run something in that user's own workspace,
/// which nobody else acts in, superusers included. A superuser may still view it.
pub fn authorize_hitl_action(
    identity: &HitlIdentity,
    request: &HitlRequest,
    action: HitlAction,
) -> Result<(), HitlAuthzError> {
    let owner_only = request.origin == HitlOrigin::CodingAgent && action != HitlAction::View;
    if identity.user_id == request.owner_user_id || (identity.is_superuser && !owner_only) {
        Ok(())
    } else {
        Err(HitlAuthzError::Forbidden { action })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{HitlKind, HitlOrigin, HitlStatus, ResumeStatus};
    use chrono::Utc;
    use serde_json::Value;

    fn request_owned_by(owner_user_id: Uuid) -> HitlRequest {
        HitlRequest {
            id: Uuid::new_v4(),
            kind: HitlKind::InputRequired,
            origin: HitlOrigin::DirectChat,
            status: HitlStatus::Pending,
            resume_status: ResumeStatus::NotStarted,
            agent_id: Uuid::new_v4(),
            owner_user_id,
            resolved_by: None,
            task_id: Some("task-1".into()),
            context_id: Some("ctx-1".into()),
            chat_session_id: None,
            maf_execution_id: None,
            maf_step_index: None,
            connector_id: None,
            tool_name: None,
            arguments_hash: None,
            consumed_at: None,
            question: Value::Null,
            human_response: None,
            resume_state: Value::Null,
            resume_claimed_at: None,
            resume_dispatch_attempts: 0,
            resume_last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: None,
            resolved_at: None,
        }
    }

    #[test]
    fn owner_is_authorized_for_view_and_resolve() {
        let owner = Uuid::new_v4();
        let request = request_owned_by(owner);
        let identity = HitlIdentity {
            user_id: owner,
            is_superuser: false,
        };
        assert!(authorize_hitl_action(&identity, &request, HitlAction::View).is_ok());
        assert!(authorize_hitl_action(&identity, &request, HitlAction::Resolve).is_ok());
    }

    #[test]
    fn superuser_is_authorized_for_every_action_outside_a_coding_agent() {
        let request = request_owned_by(Uuid::new_v4());
        let identity = HitlIdentity {
            user_id: Uuid::new_v4(),
            is_superuser: true,
        };
        for action in [HitlAction::View, HitlAction::Resolve, HitlAction::Cancel] {
            assert!(authorize_hitl_action(&identity, &request, action).is_ok());
        }
    }

    /// A coding agent's question asks whether to run something in its owner's workspace: a
    /// superuser sees it, only the owner answers or cancels it.
    #[test]
    fn a_coding_agents_question_is_answered_by_its_owner_only() {
        let owner = Uuid::new_v4();
        let mut request = request_owned_by(owner);
        request.kind = HitlKind::ToolApproval;
        request.origin = HitlOrigin::CodingAgent;
        let superuser = HitlIdentity {
            user_id: Uuid::new_v4(),
            is_superuser: true,
        };
        assert!(authorize_hitl_action(&superuser, &request, HitlAction::View).is_ok());
        assert!(authorize_hitl_action(&superuser, &request, HitlAction::Resolve).is_err());
        assert!(authorize_hitl_action(&superuser, &request, HitlAction::Cancel).is_err());

        let owner = HitlIdentity {
            user_id: owner,
            is_superuser: false,
        };
        for action in [HitlAction::View, HitlAction::Resolve, HitlAction::Cancel] {
            assert!(authorize_hitl_action(&owner, &request, action).is_ok());
        }
    }

    /// The direct regression test for §10's rule: a different, non-superuser user — even one
    /// with agent-level access — must never be authorized for another user's row, on either a
    /// conversational row or a `tool_approval` row (the case v3's now-superseded
    /// `can_manage_agent` split would have gotten wrong).
    #[test]
    fn different_user_is_denied_on_conversational_and_tool_approval_rows() {
        let owner = Uuid::new_v4();
        let other = Uuid::new_v4();
        let identity = HitlIdentity {
            user_id: other,
            is_superuser: false,
        };

        let conversational = request_owned_by(owner);
        assert!(authorize_hitl_action(&identity, &conversational, HitlAction::View).is_err());

        let mut tool_approval = request_owned_by(owner);
        tool_approval.kind = HitlKind::ToolApproval;
        tool_approval.origin = HitlOrigin::McpTool;
        tool_approval.task_id = None;
        assert!(authorize_hitl_action(&identity, &tool_approval, HitlAction::Resolve).is_err());
    }
}
