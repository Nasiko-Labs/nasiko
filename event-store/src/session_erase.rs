//! A deleted chat's cold copies (`chat_messages`, `chat_message_bodies`).
//!
//! Deleting a chat removes its `chat_sessions` row and, by cascade, its hot messages.
//! Tiered messages live only in Lance and are keyed by `session_id`, so without this
//! they would outlive the delete, and a session re-created under the same id would
//! merge them back in once it tiered. The request is recorded with the session delete
//! (`record_session_erasure`) and removed once the cold rows are gone; the tiering
//! worker retries the rest (`retry_session_erasures`).
//!
//! Only batches up to the request's `batch_watermark` are erased, so a re-created
//! session's own later batches are never touched. Deletion is logical (rows vanish
//! from reads at once); maintenance purges the old versions.

use anyhow::{Context, Result};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{EventStore, TIERING_LOCK, chat_messages, cold, flow_participants, flows};

/// Body ids per `id IN (...)` delete, to keep each filter small.
const ID_CHUNK: usize = 500;

impl EventStore {
    /// Records that `session_id`'s cold messages must go. Call in the transaction
    /// that deletes the `chat_sessions` row, so the delete and the request commit
    /// together. A repeat delete of a re-created session raises the watermark.
    pub async fn record_session_erasure(conn: &mut PgConnection, session_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO session_erasure_requests (session_id, batch_watermark)
             VALUES ($1, (SELECT COALESCE(max(id), 0) FROM tier_batches))
             ON CONFLICT (session_id) DO UPDATE
                 SET batch_watermark = GREATEST(session_erasure_requests.batch_watermark,
                                                EXCLUDED.batch_watermark)",
        )
        .bind(session_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Erases one recorded session now, under the tiering lock. `Ok(false)` when
    /// tiering holds the lock or no request is recorded (the worker retries pending ones).
    pub async fn erase_session(&self, session_id: &str) -> Result<bool> {
        let mut lock_conn = self.tiering_lock_connection().await?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(TIERING_LOCK)
            .fetch_one(&mut lock_conn)
            .await?;
        if !got {
            return Ok(false);
        }
        let res: Result<bool> = async {
            let watermark: Option<i64> = sqlx::query_scalar(
                "SELECT batch_watermark FROM session_erasure_requests WHERE session_id = $1",
            )
            .bind(session_id)
            .fetch_optional(&self.db)
            .await?;
            match watermark {
                Some(w) => {
                    // Settle a crashed batch first, so it cannot land the session's rows later.
                    self.recover().await?;
                    self.erase_session_locked(session_id, w).await?;
                    Ok(true)
                }
                None => Ok(false),
            }
        }
        .await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(TIERING_LOCK)
            .execute(&mut lock_conn)
            .await?;
        res
    }

    /// Caller holds the tiering lock (the tiering run, after `recover`). Failures stay
    /// pending with their error; returns how many sessions were erased.
    pub async fn retry_session_erasures(&self) -> Result<usize> {
        let pending: Vec<(String, i64)> = sqlx::query_as(
            "SELECT session_id, batch_watermark FROM session_erasure_requests ORDER BY requested_at",
        )
        .fetch_all(&self.db)
        .await?;
        let mut done: usize = 0;
        for (session_id, watermark) in pending {
            match self.erase_session_locked(&session_id, watermark).await {
                Ok(()) => done += 1,
                Err(e) => tracing::warn!(
                    target: "nasiko::tiering", session_id, error = %e, "session erasure retry failed"
                ),
            }
        }
        Ok(done)
    }

    /// Deletes the session's cold messages from batches up to `watermark`, bodies
    /// first (they are found through the main rows), then clears the request.
    /// On failure the request stays, with the attempt recorded.
    async fn erase_session_locked(&self, session_id: &str, watermark: i64) -> Result<()> {
        let res: Result<()> = async {
            let filter = self.session_filter(session_id, watermark).await?;
            let main: cold::Loc = self.uri(chat_messages::TABLE);
            let ids: Vec<String> = cold::strings(main.clone(), "id", &filter)
                .await
                .context("cold session erase: read ids")?;
            let bodies: cold::Loc = self.uri(chat_messages::BODIES);
            for chunk in ids.chunks(ID_CHUNK) {
                let in_list: Vec<String> = chunk.iter().map(|id| cold::lit(id)).collect();
                cold::delete(bodies.clone(), &format!("id IN ({})", in_list.join(", ")))
                    .await
                    .context("cold session erase: bodies")?;
            }
            if !ids.is_empty() {
                cold::delete(main, &filter)
                    .await
                    .context("cold session erase: messages")?;
            }

            // Cold flows and flow participants for this session
            let flows_filter = format!("session_id = {}", cold::lit(session_id));
            let flows_loc: cold::Loc = self.uri(flows::TABLE);
            let flow_ids: Vec<String> = cold::strings(flows_loc.clone(), "flow_id", &flows_filter)
                .await
                .context("cold session erase: read flow_ids")?;
            if !flow_ids.is_empty() {
                let parts_loc: cold::Loc = self.uri(flow_participants::TABLE);
                for chunk in flow_ids.chunks(ID_CHUNK) {
                    let in_list: Vec<String> = chunk.iter().map(|f| cold::lit(f)).collect();
                    cold::delete(
                        parts_loc.clone(),
                        &format!("flow_id IN ({})", in_list.join(", ")),
                    )
                    .await
                    .context("cold session erase: flow_participants")?;
                }
                cold::delete(flows_loc, &flows_filter)
                    .await
                    .context("cold session erase: flows")?;
            }

            Ok(())
        }
        .await;
        match &res {
            Ok(()) => {
                // Only the request this pass served: a newer delete may have raised it.
                sqlx::query(
                    "DELETE FROM session_erasure_requests
                     WHERE session_id = $1 AND batch_watermark <= $2",
                )
                .bind(session_id)
                .bind(watermark)
                .execute(&self.db)
                .await?;
            }
            Err(e) => {
                sqlx::query(
                    "UPDATE session_erasure_requests SET attempts = attempts + 1,
                         last_error = $2, last_attempt_at = now() WHERE session_id = $1",
                )
                .bind(session_id)
                .bind(format!("{e:#}"))
                .execute(&self.db)
                .await?;
            }
        }
        res
    }

    /// `session_id = …`, minus the chat batches newer than `watermark` (usually none:
    /// those hold a re-created session's rows, never the deleted one's).
    async fn session_filter(&self, session_id: &str, watermark: i64) -> Result<String> {
        let newer: Vec<Uuid> = sqlx::query_scalar(
            "SELECT batch_id FROM tier_batches WHERE table_name = $1 AND id > $2",
        )
        .bind(chat_messages::TABLE)
        .bind(watermark)
        .fetch_all(&self.db)
        .await?;
        Ok(session_filter(session_id, &newer))
    }
}

fn session_filter(session_id: &str, newer_batches: &[Uuid]) -> String {
    let mut filter = format!("session_id = {}", cold::lit(session_id));
    if !newer_batches.is_empty() {
        let ids: Vec<String> = newer_batches
            .iter()
            .map(|b| cold::lit(&b.to_string()))
            .collect();
        filter.push_str(&format!(" AND batch_id NOT IN ({})", ids.join(", ")));
    }
    filter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_escapes_the_session_id_and_excludes_newer_batches() {
        assert_eq!(session_filter("a'b", &[]), "session_id = 'a''b'");
        let b = Uuid::nil();
        assert_eq!(
            session_filter("s", &[b]),
            format!("session_id = 's' AND batch_id NOT IN ('{b}')")
        );
    }
}
