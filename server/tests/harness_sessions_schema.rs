//! Schema guard for migration `0081_harness_sessions.sql`: the `harness`
//! workload kind, the agent parking policy, the harness catalog seed rows and
//! the sandbox columns on `chat_sessions`.
//!
//! The existing-rows -> 'resident' flip is deliberately not covered: it is a
//! stored DEFAULT swap, and driving `Migrator.migrations` to split the run is
//! semver-fragile in sqlx 0.8.
//!
//! Like `agent_status_check_constraint.rs`, this provisions an isolated
//! database and runs migrations, so it never depends on seed data.
//!
//! Run with infra up (`just infra`):
//!   `cargo test -p nasiko-server --test harness_sessions_schema`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn harness_sessions_schema_is_in_place() {
    let admin_dsn = admin_url();

    let admin = match PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_dsn)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: cannot reach Postgres at {admin_dsn}: {e}");
            return;
        }
    };
    let db_name = format!("nasiko_test_harnesssch_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin)
        .await
        .expect("create test database");

    let db_url = {
        let base = admin_dsn
            .rsplit_once('/')
            .map(|(b, _)| b)
            .unwrap_or(&admin_dsn);
        format!("{base}/{db_name}")
    };

    let result = run_checks(&db_url).await;

    // Always drop the test DB, even on failure.
    let _ = sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    ))
    .execute(&admin)
    .await;

    result.expect("0079 harness schema is in place");
}

async fn insert_agent(
    pool: &PgPool,
    owner: Uuid,
    kind: &str,
    parking: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let name = format!("harnesssch-{}", Uuid::new_v4().simple());
    match parking {
        Some(p) => {
            sqlx::query_scalar(
                "INSERT INTO agents (name, owner_id, workload_kind, parking_policy) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(name)
            .bind(owner)
            .bind(kind)
            .bind(p)
            .fetch_one(pool)
            .await
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO agents (name, owner_id, workload_kind) \
                 VALUES ($1, $2, $3) RETURNING id",
            )
            .bind(name)
            .bind(owner)
            .bind(kind)
            .fetch_one(pool)
            .await
        }
    }
}

fn is_check_violation<T>(r: &Result<T, sqlx::Error>) -> bool {
    matches!(r, Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23514"))
}

async fn insert_with_harness(
    pool: &PgPool,
    owner: Uuid,
    kind: &str,
    harness_id: &str,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, workload_kind, harness_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("harnesssch-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .bind(kind)
    .bind(harness_id)
    .fetch_one(pool)
    .await
}

async fn run_checks(db_url: &str) -> Result<(), String> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .map_err(|e| format!("connect test db: {e}"))?;

    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;

    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("harnesssch-{}", Uuid::new_v4().simple()))
            .bind(format!("hs-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert user: {e}"))?;

    // (a) workload_kind accepts 'harness' and still rejects typos.
    let harness_agent = insert_agent(&pool, owner, "harness", None)
        .await
        .map_err(|e| format!("workload_kind 'harness' rejected: {e}"))?;
    if !is_check_violation(&insert_agent(&pool, owner, "bogus", None).await) {
        return Err("workload_kind 'bogus' was not rejected by the CHECK (23514)".into());
    }

    // (b) parking_policy defaults to 'auto' and rejects unknown values.
    let policy: String = sqlx::query_scalar("SELECT parking_policy FROM agents WHERE id = $1")
        .bind(harness_agent)
        .fetch_one(&pool)
        .await
        .map_err(|e| format!("read parking_policy: {e}"))?;
    if policy != "auto" {
        return Err(format!("parking_policy default is '{policy}', want 'auto'"));
    }
    if !is_check_violation(&insert_agent(&pool, owner, "agent", Some("sometimes")).await) {
        return Err("parking_policy 'sometimes' was not rejected by the CHECK (23514)".into());
    }

    // harness_id: FK to the catalog, and only meaningful for 'harness' rows.
    if insert_with_harness(&pool, owner, "harness", "nonexistent")
        .await
        .is_ok()
    {
        return Err("harness_id 'nonexistent' accepted; FK not enforcing".into());
    }
    insert_with_harness(&pool, owner, "harness", "opencode")
        .await
        .map_err(|e| format!("harness_id 'opencode' rejected: {e}"))?;
    if !is_check_violation(&insert_with_harness(&pool, owner, "agent", "opencode").await) {
        return Err("harness_id on a non-harness agent was not rejected (23514)".into());
    }

    // (c) the launch harnesses are seeded and acp_command round-trips.
    let ids = vec![
        "opencode".to_string(),
        "claude-code".to_string(),
        "codex".to_string(),
    ];
    let mut rows: Vec<(String, Vec<String>)> = sqlx::query_as(
        "SELECT id, acp_command FROM harness_catalog WHERE id = ANY($1) ORDER BY id",
    )
    .bind(&ids)
    .fetch_all(&pool)
    .await
    .map_err(|e| format!("read harness_catalog: {e}"))?;
    rows.sort();
    let expected = vec![
        (
            "claude-code".to_string(),
            vec!["claude-code-acp".to_string()],
        ),
        ("codex".to_string(), vec!["codex-acp".to_string()]),
        (
            "opencode".to_string(),
            vec!["opencode".to_string(), "acp".to_string()],
        ),
    ];
    if rows != expected {
        return Err(format!("harness_catalog seed mismatch: {rows:?}"));
    }

    // (d) chat_sessions carries the sandbox columns.
    let session_id = format!("ses_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO chat_sessions \
         (session_id, user_id, agent_id, title, sandbox_ref, suspended_at, workspace_repo, pending_permission) \
         VALUES ($1, $2, $3, 't', 'docker:abc', now(), 'https://example.com/r.git', $4)",
    )
    .bind(&session_id)
    .bind(owner)
    .bind(harness_agent)
    .bind(serde_json::json!({"tool": "write"}))
    .execute(&pool)
    .await
    .map_err(|e| format!("insert chat_sessions with sandbox columns: {e}"))?;

    let (sandbox_ref, repo, perm): (String, String, serde_json::Value) = sqlx::query_as(
        "SELECT sandbox_ref, workspace_repo, pending_permission FROM chat_sessions \
         WHERE session_id = $1 AND suspended_at IS NOT NULL",
    )
    .bind(&session_id)
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("read back session columns: {e}"))?;
    if sandbox_ref != "docker:abc" || repo != "https://example.com/r.git" || perm["tool"] != "write"
    {
        return Err("chat_sessions sandbox columns did not round-trip".into());
    }

    // (h) The concurrently built indexes (0088-0090) exist: reaper scan and the `tasks/*` lookups.
    for index in [
        "idx_chat_sessions_reapable",
        "idx_chat_sessions_last_turn",
        "idx_chat_sessions_active_turn",
    ] {
        let found: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_indexes WHERE indexname = $1")
            .bind(index)
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("read pg_indexes: {e}"))?;
        if found != 1 {
            return Err(format!("index {index} is missing"));
        }
    }

    Ok(())
}
