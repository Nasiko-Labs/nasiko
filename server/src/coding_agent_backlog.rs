//! A once-a-minute report on coding-agent ingest: what the batch route did
//! (turns accepted, batches turned away by status) and what background work is
//! waiting (the OTLP outbox and the Tempo read-back).
//!
//! Written as a log line, which every deployment has, and as OTel gauges, for
//! one with a metrics pipeline. The line is `info` when anything happened or is
//! waiting, `debug` when idle, so a quiet server does not log every minute.

use std::time::Duration;

use opentelemetry::{global, metrics::Gauge};
use sqlx::PgPool;
use tokio::time::MissedTickBehavior;

use crate::coding_agent_ingest_guard::IngestMetrics;

const REPORT_EVERY: Duration = Duration::from_secs(60);

/// Background work waiting on coding-agent receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct Backlog {
    /// Receipts not yet exported to the OTel collector (pending, retrying or in flight).
    pub outbox_pending: i64,
    /// Exported receipts not yet read back from Tempo (legacy receipts only:
    /// ingest now writes a turn's usage itself).
    pub readback_pending: i64,
}

impl Backlog {
    /// Both counts are served by partial indexes over just the waiting rows
    /// (0071 and 0045), so this stays cheap however large the table grows.
    pub async fn measure(db: &PgPool) -> Result<Self, sqlx::Error> {
        sqlx::query_as(
            "SELECT
                 (SELECT count(*) FROM coding_agent_telemetry_events
                   WHERE otlp_state IN ('pending', 'failed', 'processing')) AS outbox_pending,
                 (SELECT count(*) FROM coding_agent_telemetry_events
                   WHERE otlp_trace_delivered_at IS NOT NULL AND materialized_at IS NULL)
                     AS readback_pending",
        )
        .fetch_one(db)
        .await
    }
}

/// Run forever, reporting every `REPORT_EVERY`.
pub async fn run(db: PgPool, metrics: IngestMetrics) {
    let meter = global::meter("nasiko.cp");
    let outbox: Gauge<u64> = meter
        .u64_gauge("nasiko.coding_agent.outbox.pending")
        .with_description("Coding-agent receipts waiting for OTLP export")
        .build();
    let readback: Gauge<u64> = meter
        .u64_gauge("nasiko.coding_agent.readback.pending")
        .with_description("Exported coding-agent receipts waiting for Tempo read-back")
        .build();
    let mut tick = tokio::time::interval(REPORT_EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tick.tick().await;
        let backlog = match Backlog::measure(&db).await {
            Ok(backlog) => backlog,
            Err(error) => {
                tracing::warn!(%error, "coding-agent backlog measurement failed");
                continue;
            }
        };
        outbox.record(u64::try_from(backlog.outbox_pending).unwrap_or(0), &[]);
        readback.record(u64::try_from(backlog.readback_pending).unwrap_or(0), &[]);
        let counts = metrics.take();
        let idle =
            counts.is_empty() && backlog.outbox_pending == 0 && backlog.readback_pending == 0;
        macro_rules! report {
            ($level:ident) => {
                tracing::$level!(
                    accepted = counts.accepted,
                    duplicate = counts.duplicate,
                    rejected = counts.rejected,
                    too_large_413 = counts.too_large,
                    rate_limited_429 = counts.rate_limited,
                    overloaded_503 = counts.overloaded,
                    outbox_pending = backlog.outbox_pending,
                    readback_pending = backlog.readback_pending,
                    "coding-agent ingest, last minute"
                )
            };
        }
        if idle {
            report!(debug);
        } else {
            report!(info);
        }
    }
}
