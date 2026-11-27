//! Starting a new session's sandbox when the session is opened, before its first message.
//!
//! Opening a session (`POST /api/sessions`, what a click on a harness sends) starts its sandbox
//! straight away: a pod of its own under the `pod` provider, the user's harness agent and the
//! session's process in it under `harness-agent`. The first message then finds it running, or
//! waits for it, instead of starting it.
//!
//! The start holds the session's turn lease, its `active_turn_id` marked as a start
//! ([`is_starting`]), and takes the steps a turn takes before it attaches
//! (`turn::mark_starting`, `turn::provision_sandbox`): `starting`, the sandbox token, the env,
//! `acquire`. A message that arrives meanwhile waits for the lease rather than being refused as
//! busy (`turn::acquire_lease`). A start that succeeds leaves the row `running`. One that fails
//! before the sandbox is up leaves it `parked`, and the first message starts the sandbox itself;
//! one that fails after leaves it `running`, as a turn does, and the first message attaches.
//!
//! A started sandbox nobody uses is suspended by the idle reaper like any other: its idle time
//! counts from the session's creation, since no turn ran (`SessionStore::release_start`).

use uuid::Uuid;

use super::model::{SessionRow, SessionRuntimeState, SessionStore};
use super::turn::{
    SandboxStart, StartFailure, StartHeartbeat, mark_starting, provision_sandbox, warn_missed_write,
};
use crate::coding::lifecycle::{self, TurnLease};
use crate::coding::proxy::{AgentTarget, load_agent_target};
use crate::coding::turn::replica_id;
use crate::state::AppState;

/// What a start's `active_turn_id` begins with. No A2A task id does: a client's are its own
/// message's task ids, and a turn's are the task ids the platform hands out.
const START_ID_PREFIX: &str = "start:";

/// Whether a start, not a turn, holds the session's lease.
pub fn is_starting(row: &SessionRow) -> bool {
    row.turn_owner.is_some()
        && row
            .active_turn_id
            .as_deref()
            .is_some_and(|id| id.starts_with(START_ID_PREFIX))
}

/// How often [`wait_out`] reads the row again.
const START_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// `row` once no start holds its session: read again until the start gives up its lease, for at
/// most `AGENT_STARTUP_BUDGET_SECS` (how long a sandbox may take to come up). Deleting or
/// suspending a session that was just opened waits for its sandbox rather than being refused as
/// busy, as its first message does (`turn::acquire_lease`). A start that outlasts the wait is
/// returned as it stands, still starting; `None` when the row is gone meanwhile.
pub async fn wait_out(state: &AppState, row: SessionRow) -> sqlx::Result<Option<SessionRow>> {
    if !is_starting(&row) {
        return Ok(Some(row));
    }
    let store = SessionStore::new(state.db.clone());
    let budget = std::time::Duration::from_secs(state.config.agent_startup_budget_secs);
    let waiting_since = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(START_POLL).await;
        let Some(fresh) = store.get(&row.session_id).await? else {
            return Ok(None);
        };
        if !is_starting(&fresh) || waiting_since.elapsed() >= budget {
            return Ok(Some(fresh));
        }
    }
}

/// Begin starting the sandbox of `session_id`, a session of `agent_id`: take its lease and mark
/// it `starting`, then set the sandbox up in the background. `false` when no start began (a turn
/// holds the session, or the start could not be set up, which is logged); the session's first
/// message starts the sandbox then.
pub async fn begin(state: &AppState, session_id: &str, agent_id: Uuid) -> bool {
    let target = match load_agent_target(&state.db, agent_id).await {
        Ok(Some(AgentTarget::Harness(target))) => target,
        Ok(_) => {
            tracing::warn!(%session_id, %agent_id, "not starting a sandbox: the session's agent is not a harness agent");
            return false;
        }
        Err(e) => {
            tracing::warn!(%session_id, %agent_id, error = %e, "not starting a sandbox: could not load the session's agent");
            return false;
        }
    };
    let flow_id = Uuid::new_v4().simple().to_string();
    let start_id = format!("{START_ID_PREFIX}{flow_id}");
    let lease = match lifecycle::acquire_turn(
        &state.db,
        session_id,
        replica_id(),
        &start_id,
        state.config.coding_agent_turn_lease_secs,
    )
    .await
    {
        Ok(Some(lease)) => lease,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "not starting a sandbox: could not take the session's lease");
            return false;
        }
    };
    // Read under the lease: every decision below is made on the row as the lease found it.
    let session = match SessionStore::new(state.db.clone()).get(session_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            release(state, &lease).await;
            return false;
        }
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "not starting a sandbox: could not read the session");
            release(state, &lease).await;
            return false;
        }
    };
    let (beat, lease_lost) = StartHeartbeat::spawn(state, &lease);
    let start = SandboxStart {
        state,
        target: &target,
        session: &session,
        user_id: session.user_id,
        flow_id: &flow_id,
        lease: &lease,
        lease_lost: &lease_lost,
    };
    if let Err(failure) = mark_starting(&start).await {
        drop(beat);
        finish(state, &lease, Err(failure)).await;
        return false;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let start = SandboxStart {
            state: &state,
            target: &target,
            session: &session,
            user_id: session.user_id,
            flow_id: &flow_id,
            lease: &lease,
            lease_lost: &lease_lost,
        };
        let outcome = provision_sandbox(&start).await.map(drop);
        drop(beat);
        finish(&state, &lease, outcome).await;
    });
    true
}

/// Leave the row `running` after a start, or as the failure says, then give up the lease (state
/// first: releasing first would let a turn start in between and have this write land on its row).
async fn finish(state: &AppState, lease: &TurnLease, outcome: Result<(), StartFailure>) {
    let session_id = lease.session_id.as_str();
    let leave = match &outcome {
        Ok(()) => SessionRuntimeState::Running,
        Err(failure) => failure.leave,
    };
    let left = SessionStore::new(state.db.clone())
        .set_runtime_state_fenced(lease, leave)
        .await;
    warn_missed_write("start", session_id, left);
    release(state, lease).await;
    match outcome {
        Ok(()) => tracing::info!(%session_id, "started the session's sandbox"),
        Err(failure) => tracing::warn!(
            %session_id, error = %failure.error,
            "could not start the session's sandbox; its first message starts it"
        ),
    }
}

async fn release(state: &AppState, lease: &TurnLease) {
    if let Err(e) = SessionStore::new(state.db.clone())
        .release_start(lease)
        .await
    {
        tracing::warn!(session_id = %lease.session_id, error = %e, "could not release a start's lease; it lapses");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(turn_owner: Option<&str>, active_turn_id: Option<&str>) -> SessionRow {
        SessionRow {
            session_id: "s".into(),
            user_id: Uuid::nil(),
            agent_id: None,
            acp_session_id: None,
            runtime_handle: None,
            runtime_state: "starting".into(),
            volume_ref: None,
            turn_owner: turn_owner.map(Into::into),
            lease_expires_at: None,
            turn_fence: 1,
            last_turn_at: None,
            active_turn_id: active_turn_id.map(Into::into),
            sandbox_ref: None,
            suspended_at: None,
            workspace_repo: None,
            pending_permission: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn only_a_held_lease_marked_as_a_start_is_a_start() {
        assert!(is_starting(&row(Some("replica"), Some("start:abc"))));
        assert!(!is_starting(&row(Some("replica"), Some("task-1"))));
        assert!(!is_starting(&row(None, Some("start:abc"))));
        assert!(!is_starting(&row(Some("replica"), None)));
    }
}
