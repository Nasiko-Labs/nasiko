//! Regression guard for schema v2 (`010_schema_v2.sql`), which converted
//! `agent_builds.status` from `TEXT` to the `build_status` ENUM.
//!
//! Before the fix, `build/routes.rs` read the column into `String` and bound a
//! `&str` on write, which failed at runtime ("Rust type String … not compatible
//! with SQL type build_status" / "column status is of type build_status but
//! expression is of type text"). The fix uses a `BuildStatus` enum deriving
//! `sqlx::Type(type_name = "build_status")`. This test mirrors that mechanism and
//! verifies the live read/write paths succeed.
//!
//! Like the `common::TestServer` harness, this provisions an isolated database
//! and runs migrations, so it never depends on seed data (the OSS seed user was
//! removed) or a shared DB's state.
//!
//! Run with infra up (`docker compose --profile infra up -d postgres`):
//!   `cargo test -p nasiko-server --test build_status_enum_regression -- --nocapture`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use sqlx::FromRow;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// Same shape as the production enum in build/routes.rs — encodes/decodes the
// Postgres `build_status` enum directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "build_status", rename_all = "snake_case")]
enum BuildStatus {
    Queued,
    Building,
    Success,
    Failed,
}

#[derive(FromRow)]
struct StatusRow {
    status: BuildStatus,
}

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn agent_builds_status_reads_and_writes_via_enum() {
    let admin_dsn = admin_url();

    // Isolated test DB (mirrors common::TestServer) so we don't depend on seed data.
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
    let db_name = format!("nasiko_test_buildstatus_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin)
        .await
        .expect("create test database");

    let db_url = {
        // swap the trailing /<db> in the admin DSN for our test db
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

    result.expect("build_status enum read/write checks");
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

    // Seed our own user (the OSS seed user migration was removed).
    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("buildstatus-{}", Uuid::new_v4().simple()))
            .bind(format!("bs-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert user: {e}"))?;

    let agent_id: Uuid =
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("enumtest-{}", Uuid::new_v4().simple()))
            .bind(owner)
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert agent: {e}"))?;

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, 'v1', $2) RETURNING id",
    )
    .bind(agent_id)
    .bind("enumtest:v1")
    .fetch_one(&pool)
    .await
    .map_err(|e| format!("insert build: {e}"))?;

    // READ path: SELECT * decoding the enum column into BuildStatus (as BuildRecord does).
    let read: StatusRow = sqlx::query_as("SELECT * FROM agent_builds WHERE id = $1")
        .bind(build_id)
        .fetch_one(&pool)
        .await
        .map_err(|e| format!("READ (SELECT * -> BuildStatus): {e}"))?;
    if read.status != BuildStatus::Queued {
        return Err(format!("expected Queued, got {:?}", read.status));
    }

    // WRITE path: bind a BuildStatus to the enum column (as update_status does).
    sqlx::query("UPDATE agent_builds SET status = $2, updated_at = now() WHERE id = $1")
        .bind(build_id)
        .bind(BuildStatus::Building)
        .execute(&pool)
        .await
        .map_err(|e| format!("WRITE (bind BuildStatus): {e}"))?;

    Ok(())
}
