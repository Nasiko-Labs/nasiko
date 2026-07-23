//! Container-hours metering for billing.
//!
//! A background reconciler polls the runtime every `CONTAINER_HOURS_POLL_SECS`
//! (default 60 s) and records one row per container/pod run in
//! `agent_instance_sessions`: `started_at` (the backend's true start time),
//! `last_seen_at` (bumped each tick while observed), `ended_at` (stamped when
//! the instance disappears). The poller only *writes intervals*; hours are
//! *counted at read time* by interval-overlap SQL ([`windowed_agent_hours`],
//! [`windowed_hours_series`]), so a missed tick can never lose billable time —
//! start times come from the runtime, not from ticks.
//!
//! Billing-correctness rules encoded here:
//! - An observation failure aborts the whole tick and **never closes
//!   sessions** — a runtime hiccup must not truncate everyone's billing.
//! - `ended_at = last_seen_at` (not `now()`): an instance died at an unknown
//!   moment inside the last poll window; billing the confirmed-alive time is
//!   the undercount-safe choice. The same applies across control-plane
//!   downtime: sessions freeze at `last_seen_at` and close on the first tick
//!   after restart.
//! - Only `ready == true` instances bill; a run that flaps not-ready is closed
//!   and reopened (`ended_at = NULL`) if the same `(instance_key, started_at)`
//!   comes back — never-ready runs (CrashLoopBackOff, Pending) never open a
//!   session.
//! - Rows intentionally have **no foreign key** to `agents`, so they survive
//!   hard agent deletion; multiple control-plane replicas may reconcile
//!   concurrently (upserts and closes are idempotent).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nasiko_runtime::{ContainerRuntime, InstanceInfo};
use sqlx::PgPool;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

/// Background loop: reconcile once per `poll` interval, forever.
///
/// Spawned from `AppState::from_config_with_db` (both editions); a failed tick
/// is logged and skipped — the loop itself never exits.
pub async fn run(
    db: PgPool,
    runtime: Arc<dyn ContainerRuntime>,
    agent_runtime: String,
    poll: Duration,
) {
    let kind = runtime_kind(&agent_runtime);
    let mut interval = tokio::time::interval(poll);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tracing::info!(poll_secs = poll.as_secs(), runtime = kind, "container-hours meter started");
    loop {
        interval.tick().await;
        match reconcile_once(&db, runtime.as_ref(), kind).await {
            Ok(stats) => tracing::debug!(
                observed = stats.observed,
                upserted = stats.upserted,
                closed = stats.closed,
                "hours meter tick"
            ),
            Err(e) => tracing::warn!(error = %e, "hours meter tick skipped"),
        }
    }
}

/// Map the `AGENT_RUNTIME` config value to the `runtime` column value.
fn runtime_kind(agent_runtime: &str) -> &'static str {
    match agent_runtime {
        "k8s" | "kubernetes" => "kubernetes",
        _ => "docker",
    }
}

/// Per-tick counters, for logs and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    /// Ready instances observed this tick (after dedupe).
    pub observed: usize,
    /// Sessions opened or bumped.
    pub upserted: usize,
    /// Open sessions closed because their instance disappeared.
    pub closed: u64,
}

