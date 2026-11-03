//! Closes flows whose request died mid-flight.
//!
//! A flow row is written `running` when a request enters the proxy and updated when it ends. A
//! request that never ends cleanly (a crashed server, a dropped connection, an agent that hung)
//! leaves the row `running` for good, and its session reads as running forever. `FlowGuard` lives
//! in Redis and never touches the row, so nothing else would close it.
//!
//! A flow that has seen no activity for longer than a request can legitimately run is marked
//! `timed_out`, with its duration frozen at the last recorded activity. `paused` flows are left
//! alone: they wait on a person and expire on their own clock (`HITL_REQUEST_TTL_DAYS`).
//!
//! Every server process runs one sweeper. The `UPDATE` is guarded on `status = 'running'`, so a
//! late real completion is never overwritten and two sweepers cannot both claim a row.

use std::time::Duration;

use sqlx::PgPool;
use tokio::time::MissedTickBehavior;

/// How often the sweeper wakes.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Flows closed per statement, so one pass never holds locks on an unbounded number of rows.
const BATCH_SIZE: i64 = 500;

/// The longest a live request can go without activity: the flow's wall-clock budget, plus one
/// agent call that started just inside it.
pub fn stale_after(flow_timeout_secs: i32, agent_call_timeout_secs: u64) -> Duration {
    Duration::from_secs(u64::try_from(flow_timeout_secs).unwrap_or(0) + agent_call_timeout_secs)
}

/// Background loop: sweep once per [`SWEEP_INTERVAL`], forever. Spawned from
/// `AppState::from_config_with_db` like the server's other periodic workers. A failed pass is
/// logged and retried at the next tick.
pub async fn run(db: PgPool, stale_after: Duration) {
    let mut tick = tokio::time::interval(SWEEP_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // The first tick fires at once; nothing is worth sweeping at boot, as for the other workers.
    tick.tick().await;
    loop {
        tick.tick().await;
        match sweep(&db, stale_after).await {
            Ok(0) => tracing::debug!("flow sweep: nothing stale"),
            Ok(closed) => tracing::info!(closed, "flow sweep closed stale flows"),
            Err(e) => tracing::warn!(error = %e, "flow sweep failed, retrying next interval"),
        }
    }
}

/// Close every `running` flow idle for longer than `stale_after`, a batch at a time. Returns how
/// many it closed.
pub async fn sweep(db: &PgPool, stale_after: Duration) -> Result<u64, sqlx::Error> {
    let mut closed = 0;
    loop {
        let batch = close_batch(db, stale_after).await?;
        closed += batch;
        if batch < BATCH_SIZE as u64 {
            return Ok(closed);
        }
    }
}

async fn close_batch(db: &PgPool, stale_after: Duration) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"WITH idle AS (
               SELECT f.flow_id,
                      GREATEST(f.created_at,
                               (SELECT max(GREATEST(s.created_at, COALESCE(s.completed_at, s.created_at)))
                                  FROM flow_steps s
                                 WHERE s.flow_id = f.flow_id)) AS last_activity
                 FROM flows f
                WHERE f.status = 'running'
                  AND f.created_at < now() - make_interval(secs => $1)
                LIMIT $2
                  FOR UPDATE OF f SKIP LOCKED
           )
           UPDATE flows f
              SET status = 'timed_out',
                  completed_at = idle.last_activity,
                  duration_ms = (EXTRACT(EPOCH FROM (idle.last_activity - f.created_at)) * 1000)::bigint,
                  error_message = COALESCE(f.error_message, 'no activity before the flow timeout')
             FROM idle
            WHERE f.flow_id = idle.flow_id
              AND idle.last_activity < now() - make_interval(secs => $1)
              AND f.status = 'running'"#,
    )
    .bind(stale_after.as_secs_f64())
    .bind(BATCH_SIZE)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_may_run_for_its_flow_budget_plus_one_agent_call() {
        assert_eq!(stale_after(600, 600), Duration::from_secs(1200));
    }

    #[test]
    fn a_negative_flow_timeout_does_not_wrap() {
        assert_eq!(stale_after(-1, 600), Duration::from_secs(600));
    }
}
