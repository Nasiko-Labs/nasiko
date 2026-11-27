//! The `harness_sessions` scenario: many parked harness sessions, a few active at a time, a
//! reaper suspending the idle ones.
//!
//! In-process, against a throwaway Postgres database and the real session code: sessions are
//! created through `SessionStore::create`, a wake resolves the session row and harness catalog
//! entry, builds the spec with `spec_for_session` and calls `acquire` on a
//! [`SimulatedSandboxProvider`], and suspension is the real `reaper::sweep`. It stops short of
//! the ACP turn itself (lease, WebSocket, shim): that path is covered by
//! `oss/server/tests/harness_turn_e2e.rs` and adds nothing to a capacity number.
//!
//! Time is virtual. "Advancing the clock" shifts every running session's `last_turn_at` into the
//! past, which is what the reaper's database-clock idle test reads, and wake latency is drawn
//! from the provider's model rather than slept.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nasiko_runtime::{ContainerRuntime, SimulatedRuntime};
use nasiko_server::sessions::reaper::{ReaperMemory, SweepOutcome, SweepScope, sweep};
use nasiko_server::sessions::{HarnessCatalog, SessionStore, own_agent, spec_for_session};
use nasiko_server::state::AppState;
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::index::sample;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::{BENCH_JWT_SECRET, build_bench_config, pg_admin_url, s3_endpoint};
use crate::server::BenchDb;
use crate::simulated_sandbox::{LatencyModel, SimulatedSandboxProvider, Wake};

/// The harness the scenario's sessions run (seeded by the catalog migration).
const HARNESS_ID: &str = "opencode";

/// Knobs of one run.
#[derive(Debug, Clone)]
pub struct Params {
    pub sessions: usize,
    pub users: usize,
    /// Share of sessions that send a turn each tick, 0..=100.
    pub active_pct: f64,
    pub ticks: usize,
    /// Virtual seconds between ticks.
    pub tick_secs: u64,
    /// Idle TTL after which the reaper may suspend, in virtual seconds.
    pub idle_ttl_secs: u64,
    pub latency: LatencyModel,
    pub seed: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            sessions: 1_000,
            users: 100,
            active_pct: 2.0,
            ticks: 10,
            tick_secs: 120,
            idle_ttl_secs: 300,
            latency: LatencyModel::default(),
            seed: 42,
        }
    }
}

/// What one tick did.
#[derive(Debug, Clone)]
pub struct TickReport {
    pub active: usize,
    pub wakes: usize,
    pub running_before_sweep: usize,
    pub suspended_by_sweep: usize,
    pub sweep_duration: Duration,
}

/// The run's numbers.
#[derive(Debug, Clone)]
pub struct Report {
    pub params: Params,
    pub ticks: Vec<TickReport>,
    pub max_running: usize,
    /// Mean Running count (before each sweep) over the second half of the ticks.
    pub steady_running: f64,
    pub cold_wakes: usize,
    pub resume_wakes: usize,
    pub wake_p50_ms: f64,
    pub wake_p95_ms: f64,
    /// Sessions the final past-the-TTL sweep suspended, and the Running count left after it.
    pub drain_suspended: usize,
    pub running_after_drain: usize,
    pub sweep_p50_ms: f64,
    pub sweep_max_ms: f64,
}