/// One observation pass: ask the runtime what is running, upsert a session per
/// ready instance, close open sessions whose instance is gone.
pub async fn reconcile_once(
    db: &PgPool,
    runtime: &dyn ContainerRuntime,
    kind: &str,
) -> anyhow::Result<ReconcileStats> {
    // Billing rule #1: on observation failure do nothing — especially do not
    // close sessions (see module doc).
    let instances = runtime.list_instances().await?;
    let now = Utc::now();

    let mut seen: HashSet<(String, Option<DateTime<Utc>>)> = HashSet::new();
    let ready: Vec<&InstanceInfo> = instances
        .iter()
        .filter(|i| i.ready)
        .filter(|i| seen.insert((i.instance_key.clone(), i.started_at)))
        .collect();

    let names = resolve_agent_names(db, &ready).await?;

    let mut stats = ReconcileStats {
        observed: ready.len(),
        ..Default::default()
    };

    // Split by whether the backend reported a true start time: known starts
    // upsert against the (instance_key, started_at) unique constraint; unknown
    // starts (trait-default synthesis only) key on the open session instead so
    // each tick does NOT mint a new row.
    let mut ids: Vec<Uuid> = Vec::new();
    let mut agent_names: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut kinds: Vec<String> = Vec::new();
    let mut starts: Vec<DateTime<Utc>> = Vec::new();
    let mut seens: Vec<DateTime<Utc>> = Vec::new();

    // Observed (key, started_at) pairs for the close step — includes
    // fallback-keyed instances with a NULL started_at, which keeps their open
    // session alive regardless of its recorded start.
    let mut observed_keys: Vec<String> = Vec::new();
    let mut observed_starts: Vec<Option<DateTime<Utc>>> = Vec::new();

    for inst in &ready {
        let Some((agent_id, agent_name)) = names.get(inst.container_id.as_str()) else {
            tracing::debug!(
                container_id = inst.container_id.as_str(),
                instance_key = %inst.instance_key,
                "hours meter: unattributable instance skipped"
            );
            continue;
        };
        observed_keys.push(inst.instance_key.clone());
        observed_starts.push(inst.started_at);
        match inst.started_at {
            Some(started_at) => {
                ids.push(*agent_id);
                agent_names.push(agent_name.clone());
                keys.push(inst.instance_key.clone());
                kinds.push(kind.to_string());
                starts.push(started_at);
                seens.push(now);
            }
            None => {
                stats.upserted += upsert_fallback_session(
                    db,
                    *agent_id,
                    agent_name,
                    &inst.instance_key,
                    kind,
                    now,
                )
                .await?;
            }
        }
    }

    if !ids.is_empty() {
        // agent_name is intentionally NOT updated on conflict — it is a
        // snapshot taken at first observation. ended_at = NULL self-heals a
        // session closed by a transient miss (readiness flap).
        let res = sqlx::query(
            r#"INSERT INTO agent_instance_sessions
                   (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at)
               SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::text[], $4::text[],
                                    $5::timestamptz[], $6::timestamptz[])
               ON CONFLICT ON CONSTRAINT agent_instance_sessions_run_unique
               DO UPDATE SET last_seen_at = EXCLUDED.last_seen_at, ended_at = NULL"#,
        )
        .bind(&ids)
        .bind(&agent_names)
        .bind(&keys)
        .bind(&kinds)
        .bind(&starts)
        .bind(&seens)
        .execute(db)
        .await?;
        stats.upserted += res.rows_affected() as usize;
    }

    // Close every open session not observed this tick. An empty observation
    // set is valid (nothing running) and closes all open sessions.
    let res = sqlx::query(
        r#"UPDATE agent_instance_sessions s
           SET ended_at = s.last_seen_at
           WHERE s.ended_at IS NULL
             AND NOT EXISTS (
                 SELECT 1
                 FROM UNNEST($1::text[], $2::timestamptz[]) AS o(instance_key, started_at)
                 WHERE o.instance_key = s.instance_key
                   AND (o.started_at = s.started_at OR o.started_at IS NULL)
             )"#,
    )
    .bind(&observed_keys)
    .bind(&observed_starts)
    .execute(db)
    .await?;
    stats.closed = res.rows_affected();

    Ok(stats)
}

