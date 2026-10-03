//! Load a [`UserHint`] from the Nasiko Postgres database for a given `user_id`.
//!
//! Uses runtime sqlx queries (no `!` macros) so the crate compiles without
//! a live DATABASE_URL or a `.sqlx` query cache. This is correct for a
//! library that is embedded in nasiko-server — the server owns the pool and
//! the migration state; user_hint.rs just reads.
//!
//! # Graceful degradation
//! Every query is wrapped in a fallible chain. DB unavailable, user not found,
//! or any NULL aggregate → the affected `Option` is `None`. If the top-level
//! user lookup fails the function returns `None` — the classifier then runs
//! with `user_hint: None`, identical to eval mode.

use sqlx::PgPool;
use uuid::Uuid;

use super::classifier::UserHint;

/// Load a `UserHint` for `user_id` from the Nasiko DB.
/// Returns `None` on any error so callers degrade gracefully.
pub async fn load_user_hint(pool: &PgPool, user_id: Uuid) -> Option<UserHint> {
    // ── 1. Basic user info ────────────────────────────────────────────────
    let row = sqlx::query(
        r#"
        SELECT role::text AS role, is_superuser
        FROM   users
        WHERE  id = $1
          AND  deleted_at IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()??; // outer ? = DB error, inner ? = row not found

    use sqlx::Row as _;
    let role: Option<String> = row.try_get("role").ok();
    let is_superuser: bool = row.try_get("is_superuser").unwrap_or(false);
    let is_admin = is_superuser || role.as_deref() == Some("admin");

    // ── 2. Session count ──────────────────────────────────────────────────
    let session_count: Option<u32> = sqlx::query(
        "SELECT COUNT(*)::int AS n FROM chat_sessions WHERE user_id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .ok()
    .and_then(|r| r.try_get::<Option<i32>, _>("n").ok().flatten())
    .map(|n| n as u32);

    // ── 3. Average input tokens of the user's last 10 messages ───────────
    let avg_token_length: Option<u32> = sqlx::query(
        r#"
        SELECT AVG(m.input_tokens)::int AS avg
        FROM   chat_messages  m
        JOIN   chat_sessions  s ON s.session_id = m.session_id
        WHERE  s.user_id      = $1
          AND  m.role         = 'user'
          AND  m.input_tokens IS NOT NULL
        ORDER  BY m.timestamp DESC
        LIMIT  10
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .ok()
    .and_then(|r| r.try_get::<Option<i32>, _>("avg").ok().flatten())
    .map(|n| n as u32);

    // ── 4. Most-used agent name ────────────────────────────────────────────
    let primary_agent_name: Option<String> = sqlx::query(
        r#"
        SELECT   a.name
        FROM     chat_sessions s
        JOIN     agents        a ON a.id = s.agent_id
        WHERE    s.user_id    = $1
          AND    s.agent_id   IS NOT NULL
          AND    s.deleted_at IS NULL
          AND    a.deleted_at IS NULL
        GROUP BY a.name
        ORDER BY COUNT(*) DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .and_then(|r| r.try_get::<String, _>("name").ok());

    Some(UserHint {
        session_count,
        avg_token_length,
        primary_agent_name,
        is_admin,
    })
}
