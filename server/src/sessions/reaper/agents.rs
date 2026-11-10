//! The agent half of the reaper: parks framework agents whose sandbox has gone idle.
//!
//! A parkable agent has a sandbox only while it serves (the resolver acquires one on the first
//! request and stamps `agents.last_request_at`). Nothing else ends that, so this sweep asks the
//! provider which `Agent`/`AgentContext` sandboxes exist and, for each agent, either parks it
//! (idle, quiet, no call in flight, no recent model call), or cleans up an inconsistent one
//! (agent gone, or policy flipped to resident). Session sandboxes are the session reaper's.
//!
//! The same knobs govern both: `CODING_AGENT_IDLE_TTL_SECS` (0 turns idle parking off, but a
//! sandbox with no agent row, or a resident agent's, is still released),
//! `SESSION_IDLE_GRACE_SECS`, `SESSION_CPU_QUIESCENT_MILLICORES`, `SESSION_REAPER_INTERVAL_SECS`.
//! The effective TTL is never under `AGENT_CALL_TIMEOUT_SECS` plus one reaper interval: another
//! replica's in-flight call is visible only through `last_request_at`, so a request that began
//! within its timeout must not be parked underneath.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use nasiko_runtime::{RuntimeError, SandboxInfo, SandboxKey, SandboxState};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    Disposition, QuietRun, ReaperMemory, ReaperPolicy, SweepOutcome, SweepScope, Tally,
    sandbox_too_young, seeded_from_age,
};
use crate::agent_scaling::ParkingEligibility;
use crate::agents::open_tasks;
use crate::sessions::suspend::ReapError;
use crate::state::AppState;

/// Everything the park decision turns on, already measured.
#[derive(Debug, Clone, Copy)]
pub struct ParkInputs {
    /// How long since the agent last served a request (or, if it never has, since its sandbox
    /// started).
    pub idle_age: Duration,
    /// The idle TTL.
    pub ttl: Duration,
    /// Requests to the agent in flight from this process right now.
    pub inflight: u64,
    /// How long every sandbox's CPU has been quiet; `None` when any is busy.
    pub quiet_for: Option<Duration>,
    /// How long CPU must stay quiet.
    pub grace: Duration,
    /// Whether a model call attributed to one of the agent's flows landed recently.
    pub had_llm_call: bool,
    /// Non-terminal A2A tasks the agent returned through the proxy and has not closed.
    pub open_tasks: u64,
}

/// Whether an agent that is `running` with live sandboxes should be parked now: idle for the
/// TTL, nothing in flight, CPU quiet for the grace window, no recent model call, and no open
/// A2A task.
pub fn should_park(i: &ParkInputs) -> bool {
    i.idle_age >= i.ttl
        && i.inflight == 0
        && !i.had_llm_call
        && i.open_tasks == 0
        && i.quiet_for.is_some_and(|quiet| quiet >= i.grace)
}

/// The slice of an `agents` row this sweep reads.
#[derive(sqlx::FromRow)]
struct AgentRow {
    status: String,
    url: Option<String>,
    last_request_at: Option<DateTime<Utc>>,
    deleted: bool,
}

/// Delete open-task rows past their bound. Once per tick, ahead of the provider listing, so rows
/// cannot pile up while the provider is unavailable or its `list()` fails. A failure is logged,
/// never fatal.
pub(super) async fn prune_open_tasks(state: &AppState) {
    let max_age = AgentSweep::new(state, SweepScope::RecoveryOnly).open_task_max_age;
    if let Err(e) = open_tasks::prune(&state.db, max_age).await {
        tracing::warn!(error = %e, "reaper: pruning expired open tasks failed");
    }
}

/// Sweep every agent-keyed sandbox in `infos` (the tick's one provider listing).
pub(super) async fn reap_agent_sandboxes(
    state: &AppState,
    memory: &mut ReaperMemory,
    scope: SweepScope,
    infos: &[SandboxInfo],
) -> sqlx::Result<SweepOutcome> {
    let by_agent = group_by_agent(infos);
    let present: HashSet<Uuid> = by_agent.keys().copied().collect();
    memory.agent_runs.retain(|id, _| present.contains(id));

    let sweep = AgentSweep::new(state, scope);
    let mut tally = Tally::default();
    for (agent_id, sandboxes) in by_agent {
        let result = sweep.reap_agent(agent_id, &sandboxes, memory).await;
        // An agent that is simply accounted for (parked with suspended sandboxes, running and
        // busy) is not a "skipped" anything; count only the ones acted on or that failed.
        if matches!(result, Ok(Disposition::Left)) {
            continue;
        }
        if let Err(msg) = tally.record(&agent_id.to_string(), result) {
            return Ok(SweepOutcome::Unsupported(msg));
        }
    }
    Ok(tally.outcome())
}

