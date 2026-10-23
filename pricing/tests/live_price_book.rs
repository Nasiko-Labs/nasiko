//! Resolver checks against a real `model_pricing` table.
//!
//! Ignored by default — needs `DATABASE_URL` pointing at a migrated database
//! (`just infra`). These exist because the defects this crate fixes were all
//! found in live data, not in reasoning: every model named here is one that the
//! running system actually failed to price.
//!
//! Run with: `cargo test -p nasiko-pricing --test live_price_book -- --ignored`

use chrono::Utc;
use nasiko_pricing::{
    CacheRatios, DbPriceBook, PriceBook, PriceSource, StaticPriceBook, load_cache_ratios, quote,
    resolve_model,
};
use sqlx::postgres::PgPoolOptions;

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for live tests");
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to the pricing database")
}

/// Every `(provider, model)` pair the live system has actually metered. The SQL
/// trigger prices 30 of 419 rows; nothing here may come back unpriced.
const OBSERVED_MODELS: &[(&str, &str)] = &[
    ("aws-bedrock", "openai.gpt-6-astra"),
    ("aws-bedrock", "qwen.qwen3-32b"),
    ("nebius-token-factory", "zai-org/GLM-5.1"),
    ("nebius-token-factory", "zai-org/GLM-5.2"),
    ("nebius-token-factory", "zai-org/GLM-5.3"),
    ("openai", "gpt-4o-mini"),
    ("anthropic", "claude-opus-5"),
    ("anthropic", "claude-opus-4-6"),
    ("anthropic", "claude-3-5-sonnet"),
];

