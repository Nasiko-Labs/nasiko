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
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome,
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
            PermissionPolicy::ApproveAll => select_allow_option(request),
            PermissionPolicy::RejectAll | PermissionPolicy::Delegate(_) => {
                select_reject_option(request)
            }
        }
    }
}

/// Approve a request by selecting an **allow** option.
///
/// Prefers "allow once" over "allow always" (the least-privilege choice), and
/// only falls back to the first offered option if the agent labeled none by kind.
/// The ACP options list is ordered but a reject choice may come first, so picking
/// `options.first()` blindly can *deny* a tool under an approve-all policy — this
/// selects by [`PermissionOptionKind`] instead. `cancelled` if the agent offered
/// no options at all.
pub fn select_allow_option(request: &RequestPermissionRequest) -> RequestPermissionResponse {
    let chosen = find_kind(request, PermissionOptionKind::AllowOnce)
        .or_else(|| find_kind(request, PermissionOptionKind::AllowAlways))
        .or_else(|| request.options.first());
    selected_or_cancelled(chosen)
}

/// Deny a request by selecting a **reject** option when the agent offers one.
///
/// Prefers "reject once" over "reject always". Falls back to
/// [`RequestPermissionOutcome::Cancelled`] only when the agent offers no reject
/// option — in ACP `Cancelled` cancels the whole turn, so it is a last resort,
/// not the way to refuse a single tool.
pub fn select_reject_option(request: &RequestPermissionRequest) -> RequestPermissionResponse {
    let chosen = find_kind(request, PermissionOptionKind::RejectOnce)
        .or_else(|| find_kind(request, PermissionOptionKind::RejectAlways));
    selected_or_cancelled(chosen)
}

/// The first option of the given kind, if any.
fn find_kind(
    request: &RequestPermissionRequest,
    kind: PermissionOptionKind,
) -> Option<&PermissionOption> {
    request.options.iter().find(|o| o.kind == kind)
}

/// Select the given option, or `cancelled` when there is none.
fn selected_or_cancelled(option: Option<&PermissionOption>) -> RequestPermissionResponse {
    match option {
        Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option.option_id.clone()),
        )),
        None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ToolCallUpdate, ToolCallUpdateFields};

    fn request(options: Vec<PermissionOption>) -> RequestPermissionRequest {
        // `ToolCallUpdateFields` is #[non_exhaustive]: build via Default + set.
        let fields = ToolCallUpdateFields::default();
        RequestPermissionRequest::new(
            "ses_1".to_owned(),
            ToolCallUpdate::new("call_1".to_owned(), fields),
            options,
        )
    }

    fn opt(id: &str, kind: PermissionOptionKind) -> PermissionOption {
        PermissionOption::new(id.to_owned(), id.to_owned(), kind)
    }

    fn selected_id(resp: RequestPermissionResponse) -> Option<String> {
        match resp.outcome {
            RequestPermissionOutcome::Selected(sel) => Some(sel.option_id.to_string()),
            _ => None,
        }
    }

    #[test]
    fn approve_prefers_allow_once_over_allow_always_and_ignores_reject_order() {
        // A reject option comes first; approve must still pick the allow-once one,
        // never `options.first()`.
        let req = request(vec![
            opt("reject", PermissionOptionKind::RejectOnce),
            opt("always", PermissionOptionKind::AllowAlways),
            opt("once", PermissionOptionKind::AllowOnce),
        ]);
        assert_eq!(
            selected_id(select_allow_option(&req)).as_deref(),
            Some("once")
        );
    }

    #[test]
    fn approve_falls_back_to_allow_always_then_first_option() {
        let req = request(vec![
            opt("reject", PermissionOptionKind::RejectOnce),
            opt("always", PermissionOptionKind::AllowAlways),
        ]);
        assert_eq!(
            selected_id(select_allow_option(&req)).as_deref(),
            Some("always")
        );

        // No option carries an allow kind — fall back to the first offered.
        let other = request(vec![opt("other", PermissionOptionKind::RejectAlways)]);
        assert_eq!(
            selected_id(select_allow_option(&other)).as_deref(),
            Some("other")
        );
    }

    #[test]
    fn approve_with_no_options_is_cancelled() {
        assert!(matches!(
            select_allow_option(&request(vec![])).outcome,
            RequestPermissionOutcome::Cancelled
        ));
    }

    #[test]
    fn reject_prefers_reject_once_over_reject_always() {
        let req = request(vec![
            opt("allow", PermissionOptionKind::AllowOnce),
            opt("always", PermissionOptionKind::RejectAlways),
            opt("once", PermissionOptionKind::RejectOnce),
        ]);
        assert_eq!(
            selected_id(select_reject_option(&req)).as_deref(),
            Some("once")
        );
    }

    #[test]
    fn reject_with_no_reject_option_cancels_the_turn() {
        // Only allow options offered: the single-tool refusal degrades to a
        // whole-turn `Cancelled` as a last resort.
        let req = request(vec![opt("allow", PermissionOptionKind::AllowOnce)]);
        assert!(matches!(
            select_reject_option(&req).outcome,
            RequestPermissionOutcome::Cancelled
        ));
    }
}