fn group_by_agent(infos: &[SandboxInfo]) -> BTreeMap<Uuid, Vec<SandboxInfo>> {
    let mut by_agent: BTreeMap<Uuid, Vec<SandboxInfo>> = BTreeMap::new();
    for info in infos {
        match info.handle.key {
            SandboxKey::Agent { agent_id } | SandboxKey::AgentContext { agent_id, .. } => {
                by_agent.entry(agent_id).or_default().push(info.clone());
            }
            SandboxKey::Session { .. } => {}
        }
    }
    by_agent
}

/// One sweep's settings.
struct AgentSweep<'a> {
    state: &'a AppState,
    policy: ReaperPolicy,
    /// The idle TTL, raised to cover an in-flight call on another replica; zero when idle
    /// parking is off.
    ttl: Duration,
    /// How long an open A2A task counts as the agent's work: `AGENT_OPEN_TASK_MAX_AGE_SECS`,
    /// raised to at least one reaper interval past the effective park TTL. The ordering is the
    /// point: park eligibility is measured from `last_request_at`, so a task row that expired
    /// before the TTL did could never block a park.
    open_task_max_age: Duration,
    idle_enabled: bool,
}

impl<'a> AgentSweep<'a> {
    fn new(state: &'a AppState, scope: SweepScope) -> Self {
        let configured = Duration::from_secs(
            u64::try_from(state.config.coding_agent_idle_ttl_secs).unwrap_or(0),
        );
        let interval = Duration::from_secs(state.config.session_reaper_interval_secs);
        let call_window = Duration::from_secs(state.config.agent_call_timeout_secs) + interval;
        let ttl = configured.max(call_window);
        let open_task_max_age =
            Duration::from_secs(state.config.agent_open_task_max_age_secs).max(ttl + interval);
        Self {
            state,
            policy: ReaperPolicy::from_config(&state.config),
            ttl,
            open_task_max_age,
            idle_enabled: scope == SweepScope::Full,
        }
    }

    async fn reap_agent(
        &self,
        agent_id: Uuid,
        sandboxes: &[SandboxInfo],
        memory: &mut ReaperMemory,
    ) -> Result<Disposition, ReapError> {
        let row = load_row(&self.state.db, agent_id).await?;
        let Some(row) = row.filter(|r| !r.deleted) else {
            tracing::warn!(%agent_id, "agent reaper: releasing sandboxes of an agent with no live row");
            return self.release_all(sandboxes).await;
        };
        match self
            .state
            .agent_scaling
            .parking_eligibility(agent_id)
            .await?
        {
            ParkingEligibility::Resident(reason) => {
                self.release_resident(agent_id, &row, reason, sandboxes)
                    .await
            }
            ParkingEligibility::Parkable => {
                self.reap_parkable(agent_id, &row, sandboxes, memory).await
            }
        }
    }

    async fn release_all(&self, sandboxes: &[SandboxInfo]) -> Result<Disposition, ReapError> {
        for info in sandboxes {
            self.state.sandbox.release(&info.handle).await?;
        }
        Ok(Disposition::Settled)
    }

    /// A resident agent should have no sandbox (the policy flipped after it woke). Suspended
    /// ones are released at once; a running one only when nothing is in flight and the agent has
    /// been idle for the grace window, since a demoted or stateful agent may hold a paused task.
    /// The row is marked stopped (the deploy path owns re-creating the workload) unless it has
    /// an address, which belongs to a running workload.
    async fn release_resident(
        &self,
        agent_id: Uuid,
        row: &AgentRow,
        reason: crate::agent_scaling::ResidentReason,
        sandboxes: &[SandboxInfo],
    ) -> Result<Disposition, ReapError> {
        let inflight = self.inflight(agent_id);
        let idle_for_grace = idle_reference(row, sandboxes)
            .and_then(|at| (Utc::now() - at).to_std().ok())
            .is_some_and(|age| age >= self.policy.idle_grace);
        let mut released = Vec::new();
        for info in sandboxes {
            if info.state != SandboxState::Running || (inflight == 0 && idle_for_grace) {
                released.push(info);
            }
        }
        if released.is_empty() {
            return Ok(Disposition::Left);
        }
        tracing::warn!(%agent_id, ?reason, count = released.len(), "agent reaper: a resident agent has a sandbox; releasing it");
        for info in released {
            self.state.sandbox.release(&info.handle).await?;
        }
        sqlx::query(
            "UPDATE agents SET status = 'stopped', updated_at = now() \
             WHERE id = $1 AND status IN ('running', 'parked') AND NULLIF(url, '') IS NULL",
        )
        .bind(agent_id)
        .execute(&self.state.db)
        .await?;
        Ok(Disposition::Settled)
    }

