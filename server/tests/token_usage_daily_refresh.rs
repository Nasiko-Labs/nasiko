//! Regression guard for the hourly `token_usage_daily` refresh fixed in
//! `0056_token_usage_daily_concurrent_refresh.sql`.
//!
//! `server/src/state.rs` refreshes the view with `REFRESH MATERIALIZED VIEW
//! CONCURRENTLY`, which needs a unique index on plain columns. The index from
//! `0001_schema.sql` indexed `COALESCE(agent_id, …)`, an expression, so every
//! refresh failed with "cannot refresh materialized view concurrently". The task
//! only logs that, so the view silently kept the rows it was created with.
//!
//! Like `agent_status_check_constraint.rs`, this provisions an isolated
//! database and runs migrations, so it never depends on seed data.
//!
//! Run with infra up (`just infra`):
//!   `cargo test -p nasiko-server --test token_usage_daily_refresh`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// The statement the hourly task in `server/src/state.rs` runs.
const REFRESH: &str = "REFRESH MATERIALIZED VIEW CONCURRENTLY token_usage_daily";

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn token_usage_daily_refreshes_concurrently() {
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
    let db_name = format!("nasiko_test_usagedaily_{}", Uuid::new_v4().simple());
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

    result.expect("token_usage_daily refreshes concurrently");
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

    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("usagedaily-{}", Uuid::new_v4().simple()))
            .bind(format!("ud-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert user: {e}"))?;
    let agent: Uuid =
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ('agent', $1) RETURNING id")
            .bind(user)
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert agent: {e}"))?;

    // Usage without an agent is the case the old COALESCE index existed for:
    // those rows must still collapse into one group per day.
    let insert =
        "INSERT INTO token_usage (user_id, agent_id, operation_type, provider, model, input_tokens)
                  VALUES ($1, $2, 'chat', 'provider', 'model', $3)";
    for (agent_id, input_tokens) in [(None, 10), (Some(agent), 5)] {
        sqlx::query(insert)
            .bind(user)
            .bind(agent_id)
            .bind(input_tokens)
            .execute(&pool)
            .await
            .map_err(|e| format!("insert token_usage: {e}"))?;
    }
    sqlx::query(REFRESH)
        .execute(&pool)
        .await
        .map_err(|e| format!("first concurrent refresh: {e}"))?;

    // A second refresh must merge new rows into the existing groups.
    for (agent_id, input_tokens) in [(None, 20), (Some(agent), 7)] {
        sqlx::query(insert)
            .bind(user)
            .bind(agent_id)
            .bind(input_tokens)
            .execute(&pool)
            .await
            .map_err(|e| format!("insert token_usage: {e}"))?;
    }
    sqlx::query(REFRESH)
        .execute(&pool)
        .await
        .map_err(|e| format!("second concurrent refresh: {e}"))?;

    let groups: Vec<(Option<Uuid>, i64, i64)> = sqlx::query_as(
        "SELECT agent_id, request_count, total_input_tokens
         FROM token_usage_daily WHERE user_id = $1 ORDER BY agent_id NULLS FIRST",
    )
    .bind(user)
    .fetch_all(&pool)
    .await
    .map_err(|e| format!("read token_usage_daily: {e}"))?;
    let expected = vec![(None, 2, 30), (Some(agent), 2, 12)];
    if groups != expected {
        return Err(format!(
            "token_usage_daily has {groups:?}, expected {expected:?}"
        ));
    }

    Ok(())
}