/// Bump the open session for an instance with no backend-reported start time,
/// inserting a first-seen session if none is open. Returns rows touched (1).
async fn upsert_fallback_session(
    db: &PgPool,
    agent_id: Uuid,
    agent_name: &str,
    instance_key: &str,
    kind: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<usize> {
    let updated = sqlx::query(
        r#"UPDATE agent_instance_sessions
           SET last_seen_at = $2, ended_at = NULL
           WHERE instance_key = $1 AND ended_at IS NULL"#,
    )
    .bind(instance_key)
    .bind(now)
    .execute(db)
    .await?
    .rows_affected();
    if updated > 0 {
        return Ok(updated as usize);
    }
    sqlx::query(
        r#"INSERT INTO agent_instance_sessions
               (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at)
           VALUES ($1, $2, $3, $4, $5, $5)
           ON CONFLICT ON CONSTRAINT agent_instance_sessions_run_unique DO NOTHING"#,
    )
    .bind(agent_id)
    .bind(agent_name)
    .bind(instance_key)
    .bind(kind)
    .bind(now)
    .execute(db)
    .await?;
    Ok(1)
}

/// Resolve each distinct runtime `container_id` to `(agent UUID, name snapshot)`.
///
/// Modern deploy paths key containers on the agent UUID; legacy paths key on
/// the agent name. A UUID whose agent row is gone (deleted agent whose
/// container outlived the best-effort destroy) falls back to the latest
/// session snapshot, then to the UUID string. Unattributable non-UUID ids
/// resolve to nothing and are skipped by the caller.
async fn resolve_agent_names(
    db: &PgPool,
    instances: &[&InstanceInfo],
) -> anyhow::Result<HashMap<String, (Uuid, String)>> {
    let mut by_uuid: Vec<Uuid> = Vec::new();
    let mut by_name: Vec<String> = Vec::new();
    for inst in instances {
        let raw = inst.container_id.as_str();
        match Uuid::parse_str(raw) {
            Ok(id) => by_uuid.push(id),
            Err(_) => by_name.push(raw.to_string()),
        }
    }
    by_uuid.sort_unstable();
    by_uuid.dedup();
    by_name.sort_unstable();
    by_name.dedup();

    let mut out: HashMap<String, (Uuid, String)> = HashMap::new();

    if !by_uuid.is_empty() {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, COALESCE(display_name, name) FROM agents WHERE id = ANY($1)",
        )
        .bind(&by_uuid)
        .fetch_all(db)
        .await?;
        for (id, name) in rows {
            out.insert(id.to_string(), (id, name));
        }
        // Deleted agents: reuse the last snapshot so the meter keeps a stable name.
        let unresolved: Vec<Uuid> = by_uuid
            .iter()
            .filter(|id| !out.contains_key(&id.to_string()))
            .copied()
            .collect();
        if !unresolved.is_empty() {
            let rows: Vec<(Uuid, String)> = sqlx::query_as(
                r#"SELECT DISTINCT ON (agent_id) agent_id, agent_name
                   FROM agent_instance_sessions
                   WHERE agent_id = ANY($1)
                   ORDER BY agent_id, last_seen_at DESC"#,
            )
            .bind(&unresolved)
            .fetch_all(db)
            .await?;
            for (id, name) in rows {
                out.insert(id.to_string(), (id, name));
            }
            for id in unresolved {
                out.entry(id.to_string()).or_insert_with(|| (id, id.to_string()));
            }
        }
    }

    if !by_name.is_empty() {
        let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
            r#"SELECT id, name, COALESCE(display_name, name)
               FROM agents WHERE name = ANY($1) AND deleted_at IS NULL"#,
        )
        .bind(&by_name)
        .fetch_all(db)
        .await?;
        for (id, name, display) in rows {
            out.insert(name, (id, display));
        }
    }

    Ok(out)
}

// ─── Read side: windowed replica-hours ───────────────────────────────────────

/// Per-agent replica-hours within a window, deleted agents included.
#[derive(Debug, sqlx::FromRow)]
pub struct AgentHoursAgg {
    pub agent_id: Uuid,
    pub agent_name: String,
    pub hours: f64,
    /// Currently-open sessions among the window-matched rows (i.e. replicas
    /// live right now that overlapped this window).
    pub live_replicas: i64,
    pub deleted: bool,
}

