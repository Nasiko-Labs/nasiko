//! The agent half of the reaper: prunes expired open-task rows, and releases the agent sandboxes
//! left from before parking scaled agents' own workloads.
//!
//! Nothing acquires an `Agent`/`AgentContext` sandbox any more: a parked agent is woken by scaling
//! its own workload back up (`agent_wake`). A sandbox the provider still lists under an agent key
//! was acquired before that, and at most serves a call that began on a replica still running the
//! earlier server. Each is released once it can no longer be serving one: at once when it is not
//! running or its agent is gone, and when it is running, once its agent has had no request for the
//! call window and none is in flight in this process. Runs whatever the idle TTL, as clean-up.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nasiko_runtime::{SandboxInfo, SandboxKey, SandboxState};
use sqlx::PgPool;
use uuid::Uuid;

use super::{Disposition, SweepOutcome, Tally};
use crate::agents::open_tasks;
use crate::sessions::suspend::ReapError;
use crate::state::AppState;

/// Delete open-task rows past their bound, once per tick. A row is kept at least as long as an
/// open pause can hold a workload park ([`super::workloads::PAUSE_HOLDS_PARK_AT_MOST`]), and longer
/// when `AGENT_OPEN_TASK_MAX_AGE_SECS` says so. A failure is logged, never fatal.
pub(super) async fn prune_open_tasks(state: &AppState) {
    let max_age = Duration::from_secs(state.config.agent_open_task_max_age_secs)
        .max(super::workloads::PAUSE_HOLDS_PARK_AT_MOST);
    if let Err(e) = open_tasks::prune(&state.db, max_age).await {
        tracing::warn!(error = %e, "reaper: pruning expired open tasks failed");
    }
}

/// Release the leftover agent-keyed sandboxes in `infos` (the tick's one provider listing).
pub(super) async fn release_leftover_sandboxes(
    state: &AppState,
    infos: &[SandboxInfo],
) -> sqlx::Result<SweepOutcome> {
    let window = call_window(
        state.config.agent_call_timeout_secs,
        state.config.session_reaper_interval_secs,
    );
    let mut tally = Tally::default();
    for (agent_id, sandboxes) in group_by_agent(infos) {
        let result = release_leftovers(state, agent_id, &sandboxes, window).await;
        // An agent whose sandbox may still be serving is not a "skipped" anything.
        if matches!(result, Ok(Disposition::Left)) {
            continue;
        }
        if let Err(msg) = tally.record(&agent_id.to_string(), result) {
            return Ok(SweepOutcome::Unsupported(msg));
        }
    }
    Ok(tally.outcome())
}

/// How long after an agent's last request a call to it can still be in flight on another
/// replica: the call timeout, plus the last-request stamp throttle (a request inside it leaves
/// `last_request_at` up to that much older than the call), plus one reaper interval.
fn call_window(call_timeout_secs: u64, reaper_interval_secs: u64) -> Duration {
    Duration::from_secs(
        call_timeout_secs
            .saturating_add(nasiko_config::LAST_REQUEST_STAMP_THROTTLE_SECS)
            .saturating_add(reaper_interval_secs),
    )
}

fn group_by_agent(infos: &[SandboxInfo]) -> BTreeMap<Uuid, Vec<&SandboxInfo>> {
    let mut by_agent: BTreeMap<Uuid, Vec<&SandboxInfo>> = BTreeMap::new();
    for info in infos {
        match info.handle.key {
            SandboxKey::Agent { agent_id } | SandboxKey::AgentContext { agent_id, .. } => {
                by_agent.entry(agent_id).or_default().push(info);
            }
            SandboxKey::Session { .. } => {}
        }
    }
    by_agent
}

