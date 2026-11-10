//! The idle reaper: suspends harness sandboxes that are doing nothing.
//!
//! A session is a candidate once its last turn ended more than `coding_agent_idle_ttl_secs`
//! ago with no live turn (none, or one whose lease lapsed: a crashed turn). A candidate is
//! suspended only when the sandbox is also quiet by its own account: CPU at or below the
//! quiescent threshold for a full grace window, and no outbound model call since the last
//! turn. (A sandbox its backend is evicting is suspended on any sample, quiet or not, but only
//! when it is a candidate at all; see the TODO on [`should_suspend`].) A harness mid-way through
//! a long tool run or a background model call looks idle to
//! the turn clock but not to those two signals, and suspending it would kill work the user is
//! waiting on. A question the harness asked a person never holds a session here: it lives only as
//! long as its turn, and a live turn already keeps the session out of the candidates. Like every
//! settled suspend, the reaper's closes any question a crashed turn left open (`super::suspend`).
//!
//! Suspending is claim, act, settle: `begin_suspend` moves the row to `stopping` under a
//! deadline, taking the lease and fence from whatever turn held them; the provider is called;
//! and the row is settled as parked under the claim's fence. [`recover_stuck`] re-drives
//! `stopping` rows whose deadline has passed, so a crash mid-suspend never strands a session.
//! It runs even when idle suspension is off, because a claim someone already made must finish.
//!
//! The same loop parks idle agents' workloads ([`workloads`]), under a switch of its own:
//! `AGENT_PARK_IDLE_SECS` (`0` = off), independent of `CODING_AGENT_IDLE_TTL_SECS`, which governs
//! session suspension only.

pub mod agents;
pub mod workloads;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::StreamExt;
use nasiko_runtime::{
    RuntimeError, SandboxHandle, SandboxInfo, SandboxKey, SandboxState, SandboxStats,
};
use tokio::task::JoinHandle;

use super::model::{SandboxLookup, SandboxOwner, SessionRow, SessionStore, owner_of};
use super::suspend::{ReapError, Suspend, claim_suspend, claim_unaccounted, settle_gone};
use crate::state::AppState;

/// How many sandboxes are sampled at once. Each sample is a provider round trip (Docker
/// stats take about a second), so a serial sweep over many sessions would outlast the tick.
const REAPER_STATS_CONCURRENCY: usize = 8;

/// How many reaper intervals to wait between sweeps once the provider has said it cannot
/// serve sessions at all.
const UNSUPPORTED_BACKOFF_INTERVALS: u32 = 10;

/// The thresholds [`should_suspend`] applies.
#[derive(Debug, Clone, Copy)]
pub struct ReaperPolicy {
    /// CPU at or below this many millicores counts as quiet.
    pub cpu_quiescent_millicores: u32,
    /// How long CPU must stay quiet before the sandbox is suspended.
    pub idle_grace: Duration,
}

impl ReaperPolicy {
    /// The policy the reaper settings describe; one construction shared by the session passes.
    pub fn from_config(config: &nasiko_config::Config) -> Self {
        Self {
            cpu_quiescent_millicores: config.session_cpu_quiescent_millicores,
            idle_grace: Duration::from_secs(config.session_idle_grace_secs),
        }
    }

    /// Whether this sample counts as quiet.
    pub fn is_quiet(&self, stats: &SandboxStats) -> bool {
        stats.cpu_millicores <= self.cpu_quiescent_millicores
    }
}

/// Whether one idle candidate should be suspended now.
///
/// `first_quiet_at` is when the sandbox was first seen quiet in this unbroken run of quiet
/// samples (`None` when it has not been seen quiet yet). Unless the sandbox is evicting, a
/// busy sample never suspends, whatever `first_quiet_at` says.
pub fn should_suspend(
    stats: &SandboxStats,
    first_quiet_at: Option<Instant>,
    now: Instant,
    policy: ReaperPolicy,
    had_llm_call: bool,
) -> bool {
    // TODO(eviction-sweep): this only helps a sandbox that is already a candidate, i.e.
    // idle past `coding_agent_idle_ttl_secs` (`sweep_idle` lists only those). A recently active
    // session on an evicting worker is not sampled; a separate sweep would cover it.
    // An evicting sandbox is suspended whenever it is sampled, busy or not.
    if stats.evicting {
        return true;
    }
    if !policy.is_quiet(stats) {
        return false;
    }
    let Some(quiet_since) = first_quiet_at else {
        return false;
    };
    now.saturating_duration_since(quiet_since) >= policy.idle_grace && !had_llm_call
}