impl Report {
    fn mean_suspends(&self) -> f64 {
        mean(self.ticks.iter().map(|t| t.suspended_by_sweep as f64))
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = &self.params;
        let active = self.ticks.first().map_or(0, |t| t.active);
        writeln!(f, "harness_sessions")?;
        writeln!(
            f,
            "  sessions            {} ({} users)",
            p.sessions, p.users
        )?;
        writeln!(
            f,
            "  ticks               {} x {}s virtual, idle TTL {}s",
            p.ticks, p.tick_secs, p.idle_ttl_secs
        )?;
        writeln!(f, "  active per tick     {active} ({}%)", p.active_pct)?;
        writeln!(
            f,
            "  running sandboxes   max {}, steady {:.1}, after final drain {}",
            self.max_running, self.steady_running, self.running_after_drain
        )?;
        writeln!(
            f,
            "  wakes               {} cold, {} resume",
            self.cold_wakes, self.resume_wakes
        )?;
        writeln!(
            f,
            "  wake latency        p50 {:.0} ms, p95 {:.0} ms (modelled)",
            self.wake_p50_ms, self.wake_p95_ms
        )?;
        writeln!(
            f,
            "  suspends per sweep  mean {:.1} (final drain {})",
            self.mean_suspends(),
            self.drain_suspended
        )?;
        write!(
            f,
            "  sweep duration      p50 {:.0} ms, max {:.0} ms (wall clock)",
            self.sweep_p50_ms, self.sweep_max_ms
        )
    }
}

/// Nearest-rank percentile (`q` in 0..=1) of `values`; 0 when empty.
pub fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// Whether the bench Postgres and object store answer; callers that must not fail without infra
/// skip on `false`. The object store counts because building the server state refuses to start
/// without one.
pub async fn infra_available() -> bool {
    let postgres = matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            sqlx::postgres::PgPoolOptions::new().connect(&pg_admin_url())
        )
        .await,
        Ok(Ok(_))
    );
    postgres && object_store_answers().await
}

/// Whether anything accepts a connection at the bench S3 endpoint's host and port.
async fn object_store_answers() -> bool {
    let endpoint = s3_endpoint();
    let (secure, rest) = match endpoint.split_once("://") {
        Some((scheme, rest)) => (scheme == "https", rest),
        None => (false, endpoint.as_str()),
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let address = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:{}", if secure { 443 } else { 80 })
    };
    matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::TcpStream::connect(address)
        )
        .await,
        Ok(Ok(_))
    )
}

/// Runs the scenario on a throwaway database, which is dropped afterwards.
pub async fn run(params: Params) -> anyhow::Result<Report> {
    anyhow::ensure!(
        params.sessions > 0 && params.users > 0,
        "sessions and users must be > 0"
    );
    let db = BenchDb::create().await;
    let outcome = run_on(&db, &params).await;
    db.drop_db().await;
    outcome
}

async fn run_on(db: &BenchDb, params: &Params) -> anyhow::Result<Report> {
    let provider = Arc::new(SimulatedSandboxProvider::new(params.latency, params.seed));
    let state = build_state(db, params, provider.clone()).await;
    let owners = seed_world(&state, params).await?;
    let sessions = create_sessions(&db.pool, &owners, params.sessions).await?;
    let world = World {
        state,
        provider,
        sessions,
        params: params.clone(),
    };
    world.drive().await
}

async fn build_state(
    db: &BenchDb,
    params: &Params,
    provider: Arc<SimulatedSandboxProvider>,
) -> AppState {
    let mut config = build_bench_config(db.database_url.clone(), "http://127.0.0.1:1");
    config.coding_agent_idle_ttl_secs = i64::try_from(params.idle_ttl_secs).unwrap_or(i64::MAX);
    config.session_idle_grace_secs = config.session_idle_grace_secs.min(params.idle_ttl_secs);
    let runtime: Arc<dyn ContainerRuntime> =
        Arc::new(SimulatedRuntime::new("http://127.0.0.1:1".to_owned()));
    let auth: Arc<dyn nasiko_auth::AuthService> = Arc::new(nasiko_auth::AuthServiceImpl::new(
        db.pool.clone(),
        BENCH_JWT_SECRET.to_string(),
    ));
    let oci: Arc<dyn nasiko_runtime::BlobStore> =
        Arc::new(nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await);
    let scaling = Arc::new(nasiko_server::agent_scaling::SingleReplica::new(
        runtime.clone(),
        db.pool.clone(),
    ));
    AppState::from_config_with_sandbox(
        config,
        auth,
        runtime,
        oci,
        db.pool.clone(),
        scaling,
        provider,
    )
    .await
}

