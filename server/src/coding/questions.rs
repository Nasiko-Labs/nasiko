//! The questions a coding turn asks a person: the `hitl_requests` rows its permission delegate
//! records ([`super::permission`]), origin `coding_agent`, keyed by the session (`context_id`) and
//! attributed to the session's user.
//!
//! A question lives no longer than the turn that asked it. When a turn ends, for any reason its
//! driver sees (completion, deadline, cancel, a lost lease), the driver closes the questions that
//! turn asked and nobody answered ([`TurnQuestions::close_open`]): no later turn delivers a late
//! answer, so leaving one open would only show a person a question nothing will act on. A turn
//! whose replica died closes nothing; its question stays open until a suspend of the session stops
//! the harness that asked it ([`SessionQuestions::close_asked_until`]).

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use nasiko_hitl::{HitlOrigin, HitlStatus};
use sqlx::PgPool;
use uuid::Uuid;

/// The questions one turn asked. The turn's delegate records each one as it is created; the
/// turn's driver closes the ones still open when the turn ends. Cloning shares the record.
#[derive(Clone, Debug, Default)]
pub struct TurnQuestions(Arc<Mutex<Vec<Uuid>>>);

impl TurnQuestions {
    /// Note that the turn asked `hitl_id`.
    pub fn record(&self, hitl_id: Uuid) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(hitl_id);
    }

    /// The questions recorded so far.
    pub fn asked(&self) -> Vec<Uuid> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Close every question this turn asked that is still open, as `expired`. One statement, and
    /// a no-op once done: an answered, rejected, cancelled or already expired question is left
    /// as it is. Returns how many it closed.
    pub async fn close_open(&self, db: &PgPool) -> sqlx::Result<u64> {
        let asked = self.asked();
        if asked.is_empty() {
            return Ok(0);
        }
        let done =
            sqlx::query("UPDATE hitl_requests SET status = $1 WHERE id = ANY($2) AND status = $3")
                .bind(HitlStatus::Expired.as_str())
                .bind(&asked)
                .bind(HitlStatus::Pending.as_str())
                .execute(db)
                .await?;
        Ok(done.rows_affected())
    }
}

/// The questions asked in one session, by its user.
#[derive(Clone, Copy, Debug)]
pub struct SessionQuestions<'a> {
    pub session_id: &'a str,
    pub owner: Uuid,
}

impl SessionQuestions<'_> {
    /// Close, as `expired`, the session's questions still open that were asked at or before
    /// `asked_until`. A question asked later belongs to a newer turn and is left alone. The owner
    /// filter lets the statement use the pending-owner index. Returns how many it closed.
    pub async fn close_asked_until(
        &self,
        db: &PgPool,
        asked_until: DateTime<Utc>,
    ) -> sqlx::Result<u64> {
        let done = sqlx::query(
            "UPDATE hitl_requests SET status = $1 \
             WHERE status = $2 AND origin = $3 AND owner_user_id = $4 AND context_id = $5 \
               AND created_at <= $6",
        )
        .bind(HitlStatus::Expired.as_str())
        .bind(HitlStatus::Pending.as_str())
        .bind(HitlOrigin::CodingAgent.as_str())
        .bind(self.owner)
        .bind(self.session_id)
        .bind(asked_until)
        .execute(db)
        .await?;
        Ok(done.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_shares_what_the_turn_asked() {
        let questions = TurnQuestions::default();
        let delegate_side = questions.clone();
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        delegate_side.record(first);
        delegate_side.record(second);
        assert_eq!(questions.asked(), vec![first, second]);
    }

    #[test]
    fn a_turn_that_asked_nothing_has_nothing_to_close() {
        assert!(TurnQuestions::default().asked().is_empty());
    }
}