/// Whether a sample is too old to act on. An evicting sample never is: the drain will not wait.
fn is_stale(stats: &SandboxStats, interval: Duration) -> bool {
    if stats.evicting {
        return false;
    }
    (Utc::now() - stats.sampled_at)
        .to_std()
        .is_ok_and(|age| age > interval)
}

/// Where a session is in its current run of CPU samples.
#[derive(Clone, Copy)]
enum QuietRun {
    /// Quiet since this instant.
    Quiet(Instant),
    /// The last sample was busy; the run restarts at the next quiet one.
    Busy,
}

/// What the reaper remembers between sweeps.
#[derive(Default)]
pub struct ReaperMemory {
    /// Each `(session, turn fence)`'s current CPU run. The fence in the key means a session that
    /// took a turn in between starts over.
    runs: HashMap<(String, i64), QuietRun>,
    /// Sessions whose unusable `sandbox_ref` has already been warned about.
    warned: HashSet<String>,
}

impl ReaperMemory {
    /// Record this sample and return when the session's quiet run began (`None` if busy).
    ///
    /// A session seen quiet for the first time is taken to have been quiet since it went idle
    /// (`idle_since`): the grace window is measured inside the idle TTL, so a session idle for
    /// the whole TTL needs no further wait. After a busy sample the run restarts at the next
    /// quiet one, so a sandbox that just worked is not suspended on that evidence.
    fn note(&mut self, session: &SessionRow, quiet: bool, now: Instant) -> Option<Instant> {
        let key = (session.session_id.clone(), session.turn_fence);
        if !quiet {
            self.runs.insert(key, QuietRun::Busy);
            return None;
        }
        let since = match self.runs.get(&key) {
            Some(QuietRun::Quiet(since)) => *since,
            Some(QuietRun::Busy) => now,
            None => seeded_from_idle(session, now),
        };
        self.runs.insert(key, QuietRun::Quiet(since));
        Some(since)
    }

    /// Forget sessions that are no longer candidates, so each starts over next time.
    fn retain_candidates(&mut self, candidates: &[SessionRow]) {
        let keys: HashSet<(&str, i64)> = candidates
            .iter()
            .map(|c| (c.session_id.as_str(), c.turn_fence))
            .collect();
        self.runs
            .retain(|(id, fence), _| keys.contains(&(id.as_str(), *fence)));
        let ids: HashSet<&str> = candidates.iter().map(|c| c.session_id.as_str()).collect();
        self.warned.retain(|id| ids.contains(id.as_str()));
    }

    /// True the first time a session's bad ref is reported, false after.
    fn first_warning(&mut self, session_id: &str) -> bool {
        self.warned.insert(session_id.to_owned())
    }
}

/// The instant a session's idle time began, as an `Instant` (`now` when unknown or in the future).
fn seeded_from_idle(session: &SessionRow, now: Instant) -> Instant {
    let age = session
        .idle_since()
        .and_then(|since| (Utc::now() - since).to_std().ok());
    seeded_from_age(age, now)
}

/// `now` minus `age` (`now` when the age is unknown or would underflow the clock).
fn seeded_from_age(age: Option<Duration>, now: Instant) -> Instant {
    age.and_then(|age| now.checked_sub(age)).unwrap_or(now)
}

/// The result of one sweep.
#[derive(Debug, PartialEq, Eq)]
pub enum SweepOutcome {
    /// The sweep ran: how many sandboxes it suspended or settled, and how many it left alone.
    Swept { suspended: usize, skipped: usize },
    /// The wired provider cannot run sandboxes at all; the sweep stopped at the first one.
    Unsupported(String),
}

/// How much of a sweep's session work to run. Agent parking has its own switch
/// (`AGENT_PARK_IDLE_SECS`), whichever this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepScope {
    /// Recovery plus suspending idle sessions.
    Full,
    /// Only recovery and clean-up (stuck claims, orphaned sandboxes, leftover agent sandboxes):
    /// idle session suspension is off, but work already begun, or a sandbox nothing accounts for,
    /// must still be dealt with.
    RecoveryOnly,
}

