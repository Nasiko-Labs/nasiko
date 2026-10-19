//! Regression guard for the cost columns widened in `0055_widen_cost_usd.sql`.
//!
//! They were NUMERIC(10, 8), so any single row costing $100 or more failed
//! with "numeric field overflow". Coding-agent ingestion returned HTTP 500 and
//! the CLI quarantined the event after its retries, so the turn's usage was
//! never recorded.
//!
//! Like `agent_status_check_constraint.rs`, this provisions an isolated
//! database and runs migrations, so it never depends on seed data.
//!
//! Run with infra up (`just infra`):
//!   `cargo test -p nasiko-server --test cost_usd_precision`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// Over the old $99.99999999 ceiling, at the full eight-digit scale.
const COST_USD: &str = "1234.56789012";

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn cost_columns_store_costs_of_100_usd_and_more() {
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
    let db_name = format!("nasiko_test_costusd_{}", Uuid::new_v4().simple());
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

    result.expect("cost columns store costs of $100 and more");
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

    let cost: Decimal = COST_USD.parse().expect("COST_USD is a valid decimal");
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("costusd-{}", Uuid::new_v4().simple()))
            .bind(format!("cu-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("insert user: {e}"))?;
    let session = format!("ses_{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 'cost')")
        .bind(&session)
        .bind(user)
        .execute(&pool)
        .await
        .map_err(|e| format!("insert chat session: {e}"))?;

    let stored: [(&str, Result<Decimal, sqlx::Error>); 3] = [
        (
            "token_usage.cost_usd",
            sqlx::query_scalar(
                "INSERT INTO token_usage (user_id, operation_type, provider, model, cost_usd)
                 VALUES ($1, 'chat', 'provider', 'model', $2) RETURNING cost_usd",
            )
            .bind(user)
            .bind(cost)
            .fetch_one(&pool)
            .await,
        ),
        (
            "chat_messages.cost_usd",
            sqlx::query_scalar(
                "INSERT INTO chat_messages (session_id, role, content, cost_usd)
                 VALUES ($1, 'agent', '', $2) RETURNING cost_usd",
            )
            .bind(&session)
            .bind(cost)
            .fetch_one(&pool)
            .await,
        ),
        (
            "flows.total_cost_usd",
            sqlx::query_scalar(
                "INSERT INTO flows (flow_id, user_id, total_cost_usd)
                 VALUES ($1, $2, $3) RETURNING total_cost_usd",
            )
            .bind(format!("flow_{}", Uuid::new_v4().simple()))
            .bind(user)
            .bind(cost)
            .fetch_one(&pool)
            .await,
        ),
    ];
    for (column, value) in stored {
        let value = value.map_err(|e| format!("{column} rejected ${COST_USD}: {e}"))?;
        if value != cost {
            return Err(format!("{column} stored ${COST_USD} as ${value}"));
        }
    }

    // The migration drops and recreates the daily rollup to widen the column
    // beneath it, so check it still sums the widened values.
    sqlx::query("REFRESH MATERIALIZED VIEW token_usage_daily")
        .execute(&pool)
        .await
        .map_err(|e| format!("refresh token_usage_daily: {e}"))?;
    let daily: Decimal =
        sqlx::query_scalar("SELECT total_cost_usd FROM token_usage_daily WHERE user_id = $1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("read token_usage_daily: {e}"))?;
    if daily != cost {
        return Err(format!("token_usage_daily summed ${COST_USD} as ${daily}"));
    }

    Ok(())
}
