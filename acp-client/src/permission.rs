//! Policy for `session/request_permission` requests the agent sends mid-turn.
//!
//! Three shapes: reject everything (the safe default), approve everything (for
//! driving a turn unattended in observability-only mode), or **delegate** each
//! request to an async [`PermissionDelegate`] — the seam the Nasiko HITL surface
//! plugs into (NAS-842).
//!
//! A delegate may block on a human, so it must never run on the ACP event loop
//! (that would stall the turn's streamed updates). The connection answers a
//! delegated request from a spawned task holding the responder — see
//! [`crate::connection`] — so `decide` is free to await for as long as it needs.

use std::fmt;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome,
};
use async_trait::async_trait;

/// Answers one permission request, possibly after awaiting a human.
///
/// Implementations run **off** the ACP event loop (in a task spawned by the
/// connection), so `decide` may do I/O or wait on a person without stalling the
/// turn. It must always produce a response — map "approve" to the agent's
/// allow option and "deny" (or any failure) to [`RequestPermissionOutcome::Cancelled`].
#[async_trait]
pub trait PermissionDelegate: Send + Sync {
    async fn decide(&self, request: &RequestPermissionRequest) -> RequestPermissionResponse;
}

/// How to answer a permission request.
#[derive(Clone, Default)]
pub enum PermissionPolicy {
    /// Deny every request (respond `cancelled`). The safe default.
    #[default]
    RejectAll,
    /// Approve every request by selecting its first offered option. Use only
    /// when driving a turn unattended (e.g. the P1 observability path).
    ApproveAll,
    /// Route every request to an async delegate (e.g. the Nasiko HITL surface).
    Delegate(Arc<dyn PermissionDelegate>),
}

impl fmt::Debug for PermissionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PermissionPolicy::RejectAll => "RejectAll",
            PermissionPolicy::ApproveAll => "ApproveAll",
            PermissionPolicy::Delegate(_) => "Delegate(..)",
        })
    }
}

/// Equality by variant only — a `Delegate`'s boxed delegate is not compared
/// (trait objects have no meaningful equality), so two `Delegate`s compare equal.
/// Exists so config carrying a policy can still derive `PartialEq`/be asserted in
/// tests; it is never used to pick a decision.
impl PartialEq for PermissionPolicy {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}
impl Eq for PermissionPolicy {}

impl PermissionPolicy {
    /// The delegate to route to, when this policy delegates.
    pub(crate) fn delegate(&self) -> Option<&Arc<dyn PermissionDelegate>> {
        match self {
            PermissionPolicy::Delegate(delegate) => Some(delegate),
            _ => None,
        }
    }

    /// The synchronous decision for the fixed policies. `Delegate` is answered on
    /// the async path instead; it returns `Cancelled` here purely as a
    /// fail-closed fallback that should never be reached.
    pub(crate) fn decide_fixed(
        &self,
        request: &RequestPermissionRequest,
    ) -> RequestPermissionResponse {
        match self {
            PermissionPolicy::ApproveAll => approve_first_option(request),
            PermissionPolicy::RejectAll | PermissionPolicy::Delegate(_) => {
                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
            }
        }
    }
}

/// Approve a request by selecting its first offered option (or `cancelled` if it
/// offers none).
pub(crate) fn approve_first_option(
    request: &RequestPermissionRequest,
) -> RequestPermissionResponse {
    match request.options.first() {
        Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option.option_id.clone()),
        )),
        None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
    }
}
