//! The suspend protocol both the reaper and the manual suspend route use: claim a session's
//! sandbox for suspension, act on the provider, settle the row, and hand the claim back when
//! the provider fails.
//!
//! A claim moves the row to `stopping` under a deadline and takes the lease and fence from
//! whatever turn held them (`SessionStore::begin_suspend` and its siblings); every later write
//! carries the claim's fence, so a superseded claimant writes nothing.
//!
//! Every settled suspend, whichever path claimed it, ends the same way: the questions the session
//! left open are closed ([`close_questions_left_open`]), since the harness that asked them has
//! stopped. A turn closes its own when it ends, so these are the ones a crashed turn left behind;
//! without this they would stay open, showing a person a question nothing can act on.

use nasiko_runtime::{RuntimeError, SandboxHandle, SandboxState};

use super::model::{SessionRow, SessionRuntimeState, SessionStore};
use crate::coding::questions::SessionQuestions;
use crate::state::AppState;

/// What went wrong with one session.
#[derive(Debug, thiserror::Error)]
pub(super) enum ReapError {
    /// The provider cannot run sandboxes at all; every other session would fail the same way.
    #[error("provider unsupported: {0}")]
    Unsupported(String),
    /// The provider failed.
    #[error("provider: {0}")]
    Provider(RuntimeError),
    /// The database failed.
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<RuntimeError> for ReapError {
    fn from(e: RuntimeError) -> Self {
        match e {
            RuntimeError::Unsupported(msg) => Self::Unsupported(msg),
            other => Self::Provider(other),
        }
    }
}

/// Claim an unaccounted session for suspension (see
/// [`SessionStore::claim_parked_with_live_sandbox`]).
pub(super) async fn claim_unaccounted(
    store: &SessionStore,
    session: &SessionRow,
    handle: &SandboxHandle,
) -> sqlx::Result<Option<i64>> {
    store
        .claim_parked_with_live_sandbox(
            &session.session_id,
            session.turn_fence,
            &handle.sandbox_ref(),
        )
        .await
}

/// Claim `session` for suspension, taking its lease and fence. `Some(claim_fence)` on a win.
/// A claim over a lapsed turn lease (a crashed turn) is logged, since it overrides that turn.
pub(super) async fn claim_suspend(
    store: &SessionStore,
    session: &SessionRow,
) -> sqlx::Result<Option<i64>> {
    let claim = store
        .begin_suspend(&session.session_id, session.turn_fence)
        .await?;
    if claim.is_some()
        && let Some(owner) = &session.turn_owner
    {
        tracing::warn!(
            session_id = %session.session_id,
            turn_owner = %owner,
            active_turn_id = ?session.active_turn_id,
            lease_expires_at = ?session.lease_expires_at,
            "suspending a sandbox whose turn lease lapsed"
        );
    }
    Ok(claim)
}

/// One claimed session's suspend, with what it needs to act.
pub(super) struct Suspend<'a> {
    state: &'a AppState,
    store: &'a SessionStore,
    session: &'a SessionRow,
    handle: &'a SandboxHandle,
}

impl<'a> Suspend<'a> {
    pub(super) fn new(
        state: &'a AppState,
        store: &'a SessionStore,
        session: &'a SessionRow,
        handle: &'a SandboxHandle,
    ) -> Self {
        Self {
            state,
            store,
            session,
            handle,
        }
    }

    /// Suspend the claimed (`stopping`) session's sandbox and settle the row under
    /// `claim_fence`.
    ///
    /// The sandbox's state is read first: one the provider no longer has is settled as cleared
    /// (token revoked, `sandbox_ref` dropped); anything else goes through `suspend`, which is
    /// idempotent. A provider failure hands the claim back (fenced); failing even that is logged
    /// at error level, never dropped. A settle failure after the provider acted is returned as
    /// `Db` and the row stays `stopping`, so its deadline hands it to a later sweep. `Ok(false)`
    /// means the claim was superseded before it could settle. Reporting is the caller's.
    pub(super) async fn finish(&self, claim_fence: i64) -> Result<bool, ReapError> {
        let id = &self.session.session_id;
        let provider = self.state.sandbox.as_ref();
        let acted = match provider.status(self.handle).await {
            Ok(SandboxState::Absent) => Ok(false),
            Ok(_) => provider.suspend(self.handle).await.map(|()| true),
            Err(e) => Err(e),
        };
        match acted {
            Ok(true) => {
                let settled = self.store.settle_suspended(id, claim_fence).await?;
                if settled {
                    close_questions_left_open(self.state, self.session).await;
                }
                Ok(settled)
            }
            Ok(false) => settle_gone(self.state, self.store, self.session, claim_fence).await,
            Err(e) => {
                release_claim(self.store, self.session, claim_fence).await;
                Err(e.into())
            }
        }
    }
}

/// Settle a claimed session whose sandbox is gone: revoke its sandbox token while the row is
/// still `stopping` (so a revoke failure leaves the claim to be retried, not a cleared row whose
/// credential still opens), then clear the ref and park. `false` when the claim was superseded.
pub(super) async fn settle_gone(
    state: &AppState,
    store: &SessionStore,
    session: &SessionRow,
    claim_fence: i64,
) -> Result<bool, ReapError> {
    let id = &session.session_id;
    if let Err(e) = super::token::revoke_sandbox_token(&state.db, id).await {
        release_claim(store, session, claim_fence).await;
        return Err(e.into());
    }
    let cleared = store.settle_cleared(id, claim_fence).await?;
    if cleared {
        close_questions_left_open(state, session).await;
    }
    Ok(cleared)
}

/// Close the questions `session` left open, asked up to when it went idle
/// (`coding::questions`): the harness that asked them has stopped. A question asked later belongs
/// to a turn that started after the suspend settled, and is left alone. Best effort: the suspend
/// has happened, and an open row only shows a question nothing will act on.
async fn close_questions_left_open(state: &AppState, session: &SessionRow) {
    let Some(idle_since) = session.idle_since() else {
        return;
    };
    let questions = SessionQuestions {
        session_id: &session.session_id,
        owner: session.user_id,
    };
    match questions.close_asked_until(&state.db, idle_since).await {
        Ok(0) => {}
        Ok(closed) => {
            tracing::info!(session_id = %session.session_id, closed, "closed the questions a suspended session left open");
        }
        Err(e) => {
            tracing::warn!(session_id = %session.session_id, error = %e, "could not close the questions a suspended session left open");
        }
    }
}

/// Hand a claim back after a failed suspend, to the state it was claimed from (a reclaimed
/// `stopping` row had been running).
pub(super) async fn release_claim(store: &SessionStore, session: &SessionRow, claim_fence: i64) {
    let restore_to = match session.runtime_state.as_str() {
        "starting" => SessionRuntimeState::Starting,
        "parked" => SessionRuntimeState::Parked,
        _ => SessionRuntimeState::Running,
    };
    if let Err(e) = store
        .release_suspend(&session.session_id, claim_fence, restore_to)
        .await
    {
        tracing::error!(session_id = %session.session_id, error = %e, "session reaper: releasing a suspend claim failed; the deadline will recover it");
    }
}
