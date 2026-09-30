//! Policy for `session/request_permission` requests the agent sends mid-turn.
//!
//! This is the seam Issue 7 (HITL) plugs into. For now it's a fixed policy:
//! reject everything by default (safe), or approve everything (for driving a
//! turn end-to-end in observability-only mode). A future `Delegate` variant
//! will route the request to the Nasiko HITL surface.

use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome,
};

/// How to answer a permission request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PermissionPolicy {
    /// Deny every request (respond `cancelled`). The safe default.
    #[default]
    RejectAll,
    /// Approve every request by selecting its first offered option. Use only
    /// when driving a turn unattended (e.g. the P1 observability path).
    ApproveAll,
}

impl PermissionPolicy {
    /// Decide the response for one request.
    pub(crate) fn decide(&self, request: &RequestPermissionRequest) -> RequestPermissionResponse {
        match self {
            PermissionPolicy::RejectAll => {
                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
            }
            PermissionPolicy::ApproveAll => match request.options.first() {
                Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(option.option_id.clone()),
                )),
                None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
            },
        }
    }
}
