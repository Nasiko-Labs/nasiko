//! Agent parking: scales idle framework agents' own workloads to zero.
//!
//! A framework agent whose parking policy is `auto` runs a workload of its own (a container, a
//! Deployment, a sandbox the runtime suspends in place). Once it is idle, this pass parks it: its
//! workload is scaled to zero (`ContainerRuntime::scale(id, 0)`) and its row marked `parked`, and
//! the next call wakes it (`agent_wake::parked::ParkedAgentWake`). An edition that does not honour
//! the owner's `resident` choice (`AgentScaling::honours_resident_policy`) has every framework
//! agent parked this way, except a superuser's `resident` ones: the platform's own, which every
//! user shares (`agent_scaling::parking_policy_in_force`). `AGENT_PARK_IDLE_SECS=0` turns the pass off (`Config::parks_agents`);
//! agents already parked still wake on their next call. Idle means all of:
//!
//! - nothing has happened to it for `AGENT_PARK_IDLE_SECS`: no request (`last_request_at`) and no
//!   change to its row (`updated_at`, which a deploy or a wake moves), whichever is later;
//! - no request to it in flight from this process (a call on another replica began at most the
//!   stamp throttle after the `last_request_at` read here, and the window is never shorter than a
//!   call plus that throttle plus one tick);
//! - no model call attributed to it within the window (`trace_usage.agent_id`). Its last
//!   request's own calls ran within the call timeout of it, so they are older than the window;
//! - no open pause younger than [`PAUSE_HOLDS_PARK_AT_MOST`]: an A2A task it left open
//!   (`agent_open_tasks`) or a pending HITL request. An older pause no longer holds it up, and a
//!   paused task the agent kept in memory is lost with its workload.
//!
//! CPU is not a signal: these agents do no work between requests.
//!
//! A tick costs one indexed query for the candidates (`idx_agents_parkable_idle`, oldest first),
//! then one for the pauses of all of them and one for their model calls
//! (`idx_trace_usage_agent_materialized`), however large the fleet: an agent that is not idle is
//! never read. A tick parks at most [`MAX_PARKS_PER_TICK`] of the candidates nothing holds up, so a
//! mass redeploy going idle at once cannot hold the session passes behind it; the rest wait for the
//! next ticks. Each park runs in its own transaction holding the agent's advisory
//! lock, the one a wake takes, so a park and a wake of one agent exclude each other: a claim
//! fenced on what was read, the scale to zero, then the commit. A request stamped since the read,
//! a wake, a redeploy or a status change wins the fence.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use nasiko_runtime::{ContainerId, RuntimeError};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::SweepOutcome;
use crate::agent_scaling::OWNER_IS_SUPERUSER_SQL;
use crate::agent_wake::try_lock_agent;
use crate::agents::deployments::record_newest_deployment_status;
use crate::state::AppState;

/// Why parking an agent's workload failed.
#[derive(Debug, thiserror::Error)]
pub enum ParkError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("runtime: {0}")]
    Runtime(#[from] RuntimeError),
}

/// The longest an open pause (an A2A task the agent left open, or a pending HITL request) keeps an
/// idle agent's workload up.
pub const PAUSE_HOLDS_PARK_AT_MOST: Duration = Duration::from_secs(2 * 60 * 60);

/// Parks running at once, each holding one pooled connection while its workload scales down.
const PARK_CONCURRENCY: usize = 4;

/// The most parks one tick attempts. The session passes of the same tick wait behind this one, and
/// a mass redeploy can make thousands of agents idle at once; the rest park on later ticks.
const MAX_PARKS_PER_TICK: usize = 64;

/// What decides whether an idle candidate is parked, already measured.
#[derive(Debug, Clone, Copy, Default)]
struct WorkloadParkInputs {
    /// Requests to the agent in flight from this process right now.
    inflight: u64,
    /// Whether a model call attributed to the agent landed within the idle window.
    had_llm_call: bool,
    /// How old the agent's newest open pause is; `None` when it has none.
    newest_pause_age: Option<Duration>,
}

/// Whether an agent already idle for the window is parked now: nothing in flight, no model call
/// within the window, and no open pause younger than [`PAUSE_HOLDS_PARK_AT_MOST`].
fn should_park_workload(inputs: &WorkloadParkInputs) -> bool {
    inputs.inflight == 0 && !inputs.had_llm_call && !pause_holds_park(inputs.newest_pause_age)
}