/// Sum each session's overlap with `[start, end)`, grouped by agent.
///
/// Open sessions end at `last_seen_at` (never `now()`), so settled windows are
/// immutable on re-query and a dead poller can't inflate hours. Negative
/// overlaps from clock skew clamp to zero.
pub async fn windowed_agent_hours(
    db: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    agent_id: Option<Uuid>,
) -> sqlx::Result<Vec<AgentHoursAgg>> {
    sqlx::query_as(
        r#"SELECT
               s.agent_id,
               COALESCE(a.display_name, a.name,
                        (array_agg(s.agent_name ORDER BY s.last_seen_at DESC))[1]) AS agent_name,
               (SUM(GREATEST(EXTRACT(EPOCH FROM (
                    LEAST(COALESCE(s.ended_at, s.last_seen_at), $2)
                  - GREATEST(s.started_at, $1))), 0)) / 3600.0)::float8 AS hours,
               COUNT(*) FILTER (WHERE s.ended_at IS NULL) AS live_replicas,
               (a.id IS NULL OR a.deleted_at IS NOT NULL) AS deleted
           FROM agent_instance_sessions s
           LEFT JOIN agents a ON a.id = s.agent_id
           WHERE s.started_at < $2
             AND COALESCE(s.ended_at, s.last_seen_at) > $1
             AND ($3::uuid IS NULL OR s.agent_id = $3)
           GROUP BY s.agent_id, a.id, a.display_name, a.name, a.deleted_at
           ORDER BY hours DESC"#,
    )
    .bind(start)
    .bind(end)
    .bind(agent_id)
    .fetch_all(db)
    .await
}

/// Bucket granularity for [`windowed_hours_series`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoursBucket {
    Hour,
    Day,
}

impl HoursBucket {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hour" => Some(Self::Hour),
            "day" => Some(Self::Day),
            _ => None,
        }
    }

    pub fn seconds(self) -> i64 {
        match self {
            Self::Hour => 3600,
            Self::Day => 86_400,
        }
    }

    /// Postgres interval literal — server-chosen, never caller text.
    fn interval_sql(self) -> &'static str {
        match self {
            Self::Hour => "1 hour",
            Self::Day => "1 day",
        }
    }
}

/// One bucket of the time series returned by [`windowed_hours_series`].
#[derive(Debug, sqlx::FromRow)]
pub struct HoursBucketRow {
    pub bucket_start: DateTime<Utc>,
    pub hours: f64,
}

/// Replica-hours per bucket across `[start, end)` — platform-wide, or one
/// agent's series when `agent_id` is set.
///
/// A trailing partial bucket is clipped to `end`, so Σ buckets always equals
/// the window total from [`windowed_agent_hours`] (additivity).
pub async fn windowed_hours_series(
    db: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    bucket: HoursBucket,
    agent_id: Option<Uuid>,
) -> sqlx::Result<Vec<HoursBucketRow>> {
    sqlx::query_as(
        r#"SELECT
               b.bucket_start,
               COALESCE(SUM(GREATEST(EXTRACT(EPOCH FROM (
                    LEAST(COALESCE(s.ended_at, s.last_seen_at),
                          LEAST(b.bucket_start + ($3::text)::interval, $2))
                  - GREATEST(s.started_at, b.bucket_start))), 0)) / 3600.0, 0)::float8 AS hours
           FROM generate_series($1, $2 - interval '1 microsecond', ($3::text)::interval)
                AS b(bucket_start)
           LEFT JOIN agent_instance_sessions s
                  ON s.started_at < LEAST(b.bucket_start + ($3::text)::interval, $2)
                 AND COALESCE(s.ended_at, s.last_seen_at) > b.bucket_start
                 AND ($4::uuid IS NULL OR s.agent_id = $4)
           GROUP BY b.bucket_start
           ORDER BY b.bucket_start"#,
    )
    .bind(start)
    .bind(end)
    .bind(bucket.interval_sql())
    .bind(agent_id)
    .fetch_all(db)
    .await
}