/// Run the reaper for the life of the process. Each sweep is independent: a failure on one
/// session or one sweep is logged and the loop carries on. With an idle TTL of 0 (never suspend
/// sessions by idleness) only session recovery and clean-up run; agent parking runs whenever
/// `AGENT_PARK_IDLE_SECS` is not 0. A provider that cannot serve sessions is reported once and the
/// loop then backs off to a tenth of the rate until a sweep succeeds.
pub fn spawn_reaper(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let scope = if state.config.coding_agent_idle_ttl_secs > 0 {
            SweepScope::Full
        } else {
            tracing::info!(
                "session reaper: idle session suspension is off (CODING_AGENT_IDLE_TTL_SECS=0); \
                 recovering stuck suspends and orphaned sandboxes only"
            );
            SweepScope::RecoveryOnly
        };
        if !state.config.parks_agents() {
            tracing::info!("agent reaper: agent parking is off (AGENT_PARK_IDLE_SECS=0)");
        }
        let interval = Duration::from_secs(state.config.session_reaper_interval_secs.max(1));
        let mut memory = ReaperMemory::default();
        let mut reported_unsupported = false;
        loop {
            tokio::time::sleep(interval).await;
            match sweep(&state, &mut memory, scope).await {
                Ok(SweepOutcome::Swept { .. }) => reported_unsupported = false,
                Ok(SweepOutcome::Unsupported(msg)) => {
                    if !reported_unsupported {
                        tracing::warn!(%msg, "session reaper: sandbox provider cannot serve sessions; backing off");
                        reported_unsupported = true;
                    }
                    tokio::time::sleep(interval * (UNSUPPORTED_BACKOFF_INTERVALS - 1)).await;
                }
                Err(e) => tracing::warn!(error = %e, "session reaper: listing candidates failed"),
            }
        }
    })
}

/// One tick of the reaper loop: finish stuck suspends, then (with the provider listed once) reap
/// orphaned session sandboxes, suspend idle sessions if the scope is full, and release leftover
/// agent sandboxes; then, unless `AGENT_PARK_IDLE_SECS` is 0, park idle framework agents' own
/// workloads, whatever the scope. The workload pass runs whatever the sandbox provider said, since
/// it never asks it.
pub async fn sweep(
    state: &AppState,
    memory: &mut ReaperMemory,
    scope: SweepScope,
) -> sqlx::Result<SweepOutcome> {
    agents::prune_open_tasks(state).await;
    let sandboxes = sweep_sandboxes(state, memory, scope).await?;
    if !state.config.parks_agents() {
        return Ok(sandboxes);
    }
    let workloads = workloads::park_idle_workloads(state).await;
    // The sandbox outcome second, so a provider that cannot serve still backs the loop off.
    Ok(merge(workloads, sandboxes))
}

async fn sweep_sandboxes(
    state: &AppState,
    memory: &mut ReaperMemory,
    scope: SweepScope,
) -> sqlx::Result<SweepOutcome> {
    // A composite provider whose session side is down (a provider configuration error at boot) still
    // lists agent sandboxes: skip the session passes, which would report `Unsupported` and stop the
    // agent pass and back the whole reaper off.
    let sessions_up = state.sandbox.is_available_for(&SandboxKey::Session {
        session_id: uuid::Uuid::nil(),
    });
    if !sessions_up {
        tracing::debug!("session reaper: session sandboxes unavailable; skipping session passes");
    }
    let mut outcome = SweepOutcome::Swept {
        suspended: 0,
        skipped: 0,
    };
    if sessions_up {
        outcome = recover_stuck(state, memory).await?;
        if matches!(outcome, SweepOutcome::Unsupported(_)) {
            return Ok(outcome);
        }
    }
    let infos = match state.sandbox.list().await {
        Ok(infos) => Some(infos),
        Err(RuntimeError::Unsupported(msg)) => {
            return Ok(SweepOutcome::Unsupported(msg));
        }
        Err(e) => {
            tracing::warn!(error = %e, "session reaper: listing sandboxes failed; skipping the orphan and agent passes");
            None
        }
    };
    if sessions_up && let Some(infos) = &infos {
        outcome = merge(outcome, orphans_in(state, infos).await?);
    }
    if sessions_up && scope == SweepScope::Full && !matches!(outcome, SweepOutcome::Unsupported(_))
    {
        outcome = merge(outcome, sweep_idle(state, memory).await?);
    }
    if let Some(infos) = &infos
        && !matches!(outcome, SweepOutcome::Unsupported(_))
    {
        outcome = merge(
            outcome,
            agents::release_leftover_sandboxes(state, infos).await?,
        );
    }
    Ok(outcome)
}

/// A candidate with a usable handle.
struct Candidate {
    session: SessionRow,
    handle: SandboxHandle,
}

/// A candidate with the provider's answer to a stats sample.
struct Sampled {
    candidate: Candidate,
    stats: Result<SandboxStats, RuntimeError>,
}

/// What a sweep did with one session.
enum Disposition {
    /// Suspended, or settled to match what the provider reported.
    Settled,
    /// Left alone: not idle long enough, busy, lost the claim, or stale data.
    Left,
}