fn pause_holds_park(newest_pause_age: Option<Duration>) -> bool {
    newest_pause_age.is_some_and(|age| age < PAUSE_HOLDS_PARK_AT_MOST)
}

/// How one park attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Park {
    /// The workload is at zero and the row says `parked`.
    Parked,
    /// A wake or another park holds the agent's lock; the next tick looks again.
    Locked,
    /// The row moved on since it was read: a request, a wake, a redeploy, a status change.
    Moved,
    /// A request to the agent began in this process since it was read.
    InFlight,
    /// The runtime has no workload for it: nothing to scale down, so the row stays `running`
    /// (startup reconciliation deploys a workload for such a row).
    NoWorkload,
}

/// Parks every idle workload this tick finds. A failure is logged, never fatal to the tick.
pub(super) async fn park_idle_workloads(state: &AppState) -> SweepOutcome {
    let sweep = WorkloadSweep {
        state,
        idle_after: Duration::from_secs(state.config.agent_park_idle_secs),
        policy: PolicyFilter::of(state),
    };
    match sweep.run().await {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::warn!(error = %e, "agent reaper: finding idle agent workloads failed");
            SweepOutcome::Swept {
                suspended: 0,
                skipped: 0,
            }
        }
    }
}

/// Parks one agent's workload if it is still as idle as it was when `read_last_request` was read:
/// in one transaction holding the agent's lock, a fenced claim, the scale to zero, the commit. On
/// any failure the transaction rolls back and the row stays `running`. Exposed so a test can move
/// the row between the read and the park.
pub async fn park_workload(
    state: &AppState,
    agent_id: Uuid,
    read_last_request: Option<DateTime<Utc>>,
) -> Result<Park, ParkError> {
    let idle_after = Duration::from_secs(state.config.agent_park_idle_secs);
    let mut tx = state.db.begin().await?;
    if !try_lock_agent(&mut tx, agent_id).await? {
        return Ok(Park::Locked);
    }
    let policy = PolicyFilter::of(state);
    if !claim(&mut tx, agent_id, read_last_request, idle_after, policy).await? {
        return Ok(Park::Moved);
    }
    if inflight_now(state, agent_id) > 0 {
        return Ok(Park::InFlight);
    }
    let workload = ContainerId::from_uuid(agent_id);
    match state.runtime.scale(&workload, 0).await {
        Ok(()) => {}
        Err(RuntimeError::ContainerNotFound(_)) => return Ok(Park::NoWorkload),
        Err(e) => return Err(e.into()),
    }
    if let Err(e) = tx.commit().await {
        restore_workload(state, &workload).await;
        return Err(e.into());
    }
    Ok(Park::Parked)
}

/// A running, parkable framework agent idle past the window, as the candidate query read it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct Candidate {
    id: Uuid,
    /// The fence: the claim needs it unchanged.
    last_request_at: Option<DateTime<Utc>>,
    /// Seconds since the agent last did anything, for the log.
    idle_secs: i64,
}

/// What the batched checks found for one tick's candidates.
#[derive(Debug, Default)]
struct Activity {
    /// The age of each candidate's newest open pause; absent when it has none.
    pause_ages: HashMap<Uuid, Duration>,
    /// The candidates a model call was attributed to within the window.
    calling: HashSet<Uuid>,
}

impl Activity {
    fn inputs(&self, agent_id: Uuid, inflight: u64) -> WorkloadParkInputs {
        WorkloadParkInputs {
            inflight,
            had_llm_call: self.calling.contains(&agent_id),
            newest_pause_age: self.pause_ages.get(&agent_id).copied(),
        }
    }
}

/// What one tick parks, and what it leaves.
#[derive(Debug)]
struct TickPlan {
    /// The candidates to park now, oldest first.
    parks: Vec<Candidate>,
    /// Candidates something holds up: a call in flight, a model call, a young pause.
    held_up: usize,
    /// Candidates free to park that wait for a later tick.
    deferred: usize,
}

/// The oldest of `candidates` (which come oldest first) that [`should_park_workload`] lets
/// through, at most [`MAX_PARKS_PER_TICK`]. Capped after filtering, so agents that are held up
/// cannot crowd out the ones that are not.
fn plan_tick(
    candidates: Vec<Candidate>,
    activity: &Activity,
    inflight: impl Fn(Uuid) -> u64,
) -> TickPlan {
    let found = candidates.len();
    let mut parks: Vec<Candidate> = candidates
        .into_iter()
        .filter(|c| should_park_workload(&activity.inputs(c.id, inflight(c.id))))
        .collect();
    let held_up = found - parks.len();
    let deferred = parks.len().saturating_sub(MAX_PARKS_PER_TICK);
    parks.truncate(MAX_PARKS_PER_TICK);
    TickPlan {
        parks,
        held_up,
        deferred,
    }
}