/// `params.users` members, each with their own harness agent, as opening a session gives them:
/// `(user, agent)` pairs.
async fn seed_world(state: &AppState, params: &Params) -> anyhow::Result<Vec<(Uuid, Uuid)>> {
    let harness = HarnessCatalog::new(state.db.clone())
        .get(HARNESS_ID)
        .await?
        .ok_or_else(|| anyhow::anyhow!("harness {HARNESS_ID} is not in the catalog"))?;
    let users: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO users (username, email, role) \
         SELECT 'bench_user_' || g, 'bench_user_' || g || '@bench.local', 'member' \
         FROM generate_series(0, $1::int - 1) g RETURNING id",
    )
    .bind(i32::try_from(params.users)?)
    .fetch_all(&state.db)
    .await?;
    let mut owners = Vec::with_capacity(users.len());
    for user in users {
        let agent = own_agent::ensure(
            &state.db,
            user,
            &harness,
            &state.config.harness_image_prefix,
        )
        .await?;
        owners.push((user, agent));
    }
    Ok(owners)
}

/// `count` parked sessions spread round-robin over `owners`, each bound to its user's agent.
async fn create_sessions(
    pool: &PgPool,
    owners: &[(Uuid, Uuid)],
    count: usize,
) -> anyhow::Result<Vec<String>> {
    let store = SessionStore::new(pool.clone());
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let (user, agent) = owners[i % owners.len()];
        let row = store.create(agent, user, None).await?;
        ids.push(row.session_id);
    }
    Ok(ids)
}

struct World {
    state: AppState,
    provider: Arc<SimulatedSandboxProvider>,
    sessions: Vec<String>,
    params: Params,
}

impl World {
    async fn drive(&self) -> anyhow::Result<Report> {
        let mut rng = StdRng::seed_from_u64(self.params.seed);
        let mut memory = ReaperMemory::default();
        let mut wakes: Vec<Wake> = Vec::new();
        let mut ticks = Vec::with_capacity(self.params.ticks);
        let per_tick = ((self.sessions.len() as f64 * self.params.active_pct / 100.0).ceil()
            as usize)
            .clamp(1, self.sessions.len());

        for _ in 0..self.params.ticks {
            for idx in sample(&mut rng, self.sessions.len(), per_tick) {
                self.send_turn(&self.sessions[idx]).await?;
            }
            let tick_wakes = self.provider.take_wakes();
            let running_before_sweep = self.provider.running_count();
            let wake_count = tick_wakes.len();
            wakes.extend(tick_wakes);
            self.advance_clock(self.params.tick_secs).await?;
            let (suspended, took) = self.sweep(&mut memory).await?;
            ticks.push(TickReport {
                active: per_tick,
                wakes: wake_count,
                running_before_sweep,
                suspended_by_sweep: suspended,
                sweep_duration: took,
            });
        }

        self.advance_clock(self.params.idle_ttl_secs + 1).await?;
        let (drain_suspended, _) = self.sweep(&mut memory).await?;
        Ok(summarise(
            &self.params,
            ticks,
            &wakes,
            drain_suspended,
            self.provider.running_count(),
        ))
    }