/// The running count of one sweep.
#[derive(Default)]
struct Tally {
    acted: usize,
    skipped: usize,
}

impl Tally {
    /// Fold one session's result in, logging a failure once here at the boundary; `Err`
    /// carries the message that stops the sweep (an unsupported provider).
    fn record(
        &mut self,
        session_id: &str,
        result: Result<Disposition, ReapError>,
    ) -> Result<(), String> {
        match result {
            Ok(Disposition::Settled) => self.acted += 1,
            Ok(Disposition::Left) => self.skipped += 1,
            Err(ReapError::Unsupported(msg)) => return Err(msg),
            Err(e) => {
                self.skipped += 1;
                tracing::warn!(%session_id, error = %e, "session reaper: skipped");
            }
        }
        Ok(())
    }

    fn outcome(self) -> SweepOutcome {
        SweepOutcome::Swept {
            suspended: self.acted,
            skipped: self.skipped,
        }
    }
}

/// One sweep's settings and handles, so the per-session steps take few parameters.
struct Reaper<'a> {
    state: &'a AppState,
    store: SessionStore,
    policy: ReaperPolicy,
    interval: Duration,
}

impl<'a> Reaper<'a> {
    fn new(state: &'a AppState) -> Self {
        Self {
            state,
            store: SessionStore::new(state.db.clone()),
            policy: ReaperPolicy::from_config(&state.config),
            interval: Duration::from_secs(state.config.session_reaper_interval_secs.max(1)),
        }
    }
}

/// The part of a sweep that runs whatever the idle TTL: finish stuck suspends, then reap
/// orphaned sandboxes.
pub async fn recover(state: &AppState, memory: &mut ReaperMemory) -> sqlx::Result<SweepOutcome> {
    let stuck = recover_stuck(state, memory).await?;
    if matches!(stuck, SweepOutcome::Unsupported(_)) {
        return Ok(stuck);
    }
    let orphans = recover_orphans(state).await?;
    Ok(merge(stuck, orphans))
}

/// Add two sweeps' counts; an unsupported provider in the second wins.
fn merge(first: SweepOutcome, second: SweepOutcome) -> SweepOutcome {
    match (first, second) {
        (
            SweepOutcome::Swept {
                suspended: a,
                skipped: b,
            },
            SweepOutcome::Swept { suspended, skipped },
        ) => SweepOutcome::Swept {
            suspended: a + suspended,
            skipped: b + skipped,
        },
        (_, other) => other,
    }
}

/// One full sweep: recover stuck suspends and orphans, then suspend idle sandboxes. A listing
/// failure is an `Err`, not an empty sweep.
pub async fn reap_once(state: &AppState, memory: &mut ReaperMemory) -> sqlx::Result<SweepOutcome> {
    let recovered = recover(state, memory).await?;
    if matches!(recovered, SweepOutcome::Unsupported(_)) {
        return Ok(recovered);
    }
    let idle = sweep_idle(state, memory).await?;
    Ok(merge(recovered, idle))
}

/// Re-drive `stopping` sessions whose claim deadline has passed: take the claim over and
/// finish the suspend. Independent of the idle TTL.
pub async fn recover_stuck(
    state: &AppState,
    memory: &mut ReaperMemory,
) -> sqlx::Result<SweepOutcome> {
    let reaper = Reaper::new(state);
    let rows = reaper.store.list_stuck().await?;
    let wired = state.sandbox.kind();
    let (usable, unusable): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|r| r.handle(wired).is_ok());
    let mut tally = Tally::default();
    for session in unusable {
        let id = session.session_id.clone();
        let result = reaper.clear_unusable(&session).await;
        if let Err(msg) = tally.record(&id, result) {
            return Ok(SweepOutcome::Unsupported(msg));
        }
    }
    for candidate in reaper.with_handles(usable, memory) {
        let id = candidate.session.session_id.clone();
        let result = reaper.recover_one(candidate).await;
        if let Err(msg) = tally.record(&id, result) {
            return Ok(SweepOutcome::Unsupported(msg));
        }
    }
    Ok(tally.outcome())
}

/// The youngest a sandbox may be and still count as an orphan, however small the idle grace.
/// A first turn records its sandbox only after `acquire` returns, and a pod's `acquire` has a
/// 120 s budget; 180 s covers that with margin, so a sandbox mid-start is never mistaken for an
/// orphan.
const ORPHAN_MIN_AGE_FLOOR_SECS: u64 = 180;