    fn inflight(&self, agent_id: Uuid) -> u64 {
        self.state
            .genai_metrics
            .agent_inflight_now(&agent_id.to_string())
    }

    async fn reap_parkable(
        &self,
        agent_id: Uuid,
        row: &AgentRow,
        sandboxes: &[SandboxInfo],
        memory: &mut ReaperMemory,
    ) -> Result<Disposition, ReapError> {
        match row.status.as_str() {
            "parked" | "stopped" | "failed" => self.suspend_stray(sandboxes).await,
            "running" if self.idle_enabled => {
                self.park_if_idle(agent_id, row, sandboxes, memory).await
            }
            _ => Ok(Disposition::Left),
        }
    }

    /// A row that says the agent is not serving (parked, stopped, failed) while its sandbox is
    /// live is a half-finished park or stop, or a wake that has not stamped its row yet. A
    /// sandbox older than the grace window is the former: suspend it.
    async fn suspend_stray(&self, sandboxes: &[SandboxInfo]) -> Result<Disposition, ReapError> {
        let strays: Vec<&SandboxInfo> = sandboxes
            .iter()
            .filter(|i| is_live(i) && !sandbox_too_young(i, self.policy.idle_grace))
            .collect();
        self.suspend_all(&strays).await?;
        Ok(if strays.is_empty() {
            Disposition::Left
        } else {
            Disposition::Settled
        })
    }

    async fn suspend_all(&self, live: &[&SandboxInfo]) -> Result<(), ReapError> {
        for info in live {
            self.state.sandbox.suspend(&info.handle).await?;
        }
        Ok(())
    }

    async fn park_if_idle(
        &self,
        agent_id: Uuid,
        row: &AgentRow,
        sandboxes: &[SandboxInfo],
        memory: &mut ReaperMemory,
    ) -> Result<Disposition, ReapError> {
        let Some(idle_since) = idle_reference(row, sandboxes) else {
            return Ok(Disposition::Left);
        };
        let idle_age = (Utc::now() - idle_since).to_std().unwrap_or_default();
        if sandboxes.iter().any(|i| i.state == SandboxState::Starting) || idle_age < self.ttl {
            memory.agent_runs.remove(&agent_id);
            return Ok(Disposition::Left);
        }
        let live: Vec<&SandboxInfo> = sandboxes.iter().filter(|i| is_live(i)).collect();
        if live.is_empty() {
            // Idle past the TTL with nothing running: the row is behind the sandbox. A row with
            // an address is a workload's, and is left alone.
            return self.settle_row_only(agent_id, row, memory).await;
        }
        let all_quiet = self.all_quiet(&live).await?;
        let now = Instant::now();
        let quiet_since = note_agent(memory, agent_id, all_quiet, now, idle_age);
        let had_llm_call = match quiet_since {
            Some(_) => llm_call_since(&self.state.db, agent_id, idle_since, self.ttl).await?,
            None => false,
        };
        let mut inputs = ParkInputs {
            idle_age,
            ttl: self.ttl,
            inflight: self.inflight(agent_id),
            quiet_for: quiet_since.map(|since| now.saturating_duration_since(since)),
            grace: self.policy.idle_grace,
            had_llm_call,
            open_tasks: 0,
        };
        if !should_park(&inputs) {
            return Ok(Disposition::Left);
        }
        // Looked up only once every other condition holds: the common case is a busy agent.
        inputs.open_tasks =
            open_tasks::count_open(&self.state.db, agent_id, self.open_task_max_age).await?;
        if !should_park(&inputs) {
            return Ok(Disposition::Left);
        }
        memory.agent_runs.remove(&agent_id);
        if row.url.is_some() {
            // A resident-era workload serves this agent: suspend the idle sandbox, leave the row.
            self.suspend_all(&live).await?;
            return Ok(Disposition::Settled);
        }
        self.park(agent_id, row.last_request_at, &live).await
    }