/// Hours of `[session_start, session_end]` overlapping `[win_start, win_end)`,
/// clamped at zero. Executable spec of the SQL in [`windowed_agent_hours`].
pub fn overlap_hours(
    session_start: DateTime<Utc>,
    session_end: DateTime<Utc>,
    win_start: DateTime<Utc>,
    win_end: DateTime<Utc>,
) -> f64 {
    let start = session_start.max(win_start);
    let end = session_end.min(win_end);
    let secs = (end - start).num_milliseconds() as f64 / 1000.0;
    (secs / 3600.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().expect("valid RFC3339 timestamp")
    }

    #[test]
    fn overlap_session_inside_window() {
        let h = overlap_hours(
            ts("2026-07-21T10:20:00Z"),
            ts("2026-07-21T10:50:00Z"),
            ts("2026-07-21T10:00:00Z"),
            ts("2026-07-21T11:00:00Z"),
        );
        assert!((h - 0.5).abs() < 1e-9);
    }

    #[test]
    fn overlap_session_straddles_window_start() {
        let h = overlap_hours(
            ts("2026-07-21T09:00:00Z"),
            ts("2026-07-21T10:30:00Z"),
            ts("2026-07-21T10:00:00Z"),
            ts("2026-07-21T11:00:00Z"),
        );
        assert!((h - 0.5).abs() < 1e-9);
    }

    #[test]
    fn overlap_session_straddles_window_end() {
        let h = overlap_hours(
            ts("2026-07-21T10:45:00Z"),
            ts("2026-07-21T12:00:00Z"),
            ts("2026-07-21T10:00:00Z"),
            ts("2026-07-21T11:00:00Z"),
        );
        assert!((h - 0.25).abs() < 1e-9);
    }

    #[test]
    fn overlap_disjoint_is_zero() {
        let h = overlap_hours(
            ts("2026-07-21T08:00:00Z"),
            ts("2026-07-21T09:00:00Z"),
            ts("2026-07-21T10:00:00Z"),
            ts("2026-07-21T11:00:00Z"),
        );
        assert_eq!(h, 0.0);
    }

    #[test]
    fn overlap_clock_skew_negative_clamps_to_zero() {
        // started_at (runtime clock) after last_seen_at (server clock).
        let h = overlap_hours(
            ts("2026-07-21T10:00:05Z"),
            ts("2026-07-21T10:00:00Z"),
            ts("2026-07-21T09:00:00Z"),
            ts("2026-07-21T11:00:00Z"),
        );
        assert_eq!(h, 0.0);
    }

    #[test]
    fn overlap_spanning_session_splits_additively_across_hours() {
        // A session 09:00 → 11:00 contributes exactly 1.0 h to each hourly window.
        let s = ts("2026-07-21T09:00:00Z");
        let e = ts("2026-07-21T11:00:00Z");
        let h1 = overlap_hours(s, e, ts("2026-07-21T09:00:00Z"), ts("2026-07-21T10:00:00Z"));
        let h2 = overlap_hours(s, e, ts("2026-07-21T10:00:00Z"), ts("2026-07-21T11:00:00Z"));
        assert!((h1 - 1.0).abs() < 1e-9);
        assert!((h2 - 1.0).abs() < 1e-9);
        assert!((h1 + h2 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn three_replicas_for_one_hour_bill_three_hours() {
        let win_s = ts("2026-07-21T10:00:00Z");
        let win_e = ts("2026-07-21T11:00:00Z");
        let total: f64 = (0..3)
            .map(|_| overlap_hours(win_s, win_e, win_s, win_e))
            .sum();
        assert!((total - 3.0).abs() < 1e-9);
    }

    #[test]
    fn runtime_kind_maps_config_values() {
        assert_eq!(runtime_kind("k8s"), "kubernetes");
        assert_eq!(runtime_kind("kubernetes"), "kubernetes");
        assert_eq!(runtime_kind("docker"), "docker");
        assert_eq!(runtime_kind("local"), "docker");
    }

    #[test]
    fn hours_bucket_parses_only_known_values() {
        assert_eq!(HoursBucket::parse("hour"), Some(HoursBucket::Hour));
        assert_eq!(HoursBucket::parse("day"), Some(HoursBucket::Day));
        assert_eq!(HoursBucket::parse("week"), None);
        assert_eq!(HoursBucket::parse(""), None);
    }
}