/// Reap sandboxes the database does not account for, found by asking the provider what exists.
///
/// A harness sandbox should be backed by a live session row that knows it, by the key recorded
/// before the sandbox was created, the ref recorded after, or (for UUID session ids) the id.
/// When no row claims it, or the row is deleted, the sandbox is released outright (warned, with
/// its key); when the row is parked, or running without a recorded sandbox, while the sandbox is
/// live and no turn holds the row, the row is claimed and the sandbox suspended and recorded.
/// Young sandboxes are left alone (see [`orphan_min_age`] and [`sandbox_too_young`]). All rows
/// are looked up in one query. Only session sandboxes are considered; agent-keyed ones belong
/// to [`agents`].
pub async fn recover_orphans(state: &AppState) -> sqlx::Result<SweepOutcome> {
    match state.sandbox.list().await {
        Ok(infos) => orphans_in(state, &infos).await,
        Err(RuntimeError::Unsupported(msg)) => Ok(SweepOutcome::Unsupported(msg)),
        Err(e) => {
            tracing::warn!(error = %e, "session reaper: listing sandboxes failed; skipping the orphan pass");
            Ok(SweepOutcome::Swept {
                suspended: 0,
                skipped: 0,
            })
        }
    }
}

/// The orphan pass over an already-listed set of sandboxes.
async fn orphans_in(state: &AppState, infos: &[SandboxInfo]) -> sqlx::Result<SweepOutcome> {
    let min_age = orphan_min_age(state.config.session_idle_grace_secs);
    let candidates: Vec<(SandboxInfo, SandboxLookup)> = infos
        .iter()
        .filter(|info| !sandbox_too_young(info, min_age))
        .filter_map(|info| {
            let SandboxKey::Session { session_id } = info.handle.key else {
                return None;
            };
            let lookup = SandboxLookup {
                key_name: info.handle.key.name(),
                sandbox_ref: info.handle.sandbox_ref(),
                key_uuid: session_id,
            };
            Some((info.clone(), lookup))
        })
        .collect();
    let reaper = Reaper::new(state);
    let lookups: Vec<SandboxLookup> = candidates.iter().map(|(_, l)| l.clone()).collect();
    let owners = reaper.store.find_for_sandboxes(&lookups).await?;
    let mut tally = Tally::default();
    for (info, lookup) in &candidates {
        let result = reaper.reap_orphan(info, owner_of(&owners, lookup)).await;
        // A sandbox that is simply accounted for is not a "skipped" anything; count only the
        // ones acted on or that failed.
        if matches!(result, Ok(Disposition::Left)) {
            continue;
        }
        if let Err(msg) = tally.record(&lookup.key_name, result) {
            return Ok(SweepOutcome::Unsupported(msg));
        }
    }
    Ok(tally.outcome())
}

/// The minimum age for an orphan: the idle grace, but never under
/// [`ORPHAN_MIN_AGE_FLOOR_SECS`].
fn orphan_min_age(idle_grace_secs: u64) -> Duration {
    Duration::from_secs(idle_grace_secs.max(ORPHAN_MIN_AGE_FLOOR_SECS))
}

/// Whether a sandbox is too young to judge. One with no reported start time is young while it
/// is Starting or Running (a backend that does not say when it started cannot prove age); only
/// a Suspended or unknown-state one is old without a start time.
pub(super) fn sandbox_too_young(info: &SandboxInfo, min_age: Duration) -> bool {
    match info.started_at {
        Some(at) => (Utc::now() - at).to_std().is_ok_and(|age| age < min_age),
        None => matches!(info.state, SandboxState::Starting | SandboxState::Running),
    }
}

/// Sample every idle candidate concurrently, then decide and act on each in turn. Acting
/// stays sequential so `memory` is only touched here.
pub async fn sweep_idle(state: &AppState, memory: &mut ReaperMemory) -> sqlx::Result<SweepOutcome> {
    let reaper = Reaper::new(state);
    // TODO(eviction-sweep): `list_idle` returns only sessions idle past the TTL, so an
    // evicting sandbox of a recently active session is not seen here (see `should_suspend`).
    let rows = reaper
        .store
        .list_idle(state.config.coding_agent_idle_ttl_secs)
        .await?;
    memory.retain_candidates(&rows);
    let candidates = reaper.with_handles(rows, memory);
    let mut tally = Tally::default();
    for sampled in reaper.sample_all(candidates).await {
        let id = sampled.candidate.session.session_id.clone();
        let result = reaper.reap_session(sampled, memory).await;
        if let Err(msg) = tally.record(&id, result) {
            return Ok(SweepOutcome::Unsupported(msg));
        }
    }
    Ok(tally.outcome())
}