    /// A `running` row with no live sandbox, idle past the TTL: mark it parked (fenced like any
    /// park). Rows with an address are left alone.
    async fn settle_row_only(
        &self,
        agent_id: Uuid,
        row: &AgentRow,
        memory: &mut ReaperMemory,
    ) -> Result<Disposition, ReapError> {
        if row.url.is_some() || self.inflight(agent_id) > 0 {
            return Ok(Disposition::Left);
        }
        memory.agent_runs.remove(&agent_id);
        let parked = claim_park(&self.state.db, agent_id, row.last_request_at).await?;
        Ok(if parked {
            Disposition::Settled
        } else {
            Disposition::Left
        })
    }

    /// Whether every live sandbox's CPU sample is quiet. A sandbox that cannot be sampled is
    /// treated as busy this round.
    async fn all_quiet(&self, live: &[&SandboxInfo]) -> Result<bool, ReapError> {
        let samples = futures::future::join_all(
            live.iter()
                .map(|info| self.state.sandbox.stats(&info.handle)),
        )
        .await;
        let mut quiet = true;
        for sample in samples {
            match sample {
                Ok(stats) => quiet &= self.policy.is_quiet(&stats),
                Err(RuntimeError::Unsupported(msg)) => return Err(ReapError::Unsupported(msg)),
                Err(_) => quiet = false,
            }
        }
        Ok(quiet)
    }

    /// Claim the park (fenced on the `last_request_at` that was read), then suspend. A request
    /// that arrived since the read wins: the claim misses and the sandbox stays running.
    async fn park(
        &self,
        agent_id: Uuid,
        read_last_request: Option<DateTime<Utc>>,
        live: &[&SandboxInfo],
    ) -> Result<Disposition, ReapError> {
        if !claim_park(&self.state.db, agent_id, read_last_request).await? {
            tracing::debug!(%agent_id, "agent reaper: a request arrived first; left running");
            return Ok(Disposition::Left);
        }
        self.suspend_claimed(agent_id, live).await
    }

    /// After a won claim: re-check that nobody woke the agent, then suspend. A request that
    /// slipped in re-woke the row (so the sandbox is its) or is in flight (the claim is handed
    /// back, with the request's time stamped so the TTL restarts). A wake that lands during the
    /// suspend itself cannot be stopped; it is logged, and the resolver's next `acquire` resumes
    /// the sandbox.
    async fn suspend_claimed(
        &self,
        agent_id: Uuid,
        live: &[&SandboxInfo],
    ) -> Result<Disposition, ReapError> {
        if row_status(&self.state.db, agent_id).await? != "parked" {
            return Ok(Disposition::Left);
        }
        if self.inflight(agent_id) > 0 {
            sqlx::query(
                "UPDATE agents SET status = 'running', last_request_at = now() \
                 WHERE id = $1 AND status = 'parked'",
            )
            .bind(agent_id)
            .execute(&self.state.db)
            .await?;
            return Ok(Disposition::Left);
        }
        self.suspend_all(live).await?;
        if row_status(&self.state.db, agent_id).await? != "parked" {
            tracing::warn!(%agent_id, "agent reaper: the agent was woken while its sandbox was being suspended; its next request resumes it");
            return Ok(Disposition::Left);
        }
        tracing::info!(%agent_id, "agent reaper: parked an idle agent");
        Ok(Disposition::Settled)
    }
}

/// The post-claim step of a park for one agent's live sandboxes: re-check that nobody woke the
/// agent, then suspend. `true` when the sandboxes were suspended and the agent is still parked.
/// Exposed so a test can interleave a wake between the claim and this step.
pub async fn suspend_claimed(
    state: &AppState,
    agent_id: Uuid,
    handles: &[nasiko_runtime::SandboxHandle],
) -> anyhow::Result<bool> {
    let infos: Vec<SandboxInfo> = handles
        .iter()
        .map(|handle| SandboxInfo {
            handle: handle.clone(),
            state: SandboxState::Running,
            started_at: None,
        })
        .collect();
    let live: Vec<&SandboxInfo> = infos.iter().collect();
    let sweep = AgentSweep::new(state, SweepScope::Full);
    Ok(matches!(
        sweep.suspend_claimed(agent_id, &live).await?,
        Disposition::Settled
    ))
}