/// Releases each of one agent's sandboxes that can no longer be serving a call.
async fn release_leftovers(
    state: &AppState,
    agent_id: Uuid,
    sandboxes: &[&SandboxInfo],
    window: Duration,
) -> Result<Disposition, ReapError> {
    let idle = match last_request(&state.db, agent_id).await? {
        // No live row: nothing routes to the agent at all.
        None => true,
        Some(last_request) => {
            let inflight = state
                .genai_metrics
                .agent_inflight_now(&agent_id.to_string());
            inflight == 0 && idle_for(last_request, sandboxes, window)
        }
    };
    let released: Vec<&&SandboxInfo> = sandboxes
        .iter()
        .filter(|info| idle || info.state != SandboxState::Running)
        .collect();
    if released.is_empty() {
        return Ok(Disposition::Left);
    }
    tracing::info!(%agent_id, count = released.len(), "agent reaper: releasing leftover agent sandboxes");
    for info in released {
        state.sandbox.release(&info.handle).await?;
    }
    Ok(Disposition::Settled)
}

/// The live agent's last request (`Some(None)` when it has served none since it was deployed or
/// last parked); `None` when there is no live agent.
async fn last_request(db: &PgPool, agent_id: Uuid) -> sqlx::Result<Option<Option<DateTime<Utc>>>> {
    sqlx::query_scalar("SELECT last_request_at FROM agents WHERE id = $1 AND deleted_at IS NULL")
        .bind(agent_id)
        .fetch_optional(db)
        .await
}

/// Whether nothing has reached the agent for `window`: measured from its last request, or, when it
/// has served none, from its newest sandbox's start. Unknown counts as not idle.
fn idle_for(
    last_request: Option<DateTime<Utc>>,
    sandboxes: &[&SandboxInfo],
    window: Duration,
) -> bool {
    last_request
        .or_else(|| sandboxes.iter().filter_map(|info| info.started_at).max())
        .and_then(|since| (Utc::now() - since).to_std().ok())
        .is_some_and(|age| age >= window)
}

#[cfg(test)]
mod tests {
    use nasiko_runtime::{SandboxHandle, SandboxProviderKind};

    use super::*;

    const WINDOW: Duration = Duration::from_secs(690);

    fn sandbox(started_ago: Option<i64>) -> SandboxInfo {
        SandboxInfo {
            handle: SandboxHandle {
                key: SandboxKey::Agent {
                    agent_id: Uuid::nil(),
                },
                provider: SandboxProviderKind::Fake,
                backend_ref: "a-0".into(),
            },
            state: SandboxState::Running,
            started_at: started_ago.map(|secs| Utc::now() - chrono::Duration::seconds(secs)),
        }
    }

    fn ago(secs: i64) -> Option<DateTime<Utc>> {
        Some(Utc::now() - chrono::Duration::seconds(secs))
    }

    #[test]
    fn the_call_window_covers_a_call_the_stamp_throttle_and_a_tick() {
        assert_eq!(call_window(600, 30), Duration::from_secs(690));
        assert_eq!(call_window(u64::MAX, 30), Duration::from_secs(u64::MAX));
    }

    #[test]
    fn an_agent_is_idle_once_its_last_request_is_older_than_the_window() {
        let running = sandbox(Some(10));
        assert!(idle_for(ago(700), &[&running], WINDOW));
        assert!(!idle_for(ago(600), &[&running], WINDOW));
    }

    #[test]
    fn a_never_called_agent_is_measured_from_its_newest_sandbox() {
        let old = sandbox(Some(7200));
        let new = sandbox(Some(60));
        assert!(idle_for(None, &[&old], WINDOW));
        assert!(!idle_for(None, &[&old, &new], WINDOW));
    }

    #[test]
    fn an_agent_with_no_known_start_is_not_idle() {
        assert!(!idle_for(None, &[&sandbox(None)], WINDOW));
    }

    #[test]
    fn only_agent_keyed_sandboxes_are_grouped() {
        let agent = sandbox(Some(1));
        let mut session = sandbox(Some(1));
        session.handle.key = SandboxKey::Session {
            session_id: Uuid::new_v4(),
        };
        let infos = [agent, session];
        let grouped = group_by_agent(&infos);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[&Uuid::nil()].len(), 1);
    }
}
