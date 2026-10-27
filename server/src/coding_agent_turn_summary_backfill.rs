//! Summarizes coding-agent receipts that predate migration 0068's typed turn
//! columns, in bounded chunks, after the server has started.
//!
//! The migration used to do this itself with one table-wide UPDATE: every
//! receipt rewritten inside the deploy's migration transaction, each row locked
//! until the last was done, with ingest blocked behind it. Here each chunk is its
//! own short transaction over at most `CHUNK` rows, skipping rows another
//! instance holds, so a large table is summarized in the background while the
//! server serves traffic, and two replicas share the work instead of colliding.
//!
//! Same rules as ingest's `TurnSummary`: tokens are every class over every LLM
//! call, summed in numeric and clamped to `BIGINT` (the wire counts are u64, and
//! one hostile receipt must not wedge the pass); tool errors are calls that
//! ended `failed` or `timed_out`.

use std::time::Duration;

use sqlx::PgPool;

/// Receipts summarized per transaction.
const CHUNK: i64 = 500;
/// Pause between chunks, so the backfill never monopolizes the database.
const BETWEEN_CHUNKS: Duration = Duration::from_millis(100);
/// How often to look again once nothing is left. Cheap — the pending set is a
/// partial index (0072) that is empty once done — and it picks up receipts an
/// older binary still wrote, unsummarized, during a rolling deploy.
const RECHECK: Duration = Duration::from_secs(10 * 60);
/// Back-off after a failed chunk.
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(30);

/// Run forever: drain the backlog chunk by chunk, then recheck periodically.
pub async fn run(db: PgPool) {
    loop {
        match backfill_chunk(&db).await {
            Ok(0) => tokio::time::sleep(RECHECK).await,
            Ok(summarized) => {
                tracing::info!(summarized, "coding-agent turn summaries backfilled");
                tokio::time::sleep(BETWEEN_CHUNKS).await;
            }
            Err(error) => {
                tracing::warn!(%error, "coding-agent turn summary backfill failed");
                tokio::time::sleep(RETRY_AFTER_ERROR).await;
            }
        }
    }
}

/// Summarize at most one chunk; returns how many receipts it summarized.
///
/// A receipt whose payload has no `turn.started_at` (none the server accepted;
/// ingest validates the turn) falls back to `captured_at`, so every row it
/// selects leaves the pending set and the loop always makes progress.
pub async fn backfill_chunk(db: &PgPool) -> Result<u64, sqlx::Error> {
    // The immutability trigger compares only the receipt's identity columns, so
    // the summary columns are writable here.
    let summarized = sqlx::query(
        r#"WITH chunk AS (
               SELECT user_id, event_id
                 FROM coding_agent_telemetry_events
                WHERE turn_started_at IS NULL
                LIMIT $1
                  FOR UPDATE SKIP LOCKED
           )
           UPDATE coding_agent_telemetry_events e
              SET turn_started_at = COALESCE(
                      (e.payload->'turn'->>'started_at')::timestamptz, e.captured_at),
                  turn_ended_at = (e.payload->'turn'->>'ended_at')::timestamptz,
                  turn_tokens = (
                      SELECT LEAST(
                                 coalesce(sum((c->>'input_tokens')::numeric
                                              + (c->>'output_tokens')::numeric
                                              + (c->>'cache_read_tokens')::numeric
                                              + (c->>'cache_creation_tokens')::numeric), 0),
                                 9223372036854775807)::bigint
                        FROM jsonb_array_elements(
                                 coalesce(e.payload->'turn'->'llm_calls', '[]'::jsonb)) c
                  ),
                  turn_tool_errors = (
                      SELECT count(*)::integer
                        FROM jsonb_array_elements(
                                 coalesce(e.payload->'turn'->'tool_calls', '[]'::jsonb)) t
                       WHERE t->>'status' IN ('failed', 'timed_out')
                  )
             FROM chunk
            WHERE e.user_id = chunk.user_id AND e.event_id = chunk.event_id"#,
    )
    .bind(CHUNK)
    .execute(db)
    .await?
    .rows_affected();
    Ok(summarized)
}