/// Mark the agent parked, but only if it is still `running` and no request has stamped
/// `last_request_at` since `read_last_request` was read. `true` means this caller owns the park.
///
/// The `agent_deployments` update is the same one `agents::parking::mark_parked` makes (no live
/// deployment row left for the crash guardian to probe); it is repeated here, in one transaction
/// with the fenced claim, because `mark_parked` is unconditional and not fenced.
pub async fn claim_park(
    db: &PgPool,
    agent_id: Uuid,
    read_last_request: Option<DateTime<Utc>>,
) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let claimed = sqlx::query(
        "UPDATE agents SET status = 'parked', url = NULL, last_request_at = NULL, \
                updated_at = now() \
         WHERE id = $1 AND status = 'running' AND deleted_at IS NULL \
           AND last_request_at IS NOT DISTINCT FROM $2",
    )
    .bind(agent_id)
    .bind(read_last_request)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if claimed {
        sqlx::query(
            "UPDATE agent_deployments SET status = 'stopped', updated_at = now() \
             WHERE agent_id = $1 AND status <> 'stopped'",
        )
        .bind(agent_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(claimed)
}

async fn load_row(db: &PgPool, agent_id: Uuid) -> sqlx::Result<Option<AgentRow>> {
    sqlx::query_as(
        "SELECT status, NULLIF(url, '') AS url, last_request_at, deleted_at IS NOT NULL AS deleted \
         FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
}

async fn row_status(db: &PgPool, agent_id: Uuid) -> sqlx::Result<String> {
    sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(db)
        .await
}

/// Whether a model call made by this agent landed after `since` and within the last `ttl`.
/// `trace_usage.agent_id` is the agent the router attributed the call to (exact, and indexed by
/// `idx_trace_usage_agent_id`), and the router stamps `materialized_at` on every call: joining
/// through flow participants would keep every agent a long multi-agent run merely touched awake.
async fn llm_call_since(
    db: &PgPool,
    agent_id: Uuid,
    since: DateTime<Utc>,
    ttl: Duration,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM trace_usage tu \
         WHERE tu.agent_id = $1 \
           AND tu.materialized_at > GREATEST($2, now() - ($3::bigint * interval '1 second')))",
    )
    .bind(agent_id)
    .bind(since)
    .bind(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX))
    .fetch_one(db)
    .await
}

/// When the agent last did anything: its last request, or, if it has never served one, the
/// newest sandbox start (so every sandbox is at least that old). `None` when unknown.
fn idle_reference(row: &AgentRow, sandboxes: &[SandboxInfo]) -> Option<DateTime<Utc>> {
    row.last_request_at
        .or_else(|| sandboxes.iter().filter_map(|i| i.started_at).max())
}

fn is_live(info: &SandboxInfo) -> bool {
    info.state == SandboxState::Running
}

/// Record the agent's CPU sample and return when its quiet run began. A first quiet sighting is
/// taken as quiet since the agent went idle (the grace runs inside the TTL); a busy sample
/// restarts the run.
fn note_agent(
    memory: &mut ReaperMemory,
    agent_id: Uuid,
    quiet: bool,
    now: Instant,
    idle_age: Duration,
) -> Option<Instant> {
    if !quiet {
        memory.agent_runs.insert(agent_id, QuietRun::Busy);
        return None;
    }
    let since = match memory.agent_runs.get(&agent_id) {
        Some(QuietRun::Quiet(since)) => *since,
        Some(QuietRun::Busy) => now,
        None => seeded_from_age(Some(idle_age), now),
    };
    memory.agent_runs.insert(agent_id, QuietRun::Quiet(since));
    Some(since)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> ParkInputs {
        ParkInputs {
            idle_age: Duration::from_secs(3600),
            ttl: Duration::from_secs(60),
            inflight: 0,
            quiet_for: Some(Duration::from_secs(120)),
            grace: Duration::from_secs(2),
            had_llm_call: false,
            open_tasks: 0,
        }
    }

    #[test]
    fn an_idle_quiet_agent_with_nothing_in_flight_parks() {
        assert!(should_park(&ready()));
    }

    #[test]
    fn each_sign_of_life_blocks_the_park() {
        let recent = ParkInputs {
            idle_age: Duration::from_secs(10),
            ..ready()
        };
        let in_flight = ParkInputs {
            inflight: 1,
            ..ready()
        };
        let busy = ParkInputs {
            quiet_for: None,
            ..ready()
        };
        let fresh_quiet = ParkInputs {
            quiet_for: Some(Duration::from_secs(1)),
            ..ready()
        };
        let model_call = ParkInputs {
            had_llm_call: true,
            ..ready()
        };
        for blocked in [recent, in_flight, busy, fresh_quiet, model_call] {
            assert!(!should_park(&blocked), "{blocked:?}");
        }
    }

    #[test]
    fn an_open_task_blocks_the_park() {
        assert!(!should_park(&ParkInputs {
            open_tasks: 1,
            ..ready()
        }));
    }
}
