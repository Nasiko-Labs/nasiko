//! Postgres-backed session ↔ trace correlation resolver.
//!
//! `flows` rows (with `session_id`) record session ↔ trace mappings;
//! this resolver serves both directions for the observability provider, which
//! needs them for agents that never set `session.id` on their spans (anything
//! not running the Python auto-instrumentation patch).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nasiko_observability::provider::SessionIdResolver;
use sqlx::PgPool;

pub struct PgSessionIdResolver {
    db: PgPool,
}

impl PgSessionIdResolver {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SessionIdResolver for PgSessionIdResolver {
    async fn session_for_trace(&self, trace_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT session_id FROM flows WHERE flow_id = $1")
            .bind(trace_id)
            .fetch_optional(&self.db)
            .await
            .ok()
            .flatten()
    }

    /// Bounded by the caller's window and capped.
    async fn traces_for_session(
        &self,
        session_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT flow_id FROM flows \
             WHERE session_id = $1 AND created_at >= $2 AND created_at <= $3 \
             ORDER BY created_at \
             LIMIT 2000",
        )
        .bind(session_id)
        .bind(start)
        .bind(end)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
    }

    async fn traces_for_agent(
        &self,
        agent_name: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<String> {
        // Resolve flows whose root_agent_id matches an agent with this name.
        sqlx::query_scalar(
            "SELECT f.flow_id FROM flows f \
             JOIN agents a ON a.id = f.root_agent_id \
             WHERE a.name = $1 AND f.created_at >= $2 AND f.created_at <= $3 \
             ORDER BY f.created_at \
             LIMIT 1000",
        )
        .bind(agent_name)
        .bind(start)
        .bind(end)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
    }
}

/// Which of `trace_ids` the server's own records tie to `session_id`.
pub async fn traces_of_session(
    db: &PgPool,
    session_id: &str,
    trace_ids: &[String],
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    if trace_ids.is_empty() {
        return Ok(Default::default());
    }
    let kept: Vec<String> = sqlx::query_scalar(
        r#"SELECT t.trace_id FROM unnest($1::text[]) AS t(trace_id)
            WHERE CASE
                    WHEN EXISTS (SELECT 1 FROM flows f WHERE f.flow_id = t.trace_id)
                    THEN EXISTS (SELECT 1 FROM flows f
                                  WHERE f.flow_id = t.trace_id AND f.session_id = $2)
                    ELSE NOT EXISTS (
                           SELECT 1 FROM flows f
                            WHERE f.flow_id = t.trace_id
                              AND NOT EXISTS (SELECT 1 FROM chat_sessions c
                                               WHERE c.session_id = $2 AND c.user_id = f.user_id))
                  END"#,
    )
    .bind(trace_ids)
    .bind(session_id)
    .fetch_all(db)
    .await?;
    Ok(kept.into_iter().collect())
}

/// The session a trace's stored rows should be filed under: the server's
/// `flows` record when there is one, else the span's claim unless the
/// trace is a flow recorded for someone other than the claimed session's owner
/// (then none). See [`traces_of_session`].
pub async fn canonical_session(
    db: &PgPool,
    trace_id: &str,
    claimed: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let recorded: Option<String> =
        sqlx::query_scalar("SELECT session_id FROM flows WHERE flow_id = $1 LIMIT 1")
            .bind(trace_id)
            .fetch_optional(db)
            .await?;
    if recorded.is_some() {
        return Ok(recorded);
    }
    let Some(claimed) = claimed else {
        return Ok(None);
    };
    let ok = traces_of_session(db, claimed, &[trace_id.to_owned()])
        .await?
        .contains(trace_id);
    Ok(ok.then(|| claimed.to_owned()))
}
