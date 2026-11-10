//! Schema guard for migration `0083_agent_parking.sql`: the `parked` agent
//! status and the `sandbox_compatible` / `last_request_at` columns.
//!
//! Like `harness_sessions_schema.rs`, this provisions an isolated database and
//! runs migrations, so it never depends on seed data.
//!
//! Run with infra up (`just infra`):
//!   `cargo test -p nasiko-server --test agent_parking_schema`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn agent_parking_schema_is_in_place() {
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
    let db_name = format!("nasiko_test_parkingsch_{}", Uuid::new_v4().simple());
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

    result.expect("0081 agent parking schema is in place");
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
            .bind(format!("parkingsch-{}", Uuid::new_v4().simple()))
            .bind(format!("ps-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert user: {e}"))?;

    let agent: Uuid =
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("parkingsch-{}", Uuid::new_v4().simple()))
            .bind(owner)
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert agent: {e}"))?;

    // New columns exist and start NULL ("not yet probed" / "never served").
    let (compatible, last): (Option<bool>, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT sandbox_compatible, last_request_at FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("read parking columns: {e}"))?;
    if compatible.is_some() || last.is_some() {
        return Err("sandbox_compatible / last_request_at should default to NULL".into());
    }

    // Every status is accepted (the six that predate 'parked', and 'parked'); the columns
    // accept writes.
    for status in [
        "registered",
        "deploying",
        "running",
        "stopped",
        "failed",
        "crashed",
        "parked",
    ] {
        sqlx::query(
            "UPDATE agents SET status = $2, sandbox_compatible = true, last_request_at = now() \
             WHERE id = $1",
        )
        .bind(agent)
        .bind(status)
        .execute(&pool)
        .await
        .map_err(|e| format!("status '{status}' rejected: {e}"))?;
    }

    // The CHECK still rejects unknown statuses.
    match sqlx::query("UPDATE agents SET status = 'sleeping' WHERE id = $1")
        .bind(agent)
        .execute(&pool)
        .await
    {
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23514") => {}
        other => {
            return Err(format!(
                "status 'sleeping' was not rejected (23514): {other:?}"
            ));
        }
    }

    Ok(())
}
