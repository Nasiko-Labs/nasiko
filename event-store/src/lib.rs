//! Cold tier for append-only event tables (docs/LAKEHOUSE_TRD.md).
//!
//! Hot tables → Lance, rolled up before delete. `token_usage` is retired (0079):
//! its rows are drained to the cold `token_usage` dataset, which stays readable
//! and erasable.
//! Single writer: the tiering job holds the `tiering_job` advisory lock.

pub mod chat_messages;
pub mod chat_page;
pub mod chunks;
pub mod cold;
pub mod context_reports;
pub mod flow_participants;
pub mod flows;
pub mod metrics;
pub mod query;
mod session_erase;
pub mod token_savings;
pub mod token_usage;
pub mod tool_calls;
pub mod trace_usage;
pub mod user_setting_changes;

pub use query::{Cursor, UsageFilter, UsagePage, UsageRange};

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use token_usage::{TABLE, TokenUsageRow};
use trace_usage::TraceUsageRow;

/// How far a downstream reader of hot `trace_usage` rows has read, so tiering never
/// moves a row that reader has not seen yet. Wired once at the composition root;
/// [`NoTieringHold`] when nothing reads them.
#[async_trait::async_trait]
pub trait TieringHold: Send + Sync {
    /// Every `trace_usage` row with `materialized_at` before the returned instant has
    /// been read. `None`: no reader, nothing is held back.
    async fn trace_usage_read_to(&self) -> Result<Option<DateTime<Utc>>>;
}

/// No downstream reader: tiering is bounded by its age windows only.
pub struct NoTieringHold;

#[async_trait::async_trait]
impl TieringHold for NoTieringHold {
    async fn trace_usage_read_to(&self) -> Result<Option<DateTime<Utc>>> {
        Ok(None)
    }
}

/// Order of a tiering run. Rows that price or attribute themselves through a trace
/// (`context_reports`, `token_savings`) go before `trace_usage`, so the hot trace rows
/// are still there when their rollups freeze (tiered traces resolve through
/// `trace_totals` either way).
const TIER_ORDER: [&str; 8] = [
    user_setting_changes::TABLE,
    context_reports::TABLE,
    token_savings::TABLE,
    trace_usage::TABLE,
    flows::TABLE,
    chat_messages::TABLE,
    tool_calls::TABLE,
    // One-time drain of the retired call log (see `tier_legacy_usage_batch`).
    TABLE,
];

/// The retired `token_usage` call log, renamed by its migration so its rows can be
/// copied to the cold `token_usage` dataset instead of being dropped. Nothing writes
/// it; tiering drains it and drops it once empty (at once on a fresh install).
const LEGACY_USAGE: &str = "token_usage_legacy";

/// Scalar indexes of the cold `token_usage` dataset (user erasure, agent and
/// session lookups).
const TOKEN_USAGE_INDEXES: [&str; 3] = ["user_id", "agent_id", "session_id"];

/// Advisory lock key for the tiering job (`hashtext('tiering_job')` is not stable
/// across PG versions, so a fixed constant).
const TIERING_LOCK: i64 = 0x0074_6965_7269_6e67; // "tiering"

#[derive(Debug, Clone)]
pub struct TieringConfig {
    /// `NASIKO_COLD_PATH`: local dir, `file://`, `s3://` or `gs://` root of the datasets.
    pub cold_path: String,
    /// Object-store options for `s3://` cold paths (Lance/object_store keys:
    /// `aws_access_key_id`, `aws_secret_access_key`, `aws_region`, `aws_endpoint`,
    /// `allow_http`). Empty for local and `gs://` paths (GCS uses Application
    /// Default Credentials).
    pub storage_options: std::collections::HashMap<String, String>,
    pub min_hot_days: i64,
    /// Hot window for `trace_usage`. Longer than `min_hot_days` because overview
    /// p95 latency and per-session queries span 90 days and need raw rows.
    pub trace_min_hot_days: i64,
    pub max_hot_days: i64,
    pub batch_rows: i64,
    pub interval: Duration,
    /// Run compaction + version cleanup every N tiering runs (TRD §8, weekly).
    pub maintenance_every: u32,
    /// Old Lance versions younger than this are kept (readers mid-scan, rollback).
    pub version_grace: TimeDelta,
    /// Sessions whose newest message is older than this tier all messages (TRD §8.1).
    pub chat_idle_days: i64,
    /// Message bodies longer than this many bytes go to the `chat_message_bodies` sidecar.
    pub chat_body_inline_max: usize,
    /// Tool-call `arguments`/`output` longer than this many bytes go to the
    /// `tool_call_bodies` sidecar.
    pub tool_body_inline_max: usize,
}

impl Default for TieringConfig {
    fn default() -> Self {
        Self {
            cold_path: "./data/cold".into(),
            storage_options: Default::default(),
            min_hot_days: 30,
            trace_min_hot_days: 90,
            max_hot_days: 90,
            batch_rows: 100_000,
            interval: Duration::from_secs(24 * 3600),
            maintenance_every: 7,
            version_grace: TimeDelta::days(7),
            chat_idle_days: 30,
            chat_body_inline_max: 32 * 1024,
            tool_body_inline_max: 32 * 1024,
        }
    }
}

/// Rows older than the returned instant are tiered.
///
/// Normally `now - min_hot_days`. Small installs may wait for bigger batches, but
/// once the oldest hot row passes `max_hot_days` everything past the minimum goes,
/// so the effective rule here is simply the minimum window (TRD D3). Kept as a
/// function so batching policy can change in one place.
pub fn tier_cutoff(now: DateTime<Utc>, cfg: &TieringConfig) -> DateTime<Utc> {
    now - TimeDelta::days(cfg.min_hot_days.max(1))
}

