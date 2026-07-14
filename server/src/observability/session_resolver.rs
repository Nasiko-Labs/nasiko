//! Postgres-backed session ↔ trace correlation resolver.
//!
//! `agent_proxy` inserts one `session_traces` row per forwarded user query;
//! this resolver serves both directions for the observability provider, which
//! needs them for agents that never set `session.id` on their spans (anything
//! not running the Python auto-instrumentation patch).

use async_trait::async_trait;
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
        sqlx::query_scalar("SELECT session_id FROM session_traces WHERE trace_id = $1")
            .bind(trace_id)
            .fetch_optional(&self.db)
            .await
            .ok()
            .flatten()
    }

    async fn traces_for_session(&self, session_id: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT trace_id FROM session_traces WHERE session_id = $1 ORDER BY created_at",
        )
        .bind(session_id)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
    }
}