impl Reaper<'_> {
    /// Pair each row with its handle; a row whose `sandbox_ref` is unusable is skipped, with
    /// a warning the first time and a debug line after.
    fn with_handles(&self, rows: Vec<SessionRow>, memory: &mut ReaperMemory) -> Vec<Candidate> {
        let wired = self.state.sandbox.kind();
        rows.into_iter()
            .filter_map(|session| match session.handle(wired) {
                Ok(handle) => Some(Candidate { session, handle }),
                Err(reason) => {
                    if memory.first_warning(&session.session_id) {
                        tracing::warn!(session_id = %session.session_id, %reason, "session reaper: skipped");
                    } else {
                        tracing::debug!(session_id = %session.session_id, %reason, "session reaper: skipped");
                    }
                    None
                }
            })
            .collect()
    }

    async fn sample_all(&self, candidates: Vec<Candidate>) -> Vec<Sampled> {
        futures::stream::iter(
            candidates
                .into_iter()
                .map(|candidate| self.sample_one(candidate)),
        )
        .buffer_unordered(REAPER_STATS_CONCURRENCY)
        .collect()
        .await
    }

    async fn sample_one(&self, candidate: Candidate) -> Sampled {
        let stats = self.state.sandbox.stats(&candidate.handle).await;
        Sampled { candidate, stats }
    }

    /// Decide what one listed sandbox needs, from the row that claims it (if any).
    async fn reap_orphan(
        &self,
        info: &SandboxInfo,
        owner: Option<&SandboxOwner>,
    ) -> Result<Disposition, ReapError> {
        let live = matches!(info.state, SandboxState::Running | SandboxState::Starting);
        match owner {
            None => self.release_orphan(info).await,
            Some(o) if o.deleted => self.release_orphan(info).await,
            Some(o) if live && is_unaccounted(&o.session) => {
                self.suspend_unaccounted(&o.session, &info.handle).await
            }
            Some(_) => Ok(Disposition::Left),
        }
    }

    /// Release a sandbox no live session owns.
    async fn release_orphan(&self, info: &SandboxInfo) -> Result<Disposition, ReapError> {
        tracing::warn!(key = %info.handle.key.name(), state = ?info.state, "session reaper: releasing an orphaned sandbox with no session row");
        self.state.sandbox.release(&info.handle).await?;
        Ok(Disposition::Settled)
    }

    /// Suspend a live sandbox whose session row says nothing is using it.
    async fn suspend_unaccounted(
        &self,
        session: &SessionRow,
        handle: &SandboxHandle,
    ) -> Result<Disposition, ReapError> {
        let Some(claim) = claim_unaccounted(&self.store, session, handle).await? else {
            return Ok(Disposition::Left);
        };
        tracing::warn!(session_id = %session.session_id, state = %session.runtime_state, "session reaper: suspending a live sandbox its session row does not account for");
        let settled = Suspend::new(self.state, &self.store, session, handle)
            .finish(claim)
            .await?;
        Ok(if settled {
            Disposition::Settled
        } else {
            Disposition::Left
        })
    }

    /// A stuck `stopping` row whose `sandbox_ref` is unusable (malformed, or another provider's)
    /// can never be suspended by this build; clear it so the session is usable and the sweep
    /// stops reporting it.
    async fn clear_unusable(&self, session: &SessionRow) -> Result<Disposition, ReapError> {
        let Some(claim) = self
            .store
            .reclaim_suspend(&session.session_id, session.turn_fence)
            .await?
        else {
            return Ok(Disposition::Left);
        };
        tracing::warn!(session_id = %session.session_id, "session reaper: stuck suspend has an unusable sandbox_ref; clearing it");
        let cleared = settle_gone(self.state, &self.store, session, claim).await?;
        Ok(if cleared {
            Disposition::Settled
        } else {
            Disposition::Left
        })
    }

    /// Re-drive one stuck `stopping` row: take the claim over (bumping the fence, which cuts
    /// off the previous claimant), then suspend and settle under the new fence.
    async fn recover_one(&self, candidate: Candidate) -> Result<Disposition, ReapError> {
        let Candidate { session, handle } = candidate;
        let Some(claim) = self
            .store
            .reclaim_suspend(&session.session_id, session.turn_fence)
            .await?
        else {
            return Ok(Disposition::Left);
        };
        tracing::info!(session_id = %session.session_id, "session reaper: recovering stuck suspend");
        let settled = Suspend::new(self.state, &self.store, &session, &handle)
            .finish(claim)
            .await?;
        Ok(if settled {
            Disposition::Settled
        } else {
            Disposition::Left
        })
    }

    async fn reap_session(
        &self,
        sampled: Sampled,
        memory: &mut ReaperMemory,
    ) -> Result<Disposition, ReapError> {
        let Sampled {
            candidate: Candidate { session, handle },
            stats,
        } = sampled;
        let stats = match stats {
            Ok(stats) => stats,
            Err(e) => return self.reconcile_sample_error(&session, &handle, e).await,
        };
        if is_stale(&stats, self.interval) {
            tracing::debug!(session_id = %session.session_id, "session reaper: stale stats sample; skipped");
            return Ok(Disposition::Left);
        }
        if stats.evicting {
            // Its backend is draining it: no quiet run or model-call check applies.
            let disposition = self.suspend_fenced(&session, &handle).await?;
            if matches!(disposition, Disposition::Left) {
                tracing::warn!(session_id = %session.session_id, "session reaper: evicting sandbox not suspended (a turn holds it or the suspend did not settle)");
            }
            return Ok(disposition);
        }
        let now = Instant::now();
        let quiet_since = memory.note(&session, self.policy.is_quiet(&stats), now);
        let had_llm_call = match quiet_since {
            Some(_) => self.llm_call_since_idle(&session).await?,
            None => false,
        };
        if !should_suspend(&stats, quiet_since, now, self.policy, had_llm_call) {
            return Ok(Disposition::Left);
        }
        self.suspend_fenced(&session, &handle).await
    }

    /// Suspend only if no turn started since the candidate list was read: claim the session
    /// with `begin_suspend` on the fence read then, and call the provider only on a win.
    async fn suspend_fenced(
        &self,
        session: &SessionRow,
        handle: &SandboxHandle,
    ) -> Result<Disposition, ReapError> {
        let Some(claim) = claim_suspend(&self.store, session).await? else {
            tracing::debug!(session_id = %session.session_id, "session reaper: lost the race to a turn");
            return Ok(Disposition::Left);
        };
        let settled = Suspend::new(self.state, &self.store, session, handle)
            .finish(claim)
            .await?;
        if !settled {
            return Ok(Disposition::Left);
        }
        let idle_secs = session
            .idle_since()
            .map(|t| (Utc::now() - t).num_seconds())
            .unwrap_or_default();
        tracing::info!(session_id = %session.session_id, idle_secs, "session reaper: suspended sandbox");
        Ok(Disposition::Settled)
    }

    /// Settle a session after a stats sample failed.
    ///
    /// `SandboxNotFound`: the sandbox is gone, so claim and forget it. `SandboxNotRunning`: it
    /// is not running, which includes a half-started one, so it takes the normal suspend path
    /// rather than being assumed stopped. `Unsupported` stops the sweep; anything else is
    /// skipped.
    async fn reconcile_sample_error(
        &self,
        session: &SessionRow,
        handle: &SandboxHandle,
        error: RuntimeError,
    ) -> Result<Disposition, ReapError> {
        match error {
            RuntimeError::SandboxNotFound(_) => {
                let Some(claim) = claim_suspend(&self.store, session).await? else {
                    return Ok(Disposition::Left);
                };
                if !settle_gone(self.state, &self.store, session, claim).await? {
                    return Ok(Disposition::Left);
                }
                tracing::info!(session_id = %session.session_id, "session reaper: sandbox gone; cleared");
                Ok(Disposition::Settled)
            }
            RuntimeError::SandboxNotRunning(_) => self.suspend_fenced(session, handle).await,
            other => Err(other.into()),
        }
    }

    /// Whether the router logged a model call for *this session* recently enough to count as a
    /// sign of life: after the session went idle (see [`SessionRow::idle_since`]) and within the
    /// last idle TTL. The TTL bound keeps one stale row (a crashed turn's own call, a usage row
    /// written after a turn ended) from pinning the session until its next turn.
    ///
    /// The router upserts one `trace_usage` row per flow and agent (`trace_id` is the flow id the
    /// call was attributed to) and stamps `materialized_at` on every call; `flows.session_id` is
    /// the chat session that flow belongs to, so joining the two counts only this session's
    /// calls: a busy sibling session of the same user on the same harness agent does not keep an
    /// idle one alive. (`token_usage`, the per-call log this used to read, is retired.)
    ///
    /// TODO NAS-1100: the MCP gateway persists no per-call rows, so outbound tool calls are
    /// not part of this signal yet.
    async fn llm_call_since_idle(&self, session: &SessionRow) -> Result<bool, ReapError> {
        let Some(since) = session.idle_since() else {
            return Ok(false);
        };
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM trace_usage tu \
             JOIN flows f ON f.flow_id = tu.trace_id \
             WHERE f.session_id = $1 \
               AND tu.materialized_at > GREATEST($2, now() - ($3::bigint * interval '1 second')))",
        )
        .bind(&session.session_id)
        .bind(since)
        .bind(self.state.config.coding_agent_idle_ttl_secs)
        .fetch_one(&self.state.db)
        .await?)
    }
}