    /// The wake half of a turn: resolve the session and its harness, acquire the sandbox,
    /// record it on the row. The sandbox is busy for the turn and quiet again after.
    async fn send_turn(&self, session_id: &str) -> anyhow::Result<()> {
        let store = SessionStore::new(self.state.db.clone());
        let row = store
            .get(session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("session {session_id} vanished"))?;
        let harness = HarnessCatalog::new(self.state.db.clone())
            .get(HARNESS_ID)
            .await?
            .ok_or_else(|| anyhow::anyhow!("harness {HARNESS_ID} missing from the catalog"))?;
        let spec = spec_for_session(
            &harness,
            &row,
            HashMap::new(),
            row.user_id,
            &self.state.config,
        );
        let handle = self.state.sandbox.acquire(&spec).await?;
        self.provider.set_busy(&handle, true);
        sqlx::query(
            "UPDATE chat_sessions SET sandbox_ref = $2, runtime_state = 'running', \
             last_turn_at = now(), suspended_at = NULL, turn_fence = turn_fence + 1 \
             WHERE session_id = $1",
        )
        .bind(session_id)
        .bind(handle.sandbox_ref())
        .execute(&self.state.db)
        .await?;
        self.provider.set_busy(&handle, false);
        Ok(())
    }

    /// Virtual time passes: every live session's last turn moves `secs` further into the past.
    async fn advance_clock(&self, secs: u64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE chat_sessions SET last_turn_at = last_turn_at - make_interval(secs => $1) \
             WHERE runtime_state = 'running'",
        )
        .bind(secs as f64)
        .execute(&self.state.db)
        .await?;
        Ok(())
    }

    async fn sweep(&self, memory: &mut ReaperMemory) -> anyhow::Result<(usize, Duration)> {
        let started = Instant::now();
        let outcome = sweep(&self.state, memory, SweepScope::Full).await?;
        match outcome {
            SweepOutcome::Swept { suspended, .. } => Ok((suspended, started.elapsed())),
            SweepOutcome::Unsupported(msg) => anyhow::bail!("provider unsupported: {msg}"),
        }
    }
}

fn summarise(
    params: &Params,
    ticks: Vec<TickReport>,
    wakes: &[Wake],
    drain_suspended: usize,
    running_after_drain: usize,
) -> Report {
    use crate::simulated_sandbox::WakeKind;
    let latencies: Vec<f64> = wakes
        .iter()
        .map(|w| w.latency.as_secs_f64() * 1000.0)
        .collect();
    let sweeps: Vec<f64> = ticks
        .iter()
        .map(|t| t.sweep_duration.as_secs_f64() * 1000.0)
        .collect();
    let steady = ticks.len() / 2;
    Report {
        params: params.clone(),
        max_running: ticks
            .iter()
            .map(|t| t.running_before_sweep)
            .max()
            .unwrap_or(0),
        steady_running: mean(
            ticks[steady..]
                .iter()
                .map(|t| t.running_before_sweep as f64),
        ),
        cold_wakes: wakes.iter().filter(|w| w.kind == WakeKind::Cold).count(),
        resume_wakes: wakes.iter().filter(|w| w.kind == WakeKind::Resume).count(),
        wake_p50_ms: percentile(&latencies, 0.50),
        wake_p95_ms: percentile(&latencies, 0.95),
        drain_suspended,
        running_after_drain,
        sweep_p50_ms: percentile(&sweeps, 0.50),
        sweep_max_ms: sweeps.iter().copied().fold(0.0, f64::max),
        ticks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_is_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.50), 50.0);
        assert_eq!(percentile(&v, 0.95), 95.0);
        assert_eq!(percentile(&v, 1.0), 100.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[7.0], 0.95), 7.0);
    }

    /// Needs infra (Postgres, Redis, S3): skips when Postgres is unreachable.
    #[tokio::test]
    async fn smoke_run_wakes_and_suspends() {
        if !infra_available().await {
            eprintln!("skipping: bench Postgres or object store unreachable");
            return;
        }
        let report = run(Params {
            sessions: 50,
            users: 5,
            active_pct: 10.0,
            ticks: 4,
            tick_secs: 200,
            idle_ttl_secs: 300,
            ..Params::default()
        })
        .await
        .expect("scenario runs");
        assert!(report.cold_wakes + report.resume_wakes > 0, "{report}");
        assert!(report.max_running > 0);
        assert_eq!(report.running_after_drain, 0, "{report}");
    }
}