/// One tick's settings.
struct WorkloadSweep<'a> {
    state: &'a AppState,
    /// `AGENT_PARK_IDLE_SECS`.
    idle_after: Duration,
    policy: PolicyFilter,
}

impl WorkloadSweep<'_> {
    async fn run(&self) -> sqlx::Result<SweepOutcome> {
        let candidates = idle_candidates(&self.state.db, self.idle_after, self.policy).await?;
        if candidates.is_empty() {
            return Ok(SweepOutcome::Swept {
                suspended: 0,
                skipped: 0,
            });
        }
        let activity = self.measure(&candidates).await?;
        let plan = plan_tick(candidates, &activity, |id| inflight_now(self.state, id));
        let parked = self.park_all(&plan.parks).await;
        if parked > 0 || plan.deferred > 0 {
            tracing::info!(
                parked,
                held_up = plan.held_up,
                deferred = plan.deferred,
                "agent reaper: parked idle agents' workloads"
            );
        }
        Ok(SweepOutcome::Swept {
            suspended: parked,
            skipped: plan.held_up + plan.deferred + (plan.parks.len() - parked),
        })
    }

    /// The batched checks: one query for every candidate's pauses, one for their model calls.
    async fn measure(&self, candidates: &[Candidate]) -> sqlx::Result<Activity> {
        let ids: Vec<Uuid> = candidates.iter().map(|c| c.id).collect();
        Ok(Activity {
            pause_ages: newest_pause_ages(&self.state.db, &ids).await?,
            calling: model_callers(&self.state.db, &ids, self.idle_after).await?,
        })
    }

    /// Parks each candidate, a few at a time; how many were parked.
    async fn park_all(&self, candidates: &[Candidate]) -> usize {
        // Built eagerly: a stream that maps with a closure borrowing `self` is not `Send` for
        // every lifetime, which the spawned reaper loop needs.
        let parks: Vec<_> = candidates.iter().map(|c| self.park_logged(c)).collect();
        let parked: Vec<bool> = futures::stream::iter(parks)
            .buffer_unordered(PARK_CONCURRENCY)
            .collect()
            .await;
        parked.into_iter().filter(|parked| *parked).count()
    }

    /// Parks one candidate and logs how it went; `true` when it was parked.
    async fn park_logged(&self, candidate: &Candidate) -> bool {
        let agent_id = candidate.id;
        match park_workload(self.state, agent_id, candidate.last_request_at).await {
            Ok(Park::Parked) => {
                tracing::info!(
                    %agent_id,
                    idle_secs = candidate.idle_secs,
                    "agent reaper: parked an idle agent's workload"
                );
                true
            }
            Ok(left) => {
                tracing::debug!(%agent_id, outcome = ?left, "agent reaper: left an idle agent's workload running");
                false
            }
            Err(e) => {
                tracing::warn!(%agent_id, error = %e, "agent reaper: parking an idle agent's workload failed; it stays running");
                false
            }
        }
    }
}

fn inflight_now(state: &AppState, agent_id: Uuid) -> u64 {
    state
        .genai_metrics
        .agent_inflight_now(&agent_id.to_string())
}

/// Which framework agents the pass may park: those whose owner left the policy at `auto`, or,
/// where the edition does not honour `resident`, all of them but a superuser's `resident` ones
/// (`agent_scaling::parking_policy_in_force`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyFilter {
    AutoOnly,
    AllButSuperusersResident,
}

impl PolicyFilter {
    fn of(state: &AppState) -> Self {
        if state.agent_scaling.honours_resident_policy() {
            Self::AutoOnly
        } else {
            Self::AllButSuperusersResident
        }
    }

    /// The SQL condition on `agents.parking_policy`, `AND`-ed onto a query's `WHERE`.
    fn condition(self) -> String {
        match self {
            Self::AutoOnly => "AND parking_policy = 'auto'".to_owned(),
            Self::AllButSuperusersResident => {
                format!("AND (parking_policy = 'auto' OR NOT {OWNER_IS_SUPERUSER_SQL})")
            }
        }
    }
}