/// Whether a session row says no turn is using its sandbox although the sandbox is live: parked,
/// or running or starting with no sandbox recorded, and no live lease.
fn is_unaccounted(session: &SessionRow) -> bool {
    let lease_live =
        session.turn_owner.is_some() && session.lease_expires_at.is_some_and(|at| at > Utc::now());
    let recorded = session.sandbox_ref.is_some();
    !lease_live
        && (session.runtime_state == "parked"
            || (matches!(session.runtime_state.as_str(), "running" | "starting") && !recorded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_runtime::SandboxProviderKind;

    fn info(state: SandboxState, started_at: Option<chrono::DateTime<Utc>>) -> SandboxInfo {
        SandboxInfo {
            handle: SandboxHandle {
                key: SandboxKey::Session {
                    session_id: uuid::Uuid::nil(),
                },
                provider: SandboxProviderKind::Fake,
                backend_ref: "x".into(),
            },
            state,
            started_at,
        }
    }

    #[test]
    fn the_orphan_age_floor_beats_a_small_grace() {
        assert_eq!(orphan_min_age(0), Duration::from_secs(180));
        assert_eq!(orphan_min_age(120), Duration::from_secs(180));
        assert_eq!(orphan_min_age(600), Duration::from_secs(600));
    }

    #[test]
    fn an_unreported_start_time_is_young_unless_the_sandbox_is_suspended() {
        let min = orphan_min_age(0);
        assert!(sandbox_too_young(&info(SandboxState::Starting, None), min));
        assert!(sandbox_too_young(&info(SandboxState::Running, None), min));
        assert!(!sandbox_too_young(
            &info(SandboxState::Suspended, None),
            min
        ));
    }

    #[test]
    fn a_reported_start_time_decides_by_age() {
        let min = orphan_min_age(0);
        let recent = Utc::now() - chrono::Duration::seconds(30);
        let old = Utc::now() - chrono::Duration::seconds(3600);
        assert!(sandbox_too_young(
            &info(SandboxState::Running, Some(recent)),
            min
        ));
        assert!(!sandbox_too_young(
            &info(SandboxState::Running, Some(old)),
            min
        ));
    }

    const POLICY: ReaperPolicy = ReaperPolicy {
        cpu_quiescent_millicores: 20,
        idle_grace: Duration::from_secs(120),
    };

    fn stats(cpu: u32) -> SandboxStats {
        SandboxStats {
            cpu_millicores: cpu,
            memory_bytes: 0,
            sampled_at: Utc::now(),
            ..Default::default()
        }
    }

    #[test]
    fn an_evicting_sandbox_is_suspended_even_when_busy_and_recent() {
        let s = SandboxStats {
            cpu_millicores: 900,
            evicting: true,
            sampled_at: Utc::now(),
            ..Default::default()
        };
        assert!(should_suspend(&s, None, Instant::now(), POLICY, true));
    }

    #[test]
    fn an_evicting_flag_is_honoured_even_on_a_stale_sample() {
        let interval = Duration::from_secs(60);
        assert!(is_stale(&SandboxStats::default(), interval));
        let evicting = SandboxStats {
            evicting: true,
            ..Default::default()
        };
        assert!(!is_stale(&evicting, interval));
    }

    #[test]
    fn busy_cpu_never_suspends() {
        let now = Instant::now();
        let long_ago = now.checked_sub(Duration::from_secs(600));
        assert!(!should_suspend(&stats(21), long_ago, now, POLICY, false));
    }

    #[test]
    fn quiet_but_grace_not_elapsed_waits() {
        let now = Instant::now();
        let recent = now.checked_sub(Duration::from_secs(30));
        assert!(!should_suspend(&stats(20), recent, now, POLICY, false));
        assert!(!should_suspend(&stats(0), None, now, POLICY, false));
    }

    #[test]
    fn grace_elapsed_but_llm_call_seen_waits() {
        let now = Instant::now();
        let long_ago = now.checked_sub(Duration::from_secs(300));
        assert!(!should_suspend(&stats(0), long_ago, now, POLICY, true));
    }

    #[test]
    fn all_clear_suspends() {
        let now = Instant::now();
        let long_ago = now.checked_sub(Duration::from_secs(120));
        assert!(should_suspend(&stats(20), long_ago, now, POLICY, false));
    }
}
