//! Regression guard for pricing calls older than a model's price history.
//!
//! `model_pricing` rows start at the moment they are written — on most installs
//! at first boot — so every imported transcript turn predates them. Those calls
//! fell through to the compiled-in list, which lags the synced book: Claude
//! Fable 5 priced as the generic `claude` entry at $3/$15 instead of $10/$50,
//! and OpenAI models missing from the list at the $2.50/$10 default.
//!
//! Like `agent_status_check_constraint.rs`, this provisions an isolated
//! database and runs migrations, so it never depends on seed data.
//!
//! Run with infra up (`just infra`):
//!   `cargo test -p nasiko-server --test pricing_backdated`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use chrono::{DateTime, Utc};
use nasiko_pricing::{PriceSource, PricingEngine, PromptConvention, RawUsage};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn calls_older_than_the_price_history_use_the_newest_row() {
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
    let db_name = format!("nasiko_test_backdated_{}", Uuid::new_v4().simple());
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

    result.expect("calls older than the price history use the newest row");
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

    // A first-boot placeholder corrected a day later, as 0046 did for Opus 5:
    // the earliest row is the wrong one to backdate.
    let model = format!("backdated-model-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO model_pricing
             (provider, model, input_price_per_1m, output_price_per_1m,
              cache_creation_price_per_1m, cache_read_price_per_1m, effective_from, effective_until)
         VALUES ('test-provider', $1, 15, 75, 18.75, 1.5, '2026-10-01', '2026-10-02'),
                ('test-provider', $1, 5, 25, 6.25, 0.5, '2026-10-02', NULL)",
    )
    .bind(&model)
    .execute(&pool)
    .await
    .map_err(|e| format!("insert model_pricing: {e}"))?;

    let engine = PricingEngine::new(pool);
    let expectations = [
        ("2026-09-01T00:00:00Z", 5.0, PriceSource::Backdated, true),
        ("2026-10-01T12:00:00Z", 15.0, PriceSource::Exact, false),
        ("2026-10-03T00:00:00Z", 5.0, PriceSource::Exact, false),
    ];
    for (at, input_per_1m, source, estimated) in expectations {
        let at: DateTime<Utc> = at.parse().expect("valid timestamp");
        let quote = engine
            .price(
                Some("test-provider"),
                &model,
                RawUsage {
                    input: 1_000_000,
                    output: 0,
                    cache_read: 0,
                    cache_creation: 0,
                    total: None,
                },
                PromptConvention::Exclusive,
                at,
            )
            .await
            .quote;
        let got = (quote.input_per_1m, quote.source, quote.estimated);
        if got != (input_per_1m, source, estimated) {
            return Err(format!(
                "call at {at} priced as {got:?}, expected {:?}",
                (input_per_1m, source, estimated)
            ));
        }
    }

    Ok(())
}