/// `trace_usage` rows older than the returned instant are tiered. Never shorter
/// than the `token_usage` window.
pub fn trace_tier_cutoff(now: DateTime<Utc>, cfg: &TieringConfig) -> DateTime<Utc> {
    now - TimeDelta::days(cfg.trace_min_hot_days.max(cfg.min_hot_days).max(1))
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TierReport {
    pub batches: usize,
    pub rows: u64,
    pub recovered: usize,
    /// Pending erasures (users and chat sessions) completed this run.
    pub erased: usize,
    /// Tables skipped or cut short this run: a batch that could not be settled, or a
    /// batch that failed. The other tables still ran; the next run retries these.
    pub failed: Vec<String>,
}

/// What settling crashed batches found.
#[derive(Debug, Default)]
struct Recovery {
    settled: usize,
    /// Tables with a `writing` batch that could not be settled. Tiering them again
    /// would re-copy rows that batch already put in Lance, so they wait.
    stuck: Vec<&'static str>,
}

#[derive(Clone)]
pub struct EventStore {
    db: PgPool,
    cfg: TieringConfig,
    hold: Arc<dyn TieringHold>,
}

impl EventStore {
    pub fn new(db: PgPool, cfg: TieringConfig) -> Self {
        Self {
            db,
            cfg,
            hold: Arc::new(NoTieringHold),
        }
    }

    /// Tiering stops at `hold`'s read position as well as the age window.
    pub fn with_hold(mut self, hold: Arc<dyn TieringHold>) -> Self {
        self.hold = hold;
        self
    }

    fn uri(&self, table: &str) -> cold::Loc {
        cold::Loc {
            uri: cold::dataset_uri(&self.cfg.cold_path, table),
            opts: self.cfg.storage_options.clone(),
        }
    }

    /// Spawns the daily loop. Each replica tries; only the lock holder works.
    pub fn spawn_tiering(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut runs: u32 = 0;
            loop {
                let started = std::time::Instant::now();
                match self.run_once(Utc::now()).await {
                    Ok(Some(r)) => {
                        // Stable field names: log-based metrics/alerts key on these (docs/LAKEHOUSE_OPS.md).
                        tracing::info!(
                            target: "nasiko::tiering",
                            batches = r.batches,
                            rows = r.rows,
                            recovered = r.recovered,
                            erased = r.erased,
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            "tiering run finished"
                        );
                        if r.recovered > 0 {
                            tracing::warn!(target: "nasiko::tiering", recovered = r.recovered, "settled writing batches left by a crash");
                        }
                        if !r.failed.is_empty() {
                            tracing::error!(target: "nasiko::tiering", failed = ?r.failed, "tiering skipped or cut short some tables");
                        }
                        runs += 1;
                        if is_maintenance_run(runs, self.cfg.maintenance_every)
                            && let Err(e) = self.maintain(self.cfg.version_grace).await
                        {
                            metrics::metrics().failure("maintenance");
                            tracing::error!(target: "nasiko::tiering", error = %e, "cold maintenance failed");
                        }
                    }
                    Ok(None) => tracing::debug!("tiering lock held elsewhere"),
                    Err(e) => {
                        metrics::metrics().failure("run");
                        tracing::error!(target: "nasiko::tiering", error = %e, "tiering run failed");
                    }
                }
                tokio::time::sleep(self.cfg.interval).await;
            }
        })
    }

    /// One locked run: recover unfinished batches, then tier everything past the cutoff.
    /// `None` when another replica holds the lock.
    pub async fn run_once(&self, now: DateTime<Utc>) -> Result<Option<TierReport>> {
        let mut lock_conn = self.db.acquire().await?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(TIERING_LOCK)
            .fetch_one(&mut *lock_conn)
            .await?;
        if !got {
            return Ok(None);
        }
        let res = self.run_locked(now).await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(TIERING_LOCK)
            .execute(&mut *lock_conn)
            .await?;
        res.map(Some)
    }

    /// Finishes recorded erasures (users and deleted chats) without tiering anything.
    /// Tiering off, including a rollback, must not leave an erasure pending forever:
    /// the request outlives the first attempt and only this or a tiering run retries it.
    /// Settles crashed batches first, as erasure needs. `None` when another replica
    /// holds the lock.
    pub async fn retry_pending_erasures(&self) -> Result<Option<usize>> {
        let mut lock_conn = self.db.acquire().await?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(TIERING_LOCK)
            .fetch_one(&mut *lock_conn)
            .await?;
        if !got {
            return Ok(None);
        }
        let res: Result<usize> = async {
            self.recover_all().await;
            Ok(self.retry_erasures().await? + self.retry_session_erasures().await?)
        }
        .await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(TIERING_LOCK)
            .execute(&mut *lock_conn)
            .await?;
        res.map(Some)
    }

    /// The erasure-only loop a replica runs when tiering is off. Hourly at most, so a
    /// failed erase completes within the hour once the cold store is reachable.
    pub fn spawn_erasure_retries(self) -> tokio::task::JoinHandle<()> {
        let every: std::time::Duration =
            self.cfg.interval.min(std::time::Duration::from_secs(3600));
        tokio::spawn(async move {
            loop {
                // Sleep first: nothing here is urgent at boot, and startup is busy enough.
                tokio::time::sleep(every).await;
                match self.retry_pending_erasures().await {
                    Ok(Some(n)) if n > 0 => {
                        tracing::info!(target: "nasiko::tiering", erased = n, "pending erasures finished")
                    }
                    Ok(_) => {}
                    Err(e) => {
                        metrics::metrics().failure("erasure");
                        tracing::error!(target: "nasiko::tiering", error = %e, "erasure retries failed");
                    }
                }
            }
        })
    }

    /// Lance maintenance chain (compact -> cleanup old versions -> optimize indices)
    /// under the tiering lock, so it never races a tiering append.
    /// `None` when another replica holds the lock.
    pub async fn maintain(&self, grace: TimeDelta) -> Result<Option<()>> {
        let mut lock_conn = self.db.acquire().await?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(TIERING_LOCK)
            .fetch_one(&mut *lock_conn)
            .await?;
        if !got {
            return Ok(None);
        }
        let res = self.purge(grace).await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(TIERING_LOCK)
            .execute(&mut *lock_conn)
            .await?;
        res.map(Some)
    }

    /// Exports `nasiko.tiering.oldest_hot_age` per table (lag / hot age).
    /// (Batch metrics: see free fn `timed_batch`.)
    async fn record_hot_age(&self, now: DateTime<Utc>) -> Result<()> {
        for (table, col) in HOT_TS_COLUMNS {
            let oldest: Option<DateTime<Utc>> =
                sqlx::query_scalar(&format!("SELECT min({col}) FROM {table}"))
                    .fetch_one(&self.db)
                    .await?;
            let age: Duration = oldest
                .and_then(|t| (now - t).to_std().ok())
                .unwrap_or_default();
            metrics::metrics().oldest_hot(table, age);
        }
        Ok(())
    }

    async fn run_locked(&self, now: DateTime<Utc>) -> Result<TierReport> {
        let report = self.run_tables(now).await?;
        if let Err(e) = self.record_hot_age(Utc::now()).await {
            tracing::warn!(target: "nasiko::tiering", error = %e, "hot age metric failed");
        }
        Ok(report)
    }

    async fn run_tables(&self, now: DateTime<Utc>) -> Result<TierReport> {
        let recovery: Recovery = self.recover_all().await;
        let mut report = TierReport {
            recovered: recovery.settled,
            failed: recovery.stuck.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        };
        // Erasures before any table: a pending chat-delete erase finishes before a
        // re-created session with the same id can tier. Failures stay pending and
        // never stop the tables below.
        match self.retry_erasures().await {
            Ok(n) => report.erased += n,
            Err(e) => {
                metrics::metrics().failure("erasure");
                tracing::error!(target: "nasiko::tiering", error = %e, "user erasure retries failed");
                report.failed.push("erasure_requests".into());
            }
        }
        match self.retry_session_erasures().await {
            Ok(n) => report.erased += n,
            Err(e) => {
                metrics::metrics().failure("erasure");
                tracing::error!(target: "nasiko::tiering", error = %e, "session erasure retries failed");
                report.failed.push("session_erasure_requests".into());
            }
        }
        let trace_base = trace_tier_cutoff(now, &self.cfg);
        let trace_hold: Result<Option<DateTime<Utc>>> = self.hold.trace_usage_read_to().await;
        let chat_cutoff = now - TimeDelta::days(self.cfg.chat_idle_days.max(1));
        for table in TIER_ORDER {
            if recovery.stuck.contains(&table) {
                continue;
            }
            let (cutoff, hold) = match table {
                // The only table with a downstream reader.
                trace_usage::TABLE => match &trace_hold {
                    Ok(h) => (hold_cutoff(trace_base, *h), *h),
                    Err(e) => {
                        metrics::metrics().failure("hold");
                        tracing::error!(target: "nasiko::tiering", error = %e, "reader position unknown; trace_usage stays hot");
                        report.failed.push(table.to_string());
                        continue;
                    }
                },
                chat_messages::TABLE => (chat_cutoff, None),
                _ => (trace_base, None),
            };
            loop {
                match timed_batch(table, self.tier_batch(table, cutoff, hold)).await {
                    Ok(0) => break,
                    Ok(n) => {
                        report.batches += 1;
                        report.rows += n;
                    }
                    Err(e) => {
                        metrics::metrics().failure(table);
                        tracing::error!(target: "nasiko::tiering", table, error = %e, "tiering batch failed; other tables continue");
                        report.failed.push(table.to_string());
                        break;
                    }
                }
            }
        }
        Ok(report)
    }

    /// One batch of `table` past `cutoff` (`hold`: see [`Self::tier_trace_usage_batch`]).
    async fn tier_batch(
        &self,
        table: &str,
        cutoff: DateTime<Utc>,
        hold: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        match table {
            trace_usage::TABLE => self.tier_trace_usage_batch(cutoff, hold).await,
            user_setting_changes::TABLE => self.tier_settings_batch(cutoff).await,
            context_reports::TABLE => self.tier_reports_batch(cutoff).await,
            token_savings::TABLE => self.tier_savings_batch(cutoff).await,
            flows::TABLE => self.tier_flows_batch(cutoff).await,
            chat_messages::TABLE => self.tier_chat_batch(cutoff).await,
            tool_calls::TABLE => self.tier_tool_calls_batch(cutoff).await,
            TABLE => self.tier_legacy_usage_batch().await,
            other => anyhow::bail!("not a tiered table: {other}"),
        }
    }

    /// Phase 2: one `trace_usage` batch → Lance, then rollup + delete.
    ///
    /// `hold` is the consumer's `materialized_at` read position: a row written
    /// at or after it (an old trace that grew again) stays hot until read,
    /// whatever its `started_at`.
    ///
    /// The batch's rows stay locked (`FOR UPDATE`) from the select to the delete.
    /// `trace_usage` rows are upserted in place by live writers, so without the lock
    /// a write landing between the rollup and the delete would be in neither, and
    /// the Lance copy could differ from the rollups. With it, such a write waits and
    /// then starts a fresh hot row once the old one is gone.
    async fn tier_trace_usage_batch(
        &self,
        cutoff: DateTime<Utc>,
        hold: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        let mut rows: Vec<TraceUsageRow> = sqlx::query_as(&format!(
            "SELECT {} FROM trace_usage
             WHERE started_at < $1 AND ($3::timestamptz IS NULL OR materialized_at < $3)
             ORDER BY started_at, trace_id, agent_name LIMIT $2
             FOR UPDATE",
            trace_usage::COLUMNS
        ))
        .bind(cutoff)
        .bind(self.cfg.batch_rows)
        .bind(hold)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let min_ts = rows.iter().map(|r| r.started_at).min().unwrap();
        let max_ts = rows.iter().map(|r| r.started_at).max().unwrap();
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(trace_usage::TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;
        trace_usage::sort_rows(&mut rows);
        let keys: Vec<String> = rows
            .iter()
            .map(|r| trace_usage::row_key(&r.trace_id, &r.agent_name))
            .collect();
        let batch = trace_usage::to_record_batch(&rows, batch_id)?;
        let version =
            cold::append(&self.uri(trace_usage::TABLE), batch, &trace_usage::INDEXES).await?;
        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;
        finish_trace_rows(&mut tx, batch_id, &keys).await?;
        tx.commit().await?;
        Ok(keys.len() as u64)
    }

    /// One `tool_calls` batch → Lance, then delete (no rollup).
    async fn tier_tool_calls_batch(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        let mut rows: Vec<tool_calls::ToolCallRow> = sqlx::query_as(&format!(
            "SELECT {} FROM tool_calls WHERE started_at < $1
             ORDER BY started_at, trace_id, call_id LIMIT $2",
            tool_calls::COLUMNS
        ))
        .bind(cutoff)
        .bind(self.cfg.batch_rows)
        .fetch_all(&self.db)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let min_ts = rows.iter().map(|r| r.started_at).min().unwrap();
        let max_ts = rows.iter().map(|r| r.started_at).max().unwrap();
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(tool_calls::TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;
        tool_calls::sort_rows(&mut rows);
        let keys: Vec<String> = rows
            .iter()
            .map(|r| tool_calls::row_key(&r.trace_id, &r.call_id))
            .collect();
        let batches: tool_calls::Batches =
            tool_calls::to_record_batches(&rows, batch_id, self.cfg.tool_body_inline_max)?;
        // Bodies first: a main row never points at a body that is not yet written.
        if let Some(b) = batches.bodies {
            cold::append(&self.uri(tool_calls::BODIES), b, &tool_calls::BODY_INDEXES).await?;
        }
        let version = cold::append(
            &self.uri(tool_calls::TABLE),
            batches.main,
            &tool_calls::INDEXES,
        )
        .await?;
        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;
        self.finish_tool_calls_batch(batch_id, &keys).await?;
        Ok(keys.len() as u64)
    }

    async fn finish_tool_calls_batch(&self, batch_id: Uuid, keys: &[String]) -> Result<()> {
        let (traces, calls): (Vec<String>, Vec<String>) = keys
            .iter()
            .filter_map(|k| tool_calls::split_key(k))
            .map(|(t, c)| (t.to_string(), c.to_string()))
            .unzip();
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM tool_calls tc USING unnest($1::text[], $2::text[]) AS k(t, c)
             WHERE tc.trace_id = k.t AND tc.call_id = k.c",
        )
        .bind(&traces)
        .bind(&calls)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// One `flows` batch → Lance, then slim the hot rows (never deleted).
    ///
    /// The hot row keeps its identity columns so every SQL join on `flows` keeps
    /// working and `flow_steps` / `flow_participants` (FK `ON DELETE CASCADE`) are
    /// untouched. Only the heavy columns are cleared; `tiered_at` marks the row.
    /// Eligible when status is final ('completed' or 'failed'), not yet tiered,
    /// and completed_at < cutoff.
    async fn tier_flows_batch(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        let mut rows: Vec<flows::FlowRow> = sqlx::query_as(&format!(
            "SELECT {} FROM flows
             WHERE tiered_at IS NULL AND completed_at < $1 AND status IN ('completed', 'failed')
             ORDER BY completed_at, flow_id LIMIT $2",
            flows::COLUMNS
        ))
        .bind(cutoff)
        .bind(self.cfg.batch_rows)
        .fetch_all(&self.db)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let min_ts = rows
            .iter()
            .filter_map(|r| r.completed_at)
            .min()
            .unwrap_or_else(Utc::now);
        let max_ts = rows
            .iter()
            .filter_map(|r| r.completed_at)
            .max()
            .unwrap_or_else(Utc::now);
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(flows::TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;

        // Participants stay hot: MCP agent auth reads them on every call and they
        // are a few bytes per row.
        let flow_ids: Vec<String> = rows.iter().map(|r| r.flow_id.clone()).collect();

        flows::sort_rows(&mut rows);
        let batch = flows::to_record_batch(&rows, batch_id)?;
        let version = cold::append(&self.uri(flows::TABLE), batch, &flows::INDEXES).await?;

        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;

        self.finish_flows_batch(batch_id, &flow_ids).await?;
        Ok(flow_ids.len() as u64)
    }

    async fn finish_flows_batch(&self, batch_id: Uuid, flow_ids: &[String]) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        // Slim, never delete: a DELETE would cascade to flow_steps and
        // flow_participants and break every join that resolves a session or owner.
        sqlx::query(
            "UPDATE flows SET metadata = '{}'::jsonb, error_message = NULL, tiered_at = now()
             WHERE flow_id = ANY($1) AND tiered_at IS NULL",
        )
        .bind(flow_ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Settles one `writing` `flows` batch left by a crash.
    async fn recover_flows_batch(&self, batch_id: Uuid) -> Result<()> {
        let uri = self.uri(flows::TABLE);
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let flow_ids = cold::strings(uri, "flow_id", &filter).await?;
        if flow_ids.is_empty() {
            // Main append never committed; drop orphan participant rows, flows still hot.
            cold::delete(&self.uri(flow_participants::TABLE), &filter).await?;
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
        } else {
            self.finish_flows_batch(batch_id, &flow_ids).await?;
        }
        Ok(())
    }

    /// Reads cold flows matching a Lance filter.
    pub async fn cold_flows(&self, filter: &str) -> Result<Vec<flows::FlowRow>> {
        let uri = self.uri(flows::TABLE);
        let mut out = Vec::new();
        for b in cold::scan(&uri, filter).await? {
            out.extend(flows::from_record_batch(&b)?);
        }
        Ok(out)
    }

    /// Reads cold flow participants matching a Lance filter.
    pub async fn cold_flow_participants(
        &self,
        filter: &str,
    ) -> Result<Vec<flow_participants::FlowParticipantRow>> {
        let uri = self.uri(flow_participants::TABLE);
        let mut out = Vec::new();
        for b in cold::scan(&uri, filter).await? {
            out.extend(flow_participants::from_record_batch(&b)?);
        }
        Ok(out)
    }

    /// Loads a flow by ID, querying hot Postgres first and falling back to cold Lance.
    pub async fn load_flow(&self, flow_id: &str) -> Result<Option<flows::FlowRow>> {
        let hot: Option<flows::FlowRow> = sqlx::query_as(&format!(
            "SELECT {} FROM flows WHERE flow_id = $1 AND tiered_at IS NULL",
            flows::COLUMNS
        ))
        .bind(flow_id)
        .fetch_optional(&self.db)
        .await?;
        if hot.is_some() {
            return Ok(hot);
        }
        let filter = format!("flow_id = {}", cold::lit(flow_id));
        let cold = hot_only_on_error(flows::TABLE, self.cold_flows(&filter).await);
        Ok(cold.into_iter().next())
    }

    /// Loads all flows for a session, merging hot Postgres rows with cold Lance rows.
    pub async fn load_flows_for_session(&self, session_id: &str) -> Result<Vec<flows::FlowRow>> {
        let hot: Vec<flows::FlowRow> = sqlx::query_as(&format!(
            "SELECT {} FROM flows WHERE session_id = $1 AND tiered_at IS NULL
             ORDER BY created_at DESC",
            flows::COLUMNS
        ))
        .bind(session_id)
        .fetch_all(&self.db)
        .await?;

        let filter = format!("session_id = {}", cold::lit(session_id));
        let cold = hot_only_on_error(flows::TABLE, self.cold_flows(&filter).await);
        Ok(flows::merge(hot, cold))
    }

    /// All `trace_usage` rows for a session: hot Postgres merged with cold Lance.
    /// Matches rows keyed by the session id or by any of the session's flow ids
    /// (one flow == one trace), so rows written before `session_id` existed still count.
    pub async fn load_trace_usage_for_session(
        &self,
        session_id: &str,
        flow_ids: &[String],
    ) -> Result<Vec<TraceUsageRow>> {
        let hot: Vec<TraceUsageRow> = sqlx::query_as(&format!(
            "SELECT {} FROM trace_usage WHERE session_id = $1 OR trace_id = ANY($2)",
            trace_usage::COLUMNS
        ))
        .bind(session_id)
        .bind(flow_ids)
        .fetch_all(&self.db)
        .await?;
        let mut filter = format!("session_id = {}", cold::lit(session_id));
        if !flow_ids.is_empty() {
            let ids: Vec<String> = flow_ids.iter().map(|i| cold::lit(i)).collect();
            filter = format!("{filter} OR trace_id IN ({})", ids.join(", "));
        }
        let cold = hot_only_on_error(trace_usage::TABLE, self.cold_trace_usage(&filter).await);
        Ok(trace_usage::merge(hot, cold))
    }

    /// One trace's `trace_usage` rows, hot Postgres merged with cold Lance.
    pub async fn load_trace_usage_for_trace(&self, trace_id: &str) -> Result<Vec<TraceUsageRow>> {
        let hot: Vec<TraceUsageRow> = sqlx::query_as(&format!(
            "SELECT {} FROM trace_usage WHERE trace_id = $1",
            trace_usage::COLUMNS
        ))
        .bind(trace_id)
        .fetch_all(&self.db)
        .await?;
        let filter = format!("trace_id = {}", cold::lit(trace_id));
        let cold = hot_only_on_error(trace_usage::TABLE, self.cold_trace_usage(&filter).await);
        Ok(trace_usage::merge(hot, cold))
    }

    /// Reads cold `trace_usage` rows matching a Lance filter.
    async fn cold_trace_usage(&self, filter: &str) -> Result<Vec<TraceUsageRow>> {
        let mut out = Vec::new();
        for b in cold::scan(&self.uri(trace_usage::TABLE), filter).await? {
            out.extend(trace_usage::from_record_batch(&b)?);
        }
        Ok(out)
    }

    /// Settles one `writing` `tool_calls` batch left by a crash.
    async fn recover_tool_calls_batch(&self, batch_id: Uuid) -> Result<()> {
        let uri = self.uri(tool_calls::TABLE);
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let keys = cold::strings(&uri, "row_key", &filter).await?;
        if keys.is_empty() {
            // Main append never committed; drop orphan sidecar bodies, rows still hot.
            cold::delete(&self.uri(tool_calls::BODIES), &filter).await?;
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
        } else {
            self.finish_tool_calls_batch(batch_id, &keys).await?;
        }
        Ok(())
    }

    /// Phase 5 (TRD §3.4): appends chunks already selected for tiering to the cold
    /// `chunks` dataset. Postgres select/delete is wired by the EE memory crate
    /// (follow-up), because `memory_l1b_chunks` still takes `UPDATE`s.
    pub async fn append_cold_chunks(&self, rows: &[chunks::ChunkRow]) -> Result<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let batch = chunks::to_record_batch(rows, Uuid::new_v4())?;
        cold::append(&self.uri(chunks::TABLE), batch, &chunks::INDEXES).await
    }

    /// Builds the cold `IVF_PQ` index once the dataset can train it.
    pub async fn index_cold_chunks(&self, partitions: usize) -> Result<()> {
        cold::create_vector_index(
            &self.uri(chunks::TABLE),
            chunks::VECTOR_COLUMN,
            partitions,
            96,
        )
        .await
    }

    /// Cold ANN top-`k` for one user (cosine distance).
    pub async fn cold_chunk_search(
        &self,
        user_id: Uuid,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<chunks::ChunkHit>> {
        let filter = format!("user_id = {}", cold::lit(&user_id.to_string()));
        let mut out = Vec::new();
        for b in cold::nearest(
            &self.uri(chunks::TABLE),
            chunks::VECTOR_COLUMN,
            query,
            k,
            &filter,
            20,
        )
        .await?
        {
            out.extend(chunks::hits_from_record_batch(&b)?);
        }
        Ok(out)
    }

    /// `cold_chunk_search` with an extra Lance SQL filter (e.g. an `agent_domain` fence).
    pub async fn cold_chunk_search_where(
        &self,
        user_id: Uuid,
        query: &[f32],
        k: usize,
        extra: Option<&str>,
    ) -> Result<Vec<chunks::ChunkHit>> {
        let mut filter: String = format!("user_id = {}", cold::lit(&user_id.to_string()));
        if let Some(x) = extra {
            filter = format!("{filter} AND ({x})");
        }
        let mut out: Vec<chunks::ChunkHit> = Vec::new();
        for b in cold::nearest(
            &self.uri(chunks::TABLE),
            chunks::VECTOR_COLUMN,
            query,
            k,
            &filter,
            20,
        )
        .await?
        {
            out.extend(chunks::hits_from_record_batch(&b)?);
        }
        Ok(out)
    }

    /// Cold `tool_calls` for one session, oldest first.
    pub async fn session_tool_calls(
        &self,
        session_id: &str,
    ) -> Result<Vec<tool_calls::ToolCallRow>> {
        let filter = format!("session_id = {}", cold::lit(session_id));
        let mut cold_rows: Vec<tool_calls::ColdToolCall> = Vec::new();
        for b in cold::scan(&self.uri(tool_calls::TABLE), &filter).await? {
            cold_rows.extend(tool_calls::cold_from_record_batch(&b)?);
        }
        let ext: Vec<String> = cold_rows
            .iter()
            .filter(|c| c.body_external)
            .map(|c| cold::lit(&tool_calls::row_key(&c.row.trace_id, &c.row.call_id)))
            .collect();
        let mut bodies: Vec<tool_calls::ToolCallBody> = Vec::new();
        if !ext.is_empty() {
            let f = format!("row_key IN ({})", ext.join(", "));
            for b in cold::scan(&self.uri(tool_calls::BODIES), &f).await? {
                bodies.extend(tool_calls::bodies_from_record_batch(&b)?);
            }
        }
        let mut out: Vec<tool_calls::ToolCallRow> = tool_calls::fill_bodies(cold_rows, bodies);
        out.sort_by(|a, b| (a.started_at, &a.call_id).cmp(&(b.started_at, &b.call_id)));
        Ok(out)
    }

    /// All `tool_calls` for one session: Postgres rows plus tiered Lance rows,
    /// oldest first. A row in both stores (batch in Lance, not yet deleted) is
    /// returned once, from Postgres.
    pub async fn session_tool_calls_all(
        &self,
        session_id: &str,
    ) -> Result<Vec<tool_calls::ToolCallRow>> {
        let hot: Vec<tool_calls::ToolCallRow> = sqlx::query_as(&format!(
            "SELECT {} FROM tool_calls WHERE session_id = $1",
            tool_calls::COLUMNS
        ))
        .bind(session_id)
        .fetch_all(&self.db)
        .await?;
        let cold = self.session_tool_calls(session_id).await?;
        Ok(tool_calls::merge(hot, cold))
    }

    /// Phase 2: one `user_setting_changes` batch → Lance, then delete (no rollup).
    async fn tier_settings_batch(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        use user_setting_changes as usc;
        let mut rows: Vec<usc::SettingChangeRow> = sqlx::query_as(&format!(
            "SELECT {} FROM user_setting_changes WHERE changed_at < $1
             ORDER BY changed_at, id LIMIT $2",
            usc::COLUMNS
        ))
        .bind(cutoff)
        .bind(self.cfg.batch_rows)
        .fetch_all(&self.db)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let min_ts = rows.iter().map(|r| r.changed_at).min().unwrap();
        let max_ts = rows.iter().map(|r| r.changed_at).max().unwrap();
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(usc::TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;
        usc::sort_rows(&mut rows);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let batch = usc::to_record_batch(&rows, batch_id)?;
        let version = cold::append(&self.uri(usc::TABLE), batch, &usc::INDEXES).await?;
        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;
        self.finish_settings_batch(batch_id, &ids).await?;
        Ok(ids.len() as u64)
    }

    async fn finish_settings_batch(&self, batch_id: Uuid, ids: &[Uuid]) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM user_setting_changes WHERE id = ANY($1)")
            .bind(ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Settles one `writing` `user_setting_changes` batch left by a crash.
    async fn recover_settings_batch(&self, batch_id: Uuid) -> Result<()> {
        let uri = self.uri(user_setting_changes::TABLE);
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let ids = cold::strings(&uri, "id", &filter).await?;
        if ids.is_empty() {
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
        } else {
            let ids: Vec<Uuid> = ids
                .iter()
                .map(|s| Uuid::parse_str(s))
                .collect::<Result<_, _>>()?;
            self.finish_settings_batch(batch_id, &ids).await?;
        }
        Ok(())
    }

    /// Phase 2: one `token_savings` batch → Lance, then frozen-cost rollup + delete.
    async fn tier_savings_batch(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        let mut rows: Vec<token_savings::TokenSavingsRow> = sqlx::query_as(&format!(
            "SELECT {} FROM token_savings WHERE created_at < $1
             ORDER BY created_at, id LIMIT $2",
            token_savings::COLUMNS
        ))
        .bind(cutoff)
        .bind(self.cfg.batch_rows)
        .fetch_all(&self.db)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let min_ts = rows.iter().map(|r| r.created_at).min().unwrap();
        let max_ts = rows.iter().map(|r| r.created_at).max().unwrap();
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(token_savings::TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;
        token_savings::sort_rows(&mut rows);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let batch = token_savings::to_record_batch(&rows, batch_id)?;
        let version = cold::append(
            &self.uri(token_savings::TABLE),
            batch,
            &token_savings::INDEXES,
        )
        .await?;
        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;
        self.finish_savings_batch(batch_id, &ids).await?;
        Ok(ids.len() as u64)
    }

    /// Rollup reads the live `savings_spend` pricing for exactly these rows, so the
    /// frozen cost equals what the dashboard showed the moment before the delete.
    /// Price and session come from `trace_first_calls`, which still resolves a flow
    /// whose `trace_usage` rows were tiered first (kept in `trace_totals`).
    async fn finish_savings_batch(&self, batch_id: Uuid, ids: &[Uuid]) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO token_savings_daily AS d
             SELECT (ts.created_at AT TIME ZONE 'UTC')::date, ts.user_id,
                    COALESCE(ts.agent_id, '00000000-0000-0000-0000-000000000000'::uuid),
                    ts.layer, ts.program, COALESCE(ts.provider, ''), COALESCE(ts.model, ''),
                    COALESCE(ts.context_tier, ''),
                    count(*),
                    sum(ts.saved_input_tokens), sum(ts.saved_output_tokens),
                    sum(CASE WHEN NOT ts.token_estimated THEN ts.saved_input_tokens ELSE 0 END),
                    sum(CASE WHEN ts.token_estimated THEN ts.saved_input_tokens ELSE 0 END),
                    sum(ts.saved_cost_usd),
                    COALESCE(sum(CASE WHEN ts.model IS NULL
                         THEN ts.saved_input_tokens * COALESCE(price.rate, 0)
                         ELSE ts.saved_cost_usd END), 0)::float8,
                    COALESCE(sum(CASE WHEN ts.model IS NULL AND price.rate IS NULL
                         THEN ts.saved_input_tokens + ts.saved_output_tokens ELSE 0 END), 0)::bigint
             FROM token_savings ts
             LEFT JOIN LATERAL (
                 SELECT (mp.input_price_per_1m / 1000000.0)::FLOAT8 AS rate
                   FROM trace_first_calls tu
                   JOIN model_pricing mp
                     ON mp.provider = tu.provider AND mp.model = tu.model
                    AND mp.effective_from <= ts.created_at
                    AND (mp.effective_until IS NULL OR mp.effective_until > ts.created_at)
                  WHERE tu.trace_id = ts.flow_id
                  ORDER BY tu.started_at ASC, mp.effective_from DESC
                  LIMIT 1
             ) price ON true
             WHERE ts.id = ANY($1)
             GROUP BY 1, 2, 3, 4, 5, 6, 7, 8
             ON CONFLICT (day, user_id, agent_id, layer, program, provider, model, context_tier)
             DO UPDATE SET
                 rows = d.rows + EXCLUDED.rows,
                 saved_input_tokens = d.saved_input_tokens + EXCLUDED.saved_input_tokens,
                 saved_output_tokens = d.saved_output_tokens + EXCLUDED.saved_output_tokens,
                 calibrated_tokens = d.calibrated_tokens + EXCLUDED.calibrated_tokens,
                 uncalibrated_tokens = d.uncalibrated_tokens + EXCLUDED.uncalibrated_tokens,
                 raw_cost_usd = d.raw_cost_usd + EXCLUDED.raw_cost_usd,
                 priced_cost_usd = d.priced_cost_usd + EXCLUDED.priced_cost_usd,
                 unpriced_tokens = d.unpriced_tokens + EXCLUDED.unpriced_tokens",
        )
        .bind(ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO savings_flows AS f
             SELECT ts.flow_id, ts.user_id,
                    COALESCE(ts.agent_id, '00000000-0000-0000-0000-000000000000'::uuid),
                    (SELECT tr.session_id FROM trace_first_calls tr WHERE tr.trace_id = ts.flow_id
                        AND tr.session_id IS NOT NULL LIMIT 1),
                    min(ts.created_at),
                    sum(ts.saved_input_tokens), sum(ts.saved_output_tokens),
                    COALESCE(sum(CASE WHEN ts.model IS NULL
                         THEN ts.saved_input_tokens * COALESCE(price.rate, 0)
                         ELSE ts.saved_cost_usd END), 0)::float8
             FROM token_savings ts
             LEFT JOIN LATERAL (
                 SELECT (mp.input_price_per_1m / 1000000.0)::FLOAT8 AS rate
                   FROM trace_first_calls tu
                   JOIN model_pricing mp
                     ON mp.provider = tu.provider AND mp.model = tu.model
                    AND mp.effective_from <= ts.created_at
                    AND (mp.effective_until IS NULL OR mp.effective_until > ts.created_at)
                  WHERE tu.trace_id = ts.flow_id
                  ORDER BY tu.started_at ASC, mp.effective_from DESC
                  LIMIT 1
             ) price ON true
             WHERE ts.id = ANY($1) AND ts.flow_id IS NOT NULL
             GROUP BY ts.flow_id, ts.user_id, 3, 4
             ON CONFLICT (flow_id, user_id, agent_id) DO UPDATE SET
                 first_at = LEAST(f.first_at, EXCLUDED.first_at),
                 session_id = COALESCE(f.session_id, EXCLUDED.session_id),
                 saved_input_tokens = f.saved_input_tokens + EXCLUDED.saved_input_tokens,
                 saved_output_tokens = f.saved_output_tokens + EXCLUDED.saved_output_tokens,
                 priced_cost_usd = f.priced_cost_usd + EXCLUDED.priced_cost_usd",
        )
        .bind(ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM token_savings WHERE id = ANY($1)")
            .bind(ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Phase 2: one `context_reports` batch → Lance, then frozen rollup + delete.
    async fn tier_reports_batch(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        let mut rows: Vec<context_reports::ContextReportRow> = sqlx::query_as(&format!(
            "SELECT {} FROM context_reports WHERE created_at < $1
             ORDER BY created_at, id LIMIT $2",
            context_reports::COLUMNS
        ))
        .bind(cutoff)
        .bind(self.cfg.batch_rows)
        .fetch_all(&self.db)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let min_ts = rows.iter().map(|r| r.created_at).min().unwrap();
        let max_ts = rows.iter().map(|r| r.created_at).max().unwrap();
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(context_reports::TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;
        context_reports::sort_rows(&mut rows);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let batch = context_reports::to_record_batch(&rows, batch_id)?;
        let version = cold::append(
            &self.uri(context_reports::TABLE),
            batch,
            &context_reports::INDEXES,
        )
        .await?;
        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;
        self.finish_reports_batch(batch_id, &ids).await?;
        Ok(ids.len() as u64)
    }

    /// Agent resolved as in `context_savings::REPORTS_FROM`, price as in `PRICE_JOIN`,
    /// both frozen into the daily row.
    async fn finish_reports_batch(&self, batch_id: Uuid, ids: &[Uuid]) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO context_reports_daily AS d
             SELECT (cr.created_at AT TIME ZONE 'UTC')::date, cr.user_id,
                    COALESCE(eff.agent_id, '00000000-0000-0000-0000-000000000000'::uuid),
                    COALESCE(cr.strategy, ''), COALESCE(cr.level, ''), cr.fallback,
                    count(*),
                    count(*) FILTER (WHERE price.rate IS NOT NULL),
                    sum(cr.pool_tokens), sum(cr.kept_tokens),
                    sum(cr.pool_messages - cr.kept_messages),
                    sum(cr.compressed_bytes_saved),
                    COALESCE(sum(cr.pool_tokens * price.rate), 0)::float8,
                    COALESCE(sum(cr.kept_tokens * price.rate), 0)::float8
             FROM context_reports cr
             LEFT JOIN LATERAL (
                 SELECT f.root_agent_id AS agent_id FROM flows f
                  WHERE f.flow_id = cr.trace_id AND f.root_agent_id IS NOT NULL LIMIT 1
             ) st ON true
             LEFT JOIN LATERAL (
                 SELECT tu.agent_id FROM trace_first_calls tu
                  WHERE tu.trace_id = cr.trace_id AND tu.agent_id IS NOT NULL
                  ORDER BY tu.started_at ASC LIMIT 1
             ) tua ON true
             LEFT JOIN LATERAL (
                 SELECT COALESCE(cr.agent_id, st.agent_id, tua.agent_id) AS agent_id
             ) eff ON true
             LEFT JOIN LATERAL (
                 SELECT (mp.input_price_per_1m / 1000000.0)::FLOAT8 AS rate
                   FROM trace_first_calls tu
                   JOIN model_pricing mp
                     ON mp.provider = tu.provider AND mp.model = tu.model
                    AND mp.effective_from <= cr.created_at
                    AND (mp.effective_until IS NULL OR mp.effective_until > cr.created_at)
                  WHERE tu.trace_id = cr.trace_id
                  ORDER BY tu.started_at ASC, mp.effective_from DESC
                  LIMIT 1
             ) price ON true
             WHERE cr.id = ANY($1)
             GROUP BY 1, 2, 3, 4, 5, 6
             ON CONFLICT (day, user_id, agent_id, strategy, level, fallback)
             DO UPDATE SET
                 reports = d.reports + EXCLUDED.reports,
                 priced_reports = d.priced_reports + EXCLUDED.priced_reports,
                 pool_tokens = d.pool_tokens + EXCLUDED.pool_tokens,
                 kept_tokens = d.kept_tokens + EXCLUDED.kept_tokens,
                 messages_dropped = d.messages_dropped + EXCLUDED.messages_dropped,
                 compressed_bytes = d.compressed_bytes + EXCLUDED.compressed_bytes,
                 pool_cost = d.pool_cost + EXCLUDED.pool_cost,
                 sent_cost = d.sent_cost + EXCLUDED.sent_cost",
        )
        .bind(ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM context_reports WHERE id = ANY($1)")
            .bind(ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Settles one `writing` `context_reports` batch left by a crash.
    async fn recover_reports_batch(&self, batch_id: Uuid) -> Result<()> {
        let uri = self.uri(context_reports::TABLE);
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let ids = cold::strings(&uri, "id", &filter).await?;
        if ids.is_empty() {
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
        } else {
            let ids: Vec<Uuid> = ids
                .iter()
                .map(|s| Uuid::parse_str(s))
                .collect::<Result<_, _>>()?;
            self.finish_reports_batch(batch_id, &ids).await?;
        }
        Ok(())
    }

    /// Settles one `writing` `token_savings` batch left by a crash.
    async fn recover_savings_batch(&self, batch_id: Uuid) -> Result<()> {
        let uri = self.uri(token_savings::TABLE);
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let ids = cold::strings(&uri, "id", &filter).await?;
        if ids.is_empty() {
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
        } else {
            let ids: Vec<Uuid> = ids
                .iter()
                .map(|s| Uuid::parse_str(s))
                .collect::<Result<_, _>>()?;
            self.finish_savings_batch(batch_id, &ids).await?;
        }
        Ok(())
    }

    /// Cold `user_setting_changes` for one user in `[from, to)`, newest first.
    pub async fn user_setting_changes(
        &self,
        user_id: Uuid,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<user_setting_changes::SettingChangeRow>> {
        let filter = format!(
            "user_id = {} AND changed_at >= {} AND changed_at < {}",
            cold::lit(&user_id.to_string()),
            ts_lit(from),
            ts_lit(to)
        );
        let mut out = Vec::new();
        for b in cold::scan(&self.uri(user_setting_changes::TABLE), &filter).await? {
            out.extend(user_setting_changes::from_record_batch(&b)?);
        }
        out.sort_by_key(|r| std::cmp::Reverse((r.changed_at, r.id)));
        Ok(out)
    }

    /// Crash recovery for a `trace_usage` batch already in Lance: lock what is left of
    /// its rows, then rollup, delete and mark live in one transaction. Keys are
    /// `trace_id/agent_name`; already-deleted rows contribute nothing.
    async fn finish_trace_batch(&self, batch_id: Uuid, keys: &[String]) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "SELECT 1 FROM trace_usage WHERE trace_id || '/' || agent_name = ANY($1) FOR UPDATE",
        )
        .bind(keys)
        .execute(&mut *tx)
        .await?;
        finish_trace_rows(&mut tx, batch_id, keys).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Step 4: settle `writing` batches left by a crash, for every tiered table.
    ///
    /// Each batch on its own: one that cannot be settled (unreadable dataset, failing
    /// finish) marks only its table stuck, so other tables and erasures still run.
    async fn recover_all(&self) -> Recovery {
        let mut out = Recovery::default();
        for table in TIER_ORDER {
            let pending: Vec<Uuid> = match sqlx::query_scalar(
                "SELECT batch_id FROM tier_batches WHERE table_name = $1 AND state = 'writing'",
            )
            .bind(table)
            .fetch_all(&self.db)
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    metrics::metrics().failure("recover");
                    tracing::error!(target: "nasiko::tiering", table, error = %e, "listing writing batches failed");
                    out.stuck.push(table);
                    continue;
                }
            };
            for batch_id in pending {
                match self.recover_batch(table, batch_id).await {
                    Ok(()) => out.settled += 1,
                    Err(e) => {
                        metrics::metrics().failure("recover");
                        tracing::error!(target: "nasiko::tiering", table, %batch_id, error = %e, "could not settle tiering batch; table skipped this run");
                        if !out.stuck.contains(&table) {
                            out.stuck.push(table);
                        }
                    }
                }
            }
        }
        out
    }

    /// [`Self::recover_all`] for callers that must not go on past any unsettled
    /// batch: an error names the tables still holding one.
    async fn recover(&self) -> Result<usize> {
        let r: Recovery = self.recover_all().await;
        anyhow::ensure!(
            r.stuck.is_empty(),
            "unsettled tiering batches in: {}",
            r.stuck.join(", ")
        );
        Ok(r.settled)
    }

    /// One batch of the retired call log → cold `token_usage`, whatever its age: the
    /// rows are history only, nothing reads them hot. No rollups (spend is
    /// `trace_usage`'s). Drops the table once it is empty, so this runs to
    /// completion once and is then a single catalog lookup per run.
    async fn tier_legacy_usage_batch(&self) -> Result<u64> {
        if !self.legacy_usage_exists().await? {
            return Ok(0);
        }
        // No ORDER BY: the order of a drain does not matter, and sorting the whole
        // table for every batch would.
        let mut rows: Vec<TokenUsageRow> = sqlx::query_as(&format!(
            "SELECT {} FROM {LEGACY_USAGE} LIMIT $1",
            token_usage::COLUMNS
        ))
        .bind(self.cfg.batch_rows)
        .fetch_all(&self.db)
        .await?;
        if rows.is_empty() {
            sqlx::query(&format!("DROP TABLE IF EXISTS {LEGACY_USAGE}"))
                .execute(&self.db)
                .await?;
            tracing::info!(target: "nasiko::tiering", "legacy token_usage drained and dropped");
            return Ok(0);
        }
        let min_ts = rows.iter().map(|r| r.created_at).min().unwrap();
        let max_ts = rows.iter().map(|r| r.created_at).max().unwrap();
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;
        token_usage::sort_rows(&mut rows);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let batch = token_usage::to_record_batch(&rows, batch_id)?;
        let version = cold::append(&self.uri(TABLE), batch, &TOKEN_USAGE_INDEXES).await?;
        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;
        self.finish_legacy_usage_batch(batch_id, &ids).await?;
        Ok(ids.len() as u64)
    }

    /// Delete the copied rows and mark the batch live, in one transaction.
    /// Idempotent: rows already gone delete nothing.
    async fn finish_legacy_usage_batch(&self, batch_id: Uuid, ids: &[Uuid]) -> Result<()> {
        let mut tx = self.db.begin().await?;
        if self.legacy_usage_exists().await? {
            sqlx::query(&format!("DELETE FROM {LEGACY_USAGE} WHERE id = ANY($1)"))
                .bind(ids)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A drain batch interrupted mid-way: finish it if its Lance append committed,
    /// otherwise forget it (its rows are still in the legacy table).
    async fn recover_legacy_usage_batch(&self, batch_id: Uuid) -> Result<()> {
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let ids = cold::strings(&self.uri(TABLE), "id", &filter).await?;
        if ids.is_empty() {
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
            return Ok(());
        }
        let ids: Vec<Uuid> = ids
            .iter()
            .map(|s| Uuid::parse_str(s))
            .collect::<Result<_, _>>()?;
        self.finish_legacy_usage_batch(batch_id, &ids).await
    }

    async fn legacy_usage_exists(&self) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(LEGACY_USAGE)
            .fetch_one(&self.db)
            .await?)
    }

    async fn recover_batch(&self, table: &str, batch_id: Uuid) -> Result<()> {
        match table {
            trace_usage::TABLE => self.recover_trace_usage_batch(batch_id).await,
            user_setting_changes::TABLE => self.recover_settings_batch(batch_id).await,
            context_reports::TABLE => self.recover_reports_batch(batch_id).await,
            token_savings::TABLE => self.recover_savings_batch(batch_id).await,
            flows::TABLE => self.recover_flows_batch(batch_id).await,
            chat_messages::TABLE => self.recover_chat_batch(batch_id).await,
            tool_calls::TABLE => self.recover_tool_calls_batch(batch_id).await,
            TABLE => self.recover_legacy_usage_batch(batch_id).await,
            other => anyhow::bail!("not a tiered table: {other}"),
        }
    }

    /// One batch of messages from sessions idle since `cutoff` → Lance (+ sidecar
    /// bodies), then `cold_since` set and rows deleted under `nasiko.tiering = on`.
    async fn tier_chat_batch(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        use chat_messages as cm;
        let mut rows: Vec<cm::ChatMessageRow> = sqlx::query_as(&format!(
            "SELECT {} FROM chat_messages m JOIN chat_sessions s USING (session_id)
             WHERE m.timestamp < $1
               AND NOT EXISTS (SELECT 1 FROM chat_messages n
                               WHERE n.session_id = m.session_id AND n.timestamp >= $1)
             ORDER BY m.timestamp, m.id LIMIT $2",
            cm::COLUMNS
        ))
        .bind(cutoff)
        .bind(self.cfg.batch_rows)
        .fetch_all(&self.db)
        .await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let min_ts = rows.iter().map(|r| r.timestamp).min().unwrap();
        let max_ts = rows.iter().map(|r| r.timestamp).max().unwrap();
        let batch_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
             VALUES ($1, $2, $3, $4, $5, 'writing')",
        )
        .bind(cm::TABLE)
        .bind(batch_id)
        .bind(rows.len() as i64)
        .bind(min_ts)
        .bind(max_ts)
        .execute(&self.db)
        .await?;
        cm::sort_rows(&mut rows);
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let batches = cm::to_record_batches(&rows, batch_id, self.cfg.chat_body_inline_max)?;
        // Bodies first: a main row never points at a body that is not yet written.
        if let Some(b) = batches.bodies {
            cold::append(&self.uri(cm::BODIES), b, &cm::BODY_INDEXES).await?;
        }
        let version = cold::append(&self.uri(cm::TABLE), batches.main, &cm::INDEXES).await?;
        self.ensure_chat_fts().await;
        sqlx::query("UPDATE tier_batches SET lance_version = $2 WHERE batch_id = $1")
            .bind(batch_id)
            .bind(version as i64)
            .execute(&self.db)
            .await?;
        self.finish_chat_batch(batch_id, &ids).await?;
        Ok(ids.len() as u64)
    }

    async fn finish_chat_batch(&self, batch_id: Uuid, ids: &[Uuid]) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("SET LOCAL nasiko.tiering = 'on'")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE chat_sessions s SET cold_since = COALESCE(s.cold_since, now())
             WHERE s.session_id IN (SELECT session_id FROM chat_messages WHERE id = ANY($1))",
        )
        .bind(ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM chat_messages WHERE id = ANY($1)")
            .bind(ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Settles one `writing` `chat_messages` batch left by a crash.
    async fn recover_chat_batch(&self, batch_id: Uuid) -> Result<()> {
        let uri = self.uri(chat_messages::TABLE);
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let ids = cold::strings(&uri, "id", &filter).await?;
        if ids.is_empty() {
            // Main append never committed; drop orphan sidecar bodies, rows still hot.
            cold::delete(&self.uri(chat_messages::BODIES), &filter).await?;
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
        } else {
            let ids: Vec<Uuid> = ids
                .iter()
                .map(|s| Uuid::parse_str(s))
                .collect::<Result<_, _>>()?;
            self.finish_chat_batch(batch_id, &ids).await?;
        }
        Ok(())
    }

    /// Cold messages with external bodies filled from the sidecar.
    async fn cold_messages(&self, filter: &str) -> Result<Vec<chat_messages::ChatMessageRow>> {
        let mut cold_rows: Vec<chat_messages::ColdMessage> = Vec::new();
        for b in cold::scan(&self.uri(chat_messages::TABLE), filter).await? {
            cold_rows.extend(chat_messages::from_record_batch(&b)?);
        }
        let ext: Vec<String> = cold_rows
            .iter()
            .filter(|m| m.content_external)
            .map(|m| cold::lit(&m.row.id.to_string()))
            .collect();
        let mut bodies: std::collections::HashMap<Uuid, String> = Default::default();
        if !ext.is_empty() {
            let f = format!("id IN ({})", ext.join(", "));
            for b in cold::scan(&self.uri(chat_messages::BODIES), &f).await? {
                bodies.extend(chat_messages::bodies_from_record_batch(&b)?);
            }
        }
        Ok(cold_rows
            .into_iter()
            .map(|mut m| {
                if m.content_external {
                    m.row.content = bodies.remove(&m.row.id).unwrap_or_default();
                }
                m.row
            })
            .collect())
    }

    /// TRD C1: whole conversation, oldest first. `cold_since` null → Postgres only;
    /// else Lance by `session_id` merged with hot rows by `(timestamp, id)`.
    pub async fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<chat_messages::ChatMessageRow>> {
        let hot: Vec<chat_messages::ChatMessageRow> = sqlx::query_as(&format!(
            "SELECT {} FROM chat_messages m JOIN chat_sessions s USING (session_id)
             WHERE m.session_id = $1",
            chat_messages::COLUMNS
        ))
        .bind(session_id)
        .fetch_all(&self.db)
        .await?;
        let cold_since: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT cold_since FROM chat_sessions WHERE session_id = $1")
                .bind(session_id)
                .fetch_optional(&self.db)
                .await?
                .flatten();
        let cold_rows = if cold_since.is_some() {
            let started = std::time::Instant::now();
            let rows = self
                .cold_messages(&format!("session_id = {}", cold::lit(session_id)))
                .await;
            metrics::metrics().cold_read(chat_messages::TABLE, "load_session", started.elapsed());
            rows?
        } else {
            vec![]
        };
        Ok(chat_messages::merge(hot, cold_rows))
    }

    /// TRD §11: a user's messages containing every word of `query`, hot (Postgres
    /// FTS) + cold (Lance `INVERTED` index on `content`, prefiltered by `user_id`;
    /// `LIKE` scan if the index is absent), newest first. Cold hits are re-checked
    /// with `chat_messages::matches` so both paths return the same rows.
    pub async fn search_messages(
        &self,
        user_id: Uuid,
        query: &str,
        limit: usize,
    ) -> Result<Vec<chat_messages::ChatMessageRow>> {
        let terms: Vec<String> = chat_messages::terms(query);
        if terms.is_empty() {
            return Ok(vec![]);
        }
        let hot: Vec<chat_messages::ChatMessageRow> = sqlx::query_as(&format!(
            "SELECT {} FROM chat_messages m JOIN chat_sessions s USING (session_id)
             WHERE s.user_id = $1 AND to_tsvector('simple', m.content) @@ plainto_tsquery('simple', $2)
             ORDER BY m.timestamp DESC LIMIT $3",
            chat_messages::COLUMNS
        ))
        .bind(user_id)
        .bind(query)
        .bind(limit as i64)
        .fetch_all(&self.db)
        .await?;
        let started = std::time::Instant::now();
        let cold_res: Result<Vec<chat_messages::ChatMessageRow>> =
            self.search_cold(user_id, &terms).await;
        metrics::metrics().cold_read(chat_messages::TABLE, "search_messages", started.elapsed());
        let cold_rows: Vec<chat_messages::ChatMessageRow> = cold_res?;
        let mut out = chat_messages::merge(hot, cold_rows);
        out.reverse();
        out.truncate(limit);
        Ok(out)
    }

    /// Builds the cold chat `INVERTED` indexes if missing (later appends are folded
    /// in by `optimize_indices`). Best effort: search falls back to a scan.
    pub async fn ensure_chat_fts(&self) {
        for t in [chat_messages::TABLE, chat_messages::BODIES] {
            let uri: cold::Loc = self.uri(t);
            let res: Result<()> = async {
                if !cold::has_fts_index(&uri, "content").await? {
                    cold::create_fts_index(&uri, "content").await?;
                }
                Ok(())
            }
            .await;
            if let Err(e) = res {
                tracing::warn!(target: "nasiko::tiering", table = t, error = %e, "fts index build failed");
            }
        }
    }

    /// Cold rows of `table` matching `base` whose `content` holds every term:
    /// `INVERTED` index when present, else a `LIKE` scan.
    async fn cold_text_filter(&self, table: &str, base: &str, terms: &[String]) -> Result<String> {
        let uri: cold::Loc = self.uri(table);
        if cold::has_fts_index(&uri, "content").await? {
            let mut ids: Vec<String> = Vec::new();
            for b in cold::fts_search(&uri, "content", terms, base).await? {
                if let Some(a) = b
                    .column_by_name("id")
                    .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>())
                {
                    ids.extend(a.iter().flatten().map(cold::lit));
                }
            }
            if ids.is_empty() {
                return Ok("false".to_owned());
            }
            return Ok(format!("id IN ({})", ids.join(", ")));
        }
        Ok(format!(
            "{base} AND {}",
            chat_messages::like_filter("content", terms)
        ))
    }

    /// Cold half of `search_messages`: inline content plus sidecar bodies.
    async fn search_cold(
        &self,
        user_id: Uuid,
        terms: &[String],
    ) -> Result<Vec<chat_messages::ChatMessageRow>> {
        let user = format!("user_id = {}", cold::lit(&user_id.to_string()));
        let inline: String = self
            .cold_text_filter(chat_messages::TABLE, &user, terms)
            .await?;
        let mut cold_rows = self.cold_messages(&inline).await?;
        // Large bodies live in the sidecar: search them there, then load their rows.
        let big_ids: Vec<String> = cold::strings(
            &self.uri(chat_messages::BODIES),
            "id",
            &self
                .cold_text_filter(chat_messages::BODIES, &user, terms)
                .await?,
        )
        .await?;
        if !big_ids.is_empty() {
            let ids: Vec<String> = big_ids.iter().map(|i| cold::lit(i)).collect();
            cold_rows.extend(
                self.cold_messages(&format!("id IN ({})", ids.join(", ")))
                    .await?,
            );
        }
        cold_rows.retain(|r| chat_messages::matches(&r.content, terms));
        Ok(cold_rows)
    }

    /// Settles one `writing` `trace_usage` batch left by a crash.
    async fn recover_trace_usage_batch(&self, batch_id: Uuid) -> Result<()> {
        let uri = self.uri(trace_usage::TABLE);
        let filter = format!("batch_id = {}", cold::lit(&batch_id.to_string()));
        let keys = cold::strings(&uri, "row_key", &filter).await?;
        if keys.is_empty() {
            sqlx::query("DELETE FROM tier_batches WHERE batch_id = $1")
                .bind(batch_id)
                .execute(&self.db)
                .await?;
        } else {
            self.finish_trace_batch(batch_id, &keys).await?;
        }
        Ok(())
    }

    /// M7: one user's cold `token_usage` rows in `[from, to)`, via the `user_id` BTREE.
    pub async fn user_usage_rows(
        &self,
        user_id: Uuid,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<TokenUsageRow>> {
        let filter = format!(
            "user_id = {} AND created_at >= {} AND created_at < {}",
            cold::lit(&user_id.to_string()),
            ts_lit(from),
            ts_lit(to)
        );
        let mut out = Vec::new();
        for b in cold::scan(&self.uri(TABLE), &filter).await? {
            out.extend(token_usage::from_record_batch(&b)?);
        }
        token_usage::sort_rows(&mut out);
        Ok(out)
    }

    /// Records a pending erasure for `user_id` (idempotent). The tiering worker
    /// retries every pending row until [`Self::erase_user`] succeeds for it.
    pub async fn request_erasure(&self, user_id: Uuid) -> Result<()> {
        sqlx::query(
            "INSERT INTO erasure_requests (user_id) VALUES ($1) ON CONFLICT (user_id) DO NOTHING",
        )
        .bind(user_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Users whose erasure has not completed yet.
    pub async fn pending_erasures(&self) -> Result<Vec<Uuid>> {
        Ok(
            sqlx::query_scalar("SELECT user_id FROM erasure_requests ORDER BY requested_at")
                .fetch_all(&self.db)
                .await?,
        )
    }

    /// Batched delete of the user's Postgres rows (hot + rollups), no lock.
    ///
    /// Safe to run beside tiering: a batch tiered concurrently may re-add cold
    /// rows or rollup rows, which the locked pass in [`Self::erase_user`] removes.
    /// Run it before deleting the `users` row: `trace_usage.user_id` is
    /// `ON DELETE SET NULL`, so later the rows can no longer be found by user.
    pub async fn erase_hot(&self, user_id: Uuid) -> Result<u64> {
        let mut total: u64 = 0;
        for t in HOT_TABLES {
            let sql = format!(
                "DELETE FROM {t} WHERE ctid = ANY(ARRAY(
                     SELECT ctid FROM {t} WHERE user_id = $1 LIMIT $2))"
            );
            loop {
                // One short autocommit statement per batch: no long row locks.
                let n: u64 = sqlx::query(&sql)
                    .bind(user_id)
                    .bind(self.cfg.batch_rows.max(1))
                    .execute(&self.db)
                    .await
                    .with_context(|| format!("hot erase {t}"))?
                    .rows_affected();
                total += n;
                if n == 0 {
                    break;
                }
            }
        }
        total += self.erase_hot_legacy_usage(user_id).await?;
        Ok(total + self.erase_hot_chats(user_id).await?)
    }

    /// The user's rows still in the legacy call log, while it exists. A drain may
    /// drop the table between the check and the delete: that is "nothing left".
    async fn erase_hot_legacy_usage(&self, user_id: Uuid) -> Result<u64> {
        let sql = format!(
            "DELETE FROM {LEGACY_USAGE} WHERE ctid = ANY(ARRAY(
                 SELECT ctid FROM {LEGACY_USAGE} WHERE user_id = $1 LIMIT $2))"
        );
        let mut total: u64 = 0;
        while self.legacy_usage_exists().await? {
            let res = sqlx::query(&sql)
                .bind(user_id)
                .bind(self.cfg.batch_rows.max(1))
                .execute(&self.db)
                .await;
            let n: u64 = match res {
                Ok(r) => r.rows_affected(),
                Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("42P01") => 0,
                Err(e) => return Err(e).context("hot erase token_usage_legacy"),
            };
            total += n;
            if n == 0 {
                break;
            }
        }
        Ok(total)
    }

    /// Hot chat history, one batch of sessions at a time. Deleting a `chat_sessions`
    /// row cascades to its messages and fires any deletion triggers on both tables
    /// (EE memory), so nothing derived from them outlives the erase. A soft-deleted
    /// user (SCIM) has no FK cascade to do this. `hitl_requests.chat_session_id`
    /// has no `ON DELETE`, so the requests on those sessions go first.
    async fn erase_hot_chats(&self, user_id: Uuid) -> Result<u64> {
        let mut total: u64 = 0;
        loop {
            let mut tx = self.db.begin().await?;
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT session_id FROM chat_sessions WHERE user_id = $1 LIMIT $2",
            )
            .bind(user_id)
            .bind(self.cfg.batch_rows.max(1))
            .fetch_all(&mut *tx)
            .await
            .context("hot erase chat_sessions")?;
            if ids.is_empty() {
                return Ok(total);
            }
            sqlx::query("DELETE FROM hitl_requests WHERE chat_session_id = ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await
                .context("hot erase hitl_requests")?;
            total += sqlx::query("DELETE FROM chat_sessions WHERE session_id = ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await
                .context("hot erase chat_sessions")?
                .rows_affected();
            tx.commit().await?;
        }
    }

    /// TRD §9: erase one user from hot rows, rollups and every Lance dataset,
    /// then compact the touched datasets. Records a pending erasure first and
    /// clears it on success, so a crash or cold-storage error leaves it for the
    /// tiering worker. Takes the tiering lock; `Ok(false)` when tiering holds it
    /// (still pending). Idempotent.
    pub async fn erase_user(&self, user_id: Uuid) -> Result<bool> {
        self.request_erasure(user_id).await?;
        let mut lock_conn = self.db.acquire().await?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(TIERING_LOCK)
            .fetch_one(&mut *lock_conn)
            .await?;
        if !got {
            return Ok(false);
        }
        let res: Result<()> = self.erase_locked(user_id).await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(TIERING_LOCK)
            .execute(&mut *lock_conn)
            .await?;
        res.map(|()| true)
    }

    /// Caller holds the tiering lock. Settles crashed batches first so no
    /// `writing` batch re-adds the user's rows after the erase. A batch that cannot
    /// be settled holds back only its own table's datasets: the rest is erased and
    /// the request stays pending until a later pass can finish that table.
    async fn erase_locked(&self, user_id: Uuid) -> Result<()> {
        let res: Result<()> = async {
            let stuck: Vec<&'static str> = self.recover_all().await.stuck;
            self.erase_hot(user_id).await?;
            let filter = format!("user_id = {}", cold::lit(&user_id.to_string()));
            let mut held: Vec<&str> = Vec::new();
            for t in COLD_TABLES {
                if dataset_owner(t).is_some_and(|owner| stuck.contains(&owner)) {
                    held.push(t);
                    continue;
                }
                let uri: cold::Loc = self.uri(t);
                let n: usize = cold::count(uri.clone(), &filter)
                    .await
                    .with_context(|| format!("cold erase {t}"))?;
                if n == 0 {
                    continue; // rerun / never tiered: skip the rewrite
                }
                cold::delete(uri.clone(), &filter)
                    .await
                    .with_context(|| format!("cold erase {t}"))?;
                cold::purge(uri, self.cfg.version_grace)
                    .await
                    .with_context(|| format!("cold compact {t}"))?;
            }
            anyhow::ensure!(
                held.is_empty(),
                "cold erase held back by unsettled tiering batches: {}",
                held.join(", ")
            );
            Ok(())
        }
        .await;
        match &res {
            Ok(()) => {
                sqlx::query("DELETE FROM erasure_requests WHERE user_id = $1")
                    .bind(user_id)
                    .execute(&self.db)
                    .await?;
            }
            Err(e) => {
                sqlx::query(
                    "UPDATE erasure_requests SET attempts = attempts + 1, last_error = $2,
                         last_attempt_at = now() WHERE user_id = $1",
                )
                .bind(user_id)
                .bind(format!("{e:#}"))
                .execute(&self.db)
                .await?;
            }
        }
        res
    }

    /// Retries every pending erasure; failures stay pending. Returns completed count.
    async fn retry_erasures(&self) -> Result<usize> {
        let mut done: usize = 0;
        for user_id in self.pending_erasures().await? {
            match self.erase_locked(user_id).await {
                Ok(()) => done += 1,
                Err(e) => {
                    tracing::warn!(target: "nasiko::tiering", %user_id, error = %e, "erasure retry failed")
                }
            }
        }
        Ok(done)
    }

    /// Batched physical purge + compaction + version cleanup (weekly).
    pub async fn purge(&self, grace: TimeDelta) -> Result<()> {
        for t in COLD_TABLES {
            let started = std::time::Instant::now();
            cold::purge(&self.uri(t), grace).await?;
            metrics::metrics().maintenance(t, started.elapsed());
        }
        self.ensure_chat_fts().await;
        Ok(())
    }
}

/// Every Postgres table (raw and rollup) holding per-user tiered data.
const HOT_TABLES: [&str; 11] = [
    "trace_usage",
    "trace_usage_daily",
    "trace_totals",
    "user_setting_changes",
    "token_savings",
    "token_savings_daily",
    "savings_flows",
    "context_reports",
    "context_reports_daily",
    "tool_calls",
    "flows",
];

/// Every Lance dataset the store writes.
const COLD_TABLES: [&str; 12] = [
    chat_messages::TABLE,
    chat_messages::BODIES,
    chunks::TABLE,
    tool_calls::TABLE,
    tool_calls::BODIES,
    context_reports::TABLE,
    TABLE,
    trace_usage::TABLE,
    user_setting_changes::TABLE,
    token_savings::TABLE,
    flows::TABLE,
    flow_participants::TABLE,
];

/// Rollup into `trace_usage_daily` and `trace_totals`, delete, mark live, inside the
/// caller's transaction, which already holds the rows' locks.
async fn finish_trace_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    batch_id: Uuid,
    keys: &[String],
) -> Result<()> {
    sqlx::query(
        "INSERT INTO trace_usage_daily AS d (day, user_id, agent_id, agent_name, source, model,
             provider, traces, input_tokens, output_tokens, cache_read_tokens,
             cache_creation_tokens, cost_usd, prompt_cost_usd, completion_cost_usd,
             latency_sum, latency_count, tool_call_count, error_count)
         SELECT (started_at AT TIME ZONE 'UTC')::date,
                COALESCE(user_id, '00000000-0000-0000-0000-000000000000'::uuid),
                COALESCE(agent_id, '00000000-0000-0000-0000-000000000000'::uuid),
                agent_name, source, COALESCE(model, ''), COALESCE(provider, ''), count(*),
                sum(input_tokens), sum(output_tokens), sum(cache_read_tokens),
                sum(cache_creation_tokens), sum(cost_usd), sum(prompt_cost_usd),
                sum(completion_cost_usd), sum(COALESCE(latency_ms, 0)), count(latency_ms),
                sum(tool_call_count), sum(COALESCE(error_count, 0))
         FROM trace_usage
         WHERE trace_id || '/' || agent_name = ANY($1)
         GROUP BY 1, 2, 3, 4, 5, 6, 7
         ON CONFLICT (day, user_id, agent_id, agent_name, source, model, provider) DO UPDATE SET
             traces = d.traces + EXCLUDED.traces,
             input_tokens = d.input_tokens + EXCLUDED.input_tokens,
             output_tokens = d.output_tokens + EXCLUDED.output_tokens,
             cache_read_tokens = d.cache_read_tokens + EXCLUDED.cache_read_tokens,
             cache_creation_tokens = d.cache_creation_tokens + EXCLUDED.cache_creation_tokens,
             cost_usd = d.cost_usd + EXCLUDED.cost_usd,
             prompt_cost_usd = d.prompt_cost_usd + EXCLUDED.prompt_cost_usd,
             completion_cost_usd = d.completion_cost_usd + EXCLUDED.completion_cost_usd,
             latency_sum = d.latency_sum + EXCLUDED.latency_sum,
             latency_count = d.latency_count + EXCLUDED.latency_count,
             tool_call_count = d.tool_call_count + EXCLUDED.tool_call_count,
             error_count = d.error_count + EXCLUDED.error_count",
    )
    .bind(keys)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO trace_totals AS t (trace_id, session_id, user_id, first_at, tokens,
             cost_usd, errors, max_latency_ms, input_tokens, output_tokens, cache_read_tokens,
             cache_creation_tokens, model, source, first_provider, first_model, first_agent_id, row_count)
         SELECT trace_id, max(session_id),
                COALESCE(user_id, '00000000-0000-0000-0000-000000000000'::uuid),
                min(started_at),
                sum(input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens),
                sum(cost_usd), sum(COALESCE(error_count, 0)), max(latency_ms),
                  COALESCE(sum(input_tokens), 0), COALESCE(sum(output_tokens), 0),
                  COALESCE(sum(cache_read_tokens), 0), COALESCE(sum(cache_creation_tokens), 0),
                  CASE WHEN count(DISTINCT model) = 1 THEN min(model) END,
                  min(source),
                  -- What trace_first_calls serves once these rows are gone: the first
                  -- call with a model (prices) and the first known agent.
                  (array_agg(provider ORDER BY started_at) FILTER (WHERE model IS NOT NULL))[1],
                  (array_agg(model ORDER BY started_at) FILTER (WHERE model IS NOT NULL))[1],
                  (array_agg(agent_id ORDER BY started_at) FILTER (WHERE agent_id IS NOT NULL))[1],
                  count(*)
         FROM trace_usage
         WHERE trace_id || '/' || agent_name = ANY($1)
         GROUP BY 1, 3
         ON CONFLICT (trace_id, user_id) DO UPDATE SET
             session_id = COALESCE(t.session_id, EXCLUDED.session_id),
             first_at = LEAST(t.first_at, EXCLUDED.first_at),
             tokens = t.tokens + EXCLUDED.tokens,
               input_tokens = t.input_tokens + EXCLUDED.input_tokens,
               output_tokens = t.output_tokens + EXCLUDED.output_tokens,
               cache_read_tokens = t.cache_read_tokens + EXCLUDED.cache_read_tokens,
               cache_creation_tokens = t.cache_creation_tokens + EXCLUDED.cache_creation_tokens,
               model = CASE WHEN t.model IS NOT DISTINCT FROM EXCLUDED.model THEN t.model END,
             cost_usd = t.cost_usd + EXCLUDED.cost_usd,
             row_count = t.row_count + EXCLUDED.row_count,
             errors = t.errors + EXCLUDED.errors,
             max_latency_ms = GREATEST(t.max_latency_ms, EXCLUDED.max_latency_ms),
             -- The earlier batch's first call wins; a side with none defers to the other.
             first_provider = CASE WHEN EXCLUDED.first_model IS NOT NULL
                  AND (t.first_model IS NULL OR EXCLUDED.first_at < t.first_at)
                  THEN EXCLUDED.first_provider ELSE t.first_provider END,
             first_model = CASE WHEN EXCLUDED.first_model IS NOT NULL
                  AND (t.first_model IS NULL OR EXCLUDED.first_at < t.first_at)
                  THEN EXCLUDED.first_model ELSE t.first_model END,
             first_agent_id = CASE WHEN EXCLUDED.first_agent_id IS NOT NULL
                  AND (t.first_agent_id IS NULL OR EXCLUDED.first_at < t.first_at)
                  THEN EXCLUDED.first_agent_id ELSE t.first_agent_id END",
    )
    .bind(keys)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM trace_usage WHERE trace_id || '/' || agent_name = ANY($1)")
        .bind(keys)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE tier_batches SET state = 'live' WHERE batch_id = $1")
        .bind(batch_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The tiered table whose `tier_batches` write a cold dataset; `None` for datasets
/// written outside the batch protocol.
fn dataset_owner(dataset: &str) -> Option<&'static str> {
    match dataset {
        chat_messages::TABLE | chat_messages::BODIES => Some(chat_messages::TABLE),
        tool_calls::TABLE | tool_calls::BODIES => Some(tool_calls::TABLE),
        flows::TABLE | flow_participants::TABLE => Some(flows::TABLE),
        context_reports::TABLE => Some(context_reports::TABLE),
        trace_usage::TABLE => Some(trace_usage::TABLE),
        user_setting_changes::TABLE => Some(user_setting_changes::TABLE),
        token_savings::TABLE => Some(token_savings::TABLE),
        TABLE => Some(TABLE),
        _ => None,
    }
}

/// Hot table -> tiering time column, for the hot-age metric.
const HOT_TS_COLUMNS: [(&str, &str); 6] = [
    ("trace_usage", "started_at"),
    ("tool_calls", "started_at"),
    ("user_setting_changes", "changed_at"),
    ("token_savings", "created_at"),
    ("context_reports", "created_at"),
    ("flows", "completed_at"),
];

/// Never tier past a downstream consumer's read position.
pub fn hold_cutoff(cutoff: DateTime<Utc>, hold: Option<DateTime<Utc>>) -> DateTime<Utc> {
    hold.map_or(cutoff, |h| cutoff.min(h))
}

/// True on every `every`-th run; `every = 0` disables maintenance.
pub fn is_maintenance_run(runs: u32, every: u32) -> bool {
    every > 0 && runs.is_multiple_of(every)
}

/// Runs one `tier_*_batch` and records `nasiko.tiering.rows` + `batch.duration`
/// for non-empty batches (empty probes would skew the histogram).
async fn timed_batch(
    table: &str,
    fut: impl std::future::Future<Output = Result<u64>>,
) -> Result<u64> {
    let started = std::time::Instant::now();
    let n: u64 = fut.await?;
    if n > 0 {
        metrics::metrics().batch(table, n, started.elapsed());
    }
    Ok(n)
}

fn ts_lit(t: DateTime<Utc>) -> String {
    format!("TIMESTAMP '{}'", t.format("%Y-%m-%d %H:%M:%S%.6f"))
}

/// The cold half of a hot + cold read. An unreachable cold tier (a bucket the server lost access
/// to, a storage outage) is logged and read as empty, so the hot rows still answer: failing the
/// read instead turns every session and trace drill-down into a 500. Tiering keeps failing loudly.
fn hot_only_on_error<T>(table: &str, cold: Result<Vec<T>>) -> Vec<T> {
    cold.unwrap_or_else(|error| {
        tracing::warn!(target: "nasiko::tiering", table, error = %format!("{error:#}"),
            "cold tier unreadable; answering from hot rows only");
        Vec::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn cutoff_is_min_window() {
        let now = Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap();
        let cfg = TieringConfig::default();
        assert_eq!(
            tier_cutoff(now, &cfg),
            Utc.with_ymd_and_hms(2026, 9, 28, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn cutoff_never_tiers_today() {
        let now = Utc::now();
        let cfg = TieringConfig {
            min_hot_days: 0,
            ..Default::default()
        };
        assert!(tier_cutoff(now, &cfg) < now);
    }

    #[test]
    fn hold_caps_cutoff() {
        let c = Utc.with_ymd_and_hms(2026, 9, 28, 0, 0, 0).unwrap();
        let early = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let late = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
        assert_eq!(hold_cutoff(c, None), c);
        assert_eq!(hold_cutoff(c, Some(early)), early);
        assert_eq!(hold_cutoff(c, Some(late)), c);
    }

    #[test]
    fn maintenance_cadence() {
        let hits: Vec<u32> = (1..=15).filter(|r| is_maintenance_run(*r, 7)).collect();
        assert_eq!(hits, [7, 14]);
        assert!(!is_maintenance_run(7, 0));
    }

    #[test]
    fn unreadable_cold_tier_reads_as_empty() {
        let failed: Result<Vec<u8>> = Err(anyhow::anyhow!("403 Forbidden"));
        assert!(hot_only_on_error(flows::TABLE, failed).is_empty());
        assert_eq!(hot_only_on_error(flows::TABLE, Ok(vec![1, 2])), [1, 2]);
    }

    #[test]
    fn timestamp_literal() {
        let t = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        assert_eq!(ts_lit(t), "TIMESTAMP '2026-01-02 03:04:05.000000'");
    }
}