/// The running, parkable framework agents with a workload image whose last request and last row
/// change are both older than `idle_after`, the longest idle first. With [`PolicyFilter::AutoOnly`]
/// it is spelled exactly as `idx_agents_parkable_idle` (OSS migration 0096), which serves both the
/// filter and the order; [`PolicyFilter::AllButSuperusersResident`] reads the running agents
/// through `idx_agents_status`.
async fn idle_candidates(
    db: &PgPool,
    idle_after: Duration,
    policy: PolicyFilter,
) -> sqlx::Result<Vec<Candidate>> {
    sqlx::query_as(&format!(
        "SELECT id, last_request_at, \
                EXTRACT(EPOCH FROM now() - GREATEST(last_request_at, updated_at))::bigint \
                    AS idle_secs \
         FROM agents \
         WHERE status = 'running' AND workload_kind = 'agent' {} \
           AND deleted_at IS NULL AND image IS NOT NULL \
           AND GREATEST(last_request_at, updated_at) < now() - make_interval(secs => $1) \
         ORDER BY GREATEST(last_request_at, updated_at)",
        policy.condition()
    ))
    .bind(idle_after.as_secs_f64())
    .fetch_all(db)
    .await
}

/// How old each agent's newest open pause is: an A2A task it left open, or a pending HITL
/// request. Agents with neither are absent.
async fn newest_pause_ages(db: &PgPool, ids: &[Uuid]) -> sqlx::Result<HashMap<Uuid, Duration>> {
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT agent_id, EXTRACT(EPOCH FROM now() - max(opened_at))::bigint \
         FROM (SELECT agent_id, updated_at AS opened_at FROM agent_open_tasks \
               WHERE agent_id = ANY($1) \
               UNION ALL \
               SELECT agent_id, created_at FROM hitl_requests \
               WHERE agent_id = ANY($1) AND status = 'pending') pauses \
         GROUP BY agent_id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, secs)| (id, Duration::from_secs(u64::try_from(secs).unwrap_or(0))))
        .collect())
}

/// The agents a model call was attributed to within the last `window`. `trace_usage.agent_id` is
/// the exact agent the router attributed the call to, and every call stamps `materialized_at` (the
/// row's `started_at` is its flow's first call, so it cannot bound the window);
/// `idx_trace_usage_agent_materialized` (OSS migration 0097) keeps the read inside the window.
async fn model_callers(db: &PgPool, ids: &[Uuid], window: Duration) -> sqlx::Result<HashSet<Uuid>> {
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT c.id FROM unnest($1::uuid[]) AS c(id) \
         WHERE EXISTS (SELECT 1 FROM trace_usage tu \
                       WHERE tu.agent_id = c.id \
                         AND tu.materialized_at > now() - make_interval(secs => $2))",
    )
    .bind(ids)
    .bind(window.as_secs_f64())
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Marks the agent parked if, under its lock, it is still a running parkable agent idle for the
/// window, and no request has been stamped since `read_last_request` was read (`IS NOT DISTINCT
/// FROM`, so a never-called agent's NULL fences too). The newest deployment row is stopped with it,
/// as the wake restarts it, and the address goes: no workload serves a parked agent, and the wake
/// writes the new one back.
async fn claim(
    tx: &mut PgConnection,
    agent_id: Uuid,
    read_last_request: Option<DateTime<Utc>>,
    idle_after: Duration,
    policy: PolicyFilter,
) -> sqlx::Result<bool> {
    let claimed = sqlx::query(&format!(
        "UPDATE agents SET status = 'parked', url = NULL, last_request_at = NULL, \
                updated_at = now() \
         WHERE id = $1 AND status = 'running' AND workload_kind = 'agent' {} \
           AND deleted_at IS NULL \
           AND last_request_at IS NOT DISTINCT FROM $2 \
           AND GREATEST(last_request_at, updated_at) < now() - make_interval(secs => $3)",
        policy.condition()
    ))
    .bind(agent_id)
    .bind(read_last_request)
    .bind(idle_after.as_secs_f64())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if claimed {
        record_newest_deployment_status(&mut *tx, agent_id, "stopped").await?;
    }
    Ok(claimed)
}