#[tokio::test]
#[ignore = "needs DATABASE_URL and a migrated model_pricing table"]
async fn every_observed_model_resolves_to_four_usable_rates() {
    let db = pool().await;
    let synced = DbPriceBook::new(db.clone());
    let offline = StaticPriceBook;
    let books: Vec<&dyn PriceBook> = vec![&synced, &offline];
    let ratios = load_cache_ratios(&db).await;
    let now = Utc::now();

    for (provider, model) in OBSERVED_MODELS {
        let key = resolve_model(Some(provider), model);
        let q = quote(&books, &key, now, &ratios).await;

        for (label, rate) in [
            ("input", q.input_per_1m),
            ("output", q.output_per_1m),
            ("cache_read", q.cache_read_per_1m),
            ("cache_creation", q.cache_creation_per_1m),
        ] {
            assert!(
                rate.is_finite() && rate >= 0.0,
                "{provider}/{model}: {label} rate was {rate}"
            );
        }
        assert!(
            q.input_per_1m > 0.0,
            "{provider}/{model} priced input at zero — this is the $0 booking bug"
        );
        assert!(
            q.cache_read_per_1m < q.input_per_1m,
            "{provider}/{model}: cache reads at the input rate ({} vs {})",
            q.cache_read_per_1m,
            q.input_per_1m
        );
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL and a migrated model_pricing table"]
async fn a_coding_agent_model_reaches_its_family_row() {
    // claude-opus-4-6 is what a real Claude Code session reports. It has no row
    // of its own; `claude-opus-4` does, with the correct cache rates.
    let db = pool().await;
    let synced = DbPriceBook::new(db.clone());
    let offline = StaticPriceBook;
    let books: Vec<&dyn PriceBook> = vec![&synced, &offline];
    let ratios = load_cache_ratios(&db).await;

    let key = resolve_model(Some("anthropic"), "claude-opus-4-6");
    let q = quote(&books, &key, Utc::now(), &ratios).await;

    assert_eq!(q.input_per_1m, 15.00, "resolved as {}", q.resolved_as);
    assert_eq!(q.output_per_1m, 75.00);
    assert_eq!(q.cache_read_per_1m, 1.50);
    assert_eq!(q.cache_creation_per_1m, 18.75);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL and a migrated model_pricing table"]
async fn a_row_with_null_cache_columns_inherits_rather_than_shadows() {
    // claude-3-5-sonnet is seeded with NULL cache prices by migration 0006. The
    // live resolver bills its cache reads at $3.00/1M; the true rate is $0.30.
    let db = pool().await;
    let synced = DbPriceBook::new(db.clone());
    let offline = StaticPriceBook;
    let books: Vec<&dyn PriceBook> = vec![&synced, &offline];
    let ratios = load_cache_ratios(&db).await;

    let key = resolve_model(Some("anthropic"), "claude-3-5-sonnet");
    let q = quote(&books, &key, Utc::now(), &ratios).await;

    assert_eq!(
        q.input_per_1m, 3.00,
        "base rates must still come from the DB"
    );
    assert_eq!(
        q.cache_read_per_1m, 0.30,
        "cache reads must be inherited, not billed at the input rate"
    );
    assert_ne!(q.cache_source, PriceSource::Exact);
}

#[tokio::test]
#[ignore = "needs DATABASE_URL and a migrated model_pricing table + a synced Bedrock book"]
async fn bedrock_models_price_against_bedrock_rates_not_the_vendors_own() {
    // Bedrock resells at its own prices — Claude Opus is $5/$25 there against
    // $15/$75 direct, a 3x difference — and keys its book by the prefixed id
    // (`openai.gpt-6-astra`, `anthropic.claude-...`). Two normalization steps
    // used to walk past those rows: rewriting the provider label away from the
    // one the sync writes under, and stripping the prefix before trying it.
    let db = pool().await;
    let synced = DbPriceBook::new(db.clone());
    let offline = StaticPriceBook;
    let books: Vec<&dyn PriceBook> = vec![&synced, &offline];
    let ratios = load_cache_ratios(&db).await;

    let cases = [
        // (model as reported, expected input, expected output)
        ("openai.gpt-6-astra", 11.00, 55.00),
        ("anthropic.claude-opus-4-6-v1:0", 5.00, 25.00),
        ("anthropic.claude-sonnet-5", 2.00, 10.00),
    ];

    for (model, expected_input, expected_output) in cases {
        let key = resolve_model(Some("aws-bedrock"), model);
        let q = quote(&books, &key, Utc::now(), &ratios).await;
        assert_eq!(
            q.input_per_1m, expected_input,
            "{model}: input resolved as {} from {:?}",
            q.resolved_as, q.source
        );
        assert_eq!(q.output_per_1m, expected_output, "{model}: output");
        assert!(
            !q.estimated,
            "{model} should be a looked-up rate, not an inference"
        );
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL and a migrated model_pricing table"]
async fn ratios_derived_from_the_live_book_match_anthropic_published_rates() {
    let db = pool().await;
    let derived = load_cache_ratios(&db).await;
    let measured = CacheRatios::measured();

    for input_rate in [3.00, 15.00] {
        let (derived_read, derived_write) =
            derived.rates_for(nasiko_pricing::Vendor::Anthropic, input_rate);
        let (expected_read, expected_write) =
            measured.rates_for(nasiko_pricing::Vendor::Anthropic, input_rate);
        assert!(
            (derived_read - expected_read).abs() < 1e-9,
            "read ratio drifted: {derived_read} vs {expected_read}"
        );
        assert!(
            (derived_write - expected_write).abs() < 1e-9,
            "write ratio drifted: {derived_write} vs {expected_write}"
        );
    }
}

#[tokio::test]
#[ignore = "needs DATABASE_URL and a migrated model_pricing table"]
async fn a_call_before_the_first_row_takes_the_earliest_price_not_the_estimate() {
    // Rows open when they are written, so every call older than the first sync
    // used to miss them all and fall through to the generic estimate.
    let db = pool().await;
    let model = format!("history-check-{}", unique_suffix());
    let first_opened = Utc::now() - chrono::Duration::days(10);
    let changed_at = Utc::now() - chrono::Duration::days(5);

    sqlx::query(
        r#"INSERT INTO model_pricing
               (provider, model, input_price_per_1m, output_price_per_1m, effective_from, effective_until)
           VALUES ('test', $1, 1.0, 2.0, $2, $3),
                  ('test', $1, 3.0, 6.0, $3, NULL)"#,
    )
    .bind(&model)
    .bind(first_opened)
    .bind(changed_at)
    .execute(&db)
    .await
    .expect("insert price history");

    let cases = [
        (
            "before the first row",
            first_opened - chrono::Duration::days(30),
            1.0,
        ),
        (
            "while the first row was in force",
            first_opened + chrono::Duration::days(1),
            1.0,
        ),
        (
            "after the change",
            changed_at + chrono::Duration::days(1),
            3.0,
        ),
    ];
    // One book across all cases, so a cached row must not answer outside its window.
    let book = DbPriceBook::new(db.clone());
    let mut results = Vec::new();
    for (label, at, _) in cases {
        results.push((label, book.exact("test", &model, at).await));
    }

    sqlx::query("DELETE FROM model_pricing WHERE provider = 'test' AND model = $1")
        .bind(&model)
        .execute(&db)
        .await
        .expect("clean up price history");

    for ((label, row), (_, _, expected_input)) in results.into_iter().zip(cases) {
        let row = row.unwrap_or_else(|| panic!("{label}: no row"));
        assert_eq!(row.input_per_1m, expected_input, "{label}");
    }
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos()
}