/// The claim could not be committed after the workload went down, so the row still says
/// `running`: bring the workload back so the row tells the truth. Best effort; a failure is
/// logged.
async fn restore_workload(state: &AppState, workload: &ContainerId) {
    if let Err(e) = state.runtime.scale(workload, 1).await {
        tracing::error!(%workload, error = %e, "agent reaper: a park could not be recorded and the workload could not be brought back");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn candidate(id: Uuid) -> Candidate {
        Candidate {
            id,
            last_request_at: None,
            idle_secs: 7200,
        }
    }

    #[test]
    fn an_edition_that_does_not_honour_resident_still_keeps_a_superusers_resident_agents() {
        assert_eq!(
            PolicyFilter::AutoOnly.condition(),
            "AND parking_policy = 'auto'"
        );
        assert_eq!(
            PolicyFilter::AllButSuperusersResident.condition(),
            format!("AND (parking_policy = 'auto' OR NOT {OWNER_IS_SUPERUSER_SQL})")
        );
    }

    #[test]
    fn an_idle_agent_with_nothing_holding_it_parks() {
        assert!(should_park_workload(&WorkloadParkInputs::default()));
    }

    #[test]
    fn a_call_in_flight_or_a_model_call_holds_the_park() {
        let in_flight = WorkloadParkInputs {
            inflight: 1,
            ..WorkloadParkInputs::default()
        };
        let model_call = WorkloadParkInputs {
            had_llm_call: true,
            ..WorkloadParkInputs::default()
        };
        for held in [in_flight, model_call] {
            assert!(!should_park_workload(&held), "{held:?}");
        }
    }

    #[test]
    fn an_open_pause_holds_the_park_for_two_hours_at_most() {
        let paused = |age: Duration| WorkloadParkInputs {
            newest_pause_age: Some(age),
            ..WorkloadParkInputs::default()
        };
        assert!(!should_park_workload(&paused(Duration::ZERO)));
        assert!(!should_park_workload(&paused(119 * MINUTE)));
        assert!(should_park_workload(&paused(PAUSE_HOLDS_PARK_AT_MOST)));
        assert!(should_park_workload(&paused(3 * 60 * MINUTE)));
        assert_eq!(PAUSE_HOLDS_PARK_AT_MOST, 120 * MINUTE);
    }

    #[test]
    fn only_the_candidates_nothing_holds_up_are_parked() {
        let [free, in_flight, calling, paused, stale_pause]: [Uuid; 5] =
            std::array::from_fn(|_| Uuid::new_v4());
        let activity = Activity {
            pause_ages: HashMap::from([(paused, 30 * MINUTE), (stale_pause, 150 * MINUTE)]),
            calling: HashSet::from([calling]),
        };
        let candidates = [free, in_flight, calling, paused, stale_pause]
            .into_iter()
            .map(candidate)
            .collect();

        let plan = plan_tick(candidates, &activity, |id| u64::from(id == in_flight));

        let kept: Vec<Uuid> = plan.parks.iter().map(|c| c.id).collect();
        assert_eq!(kept, vec![free, stale_pause]);
        assert_eq!((plan.held_up, plan.deferred), (3, 0));
    }

    /// A tick parks the oldest [`MAX_PARKS_PER_TICK`] free candidates. The cap counts only those:
    /// held-up candidates ahead of them in age do not use up the tick.
    #[test]
    fn a_tick_parks_at_most_the_cap_counted_after_the_held_up_are_dropped() {
        let ids: Vec<Uuid> = (0..MAX_PARKS_PER_TICK + 40)
            .map(|_| Uuid::new_v4())
            .collect();
        let held: HashSet<Uuid> = ids[..10].iter().copied().collect();
        let candidates = ids.iter().copied().map(candidate).collect();

        let plan = plan_tick(candidates, &Activity::default(), |id| {
            u64::from(held.contains(&id))
        });

        let kept: Vec<Uuid> = plan.parks.iter().map(|c| c.id).collect();
        assert_eq!(kept, ids[10..10 + MAX_PARKS_PER_TICK].to_vec());
        assert_eq!(plan.held_up, 10);
        assert_eq!(plan.deferred, 30);
    }

    /// A never-called agent (no `last_request_at`) is a candidate like any other, and each keeps
    /// the value it was read with: the claim is fenced on it.
    #[test]
    fn a_candidate_keeps_the_last_request_it_was_read_with() {
        let called = Candidate {
            last_request_at: Some(Utc::now() - chrono::Duration::hours(2)),
            ..candidate(Uuid::new_v4())
        };
        let never_called = candidate(Uuid::new_v4());
        let read: Vec<_> = [&called, &never_called]
            .iter()
            .map(|c| c.last_request_at)
            .collect();

        let plan = plan_tick(
            vec![called.clone(), never_called.clone()],
            &Activity::default(),
            |_| 0,
        );

        let fences: Vec<_> = plan.parks.iter().map(|c| c.last_request_at).collect();
        assert_eq!(fences, read);
        assert_eq!(fences[1], None);
    }
}
