//! Price book ingestion and DB-first duration pricing against an isolated database.
mod common;

use nasiko_llm_router::routing::pricing_sync::sync_provider_prices;
use nasiko_pricing::{PricingContext, PricingEngine, PromptConvention, RawUsage};
use serde_json::json;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn sync_is_provider_scoped_duration_aware_and_idempotent() {
    let server = common::TestServer::start().await;
    assert!(
        server
            .client
            .get(server.url("/health"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let mut upstream = mockito::Server::new_async().await;
    let book = json!({"claude-opus-5": {"pricing_config": {"pay_as_you_go": {
        "request_token": {"price": 0.0005}, "response_token": {"price": 0.0025},
        "cache_write_input_token": {"price": 0.000625}, "cache_read_input_token": {"price": 0.00005},
        "additional_units": {"cache_write_1h": {"price": 0.001}}
    }}}});
    let mock = upstream
        .mock("GET", "/pricing/anthropic.json")
        .with_status(200)
        .with_body(book.to_string())
        .expect(3)
        .create_async()
        .await;
    // The wrong provider already has precisely the incoming prices. It must not
    // suppress this provider's insert, nor have its row closed by this sync.
    sqlx::query("INSERT INTO model_pricing (provider,model,input_price_per_1m,output_price_per_1m,cache_creation_price_per_1m,cache_read_price_per_1m,cache_creation_1h_price_per_1m) VALUES ('other-host','claude-opus-5',5,25,6.25,0.5,10)")
        .execute(&server.db).await.unwrap();
    // Avoid borrowing the temporary URL across await.
    let url = upstream.url();
    let (a, b) = tokio::join!(
        sync_provider_prices(
            &server.db,
            &server.client,
            "test-anthropic",
            "https://api.anthropic.com",
            &url
        ),
        sync_provider_prices(
            &server.db,
            &server.client,
            "test-anthropic",
            "https://api.anthropic.com",
            &url
        )
    );
    assert_eq!(a.unwrap() + b.unwrap(), 1);
    assert_eq!(
        sync_provider_prices(
            &server.db,
            &server.client,
            "test-anthropic",
            "https://api.anthropic.com",
            &url
        )
        .await
        .unwrap(),
        0
    );
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM model_pricing WHERE model='claude-opus-5' AND provider IN ('other-host','test-anthropic') AND effective_until IS NULL")
        .fetch_one(&server.db).await.unwrap();
    assert_eq!(active, 2);
    let engine = PricingEngine::new(server.db.clone());
    let result = engine
        .price_with_context(
            Some("test-anthropic"),
            "claude-opus-5",
            RawUsage {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_creation: 1_000_000,
                total: None,
            },
            PromptConvention::Exclusive,
            chrono::Utc::now(),
            PricingContext {
                cache_creation_5m: Some(0),
                cache_creation_1h: Some(1_000_000),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(result.quote.cache_creation_1h_per_1m, Some(10.0));
    assert_eq!(result.cost.total_usd, 10.0);
    mock.assert_async().await;
    server.cleanup().await;
}

/// A full pass must fetch the Anthropic book whatever the agent table holds.
///
/// Coverage used to be conditional on a `coding_agent_integration_id = 'claude'`
/// row existing at the instant a pass ran. A pass runs at boot + 10s and then
/// once a day, so installing the integration a minute after boot left every
/// Claude call priced from the offline seed for the next 24 hours — which is how
/// Opus 5 was billed at the Opus 4 rate. The book is public and needs no key, so
/// nothing about it should depend on what is deployed.
#[tokio::test]
#[serial]
async fn anthropic_book_is_fetched_with_no_claude_agent_and_no_keys() {
    let server = common::TestServer::start().await;
    let claude_agents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agents WHERE coding_agent_integration_id = 'claude' AND deleted_at IS NULL",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(claude_agents, 0, "precondition: no Claude integration");

    let mut upstream = mockito::Server::new_async().await;
    let anthropic = upstream
        .mock("GET", "/pricing/anthropic.json")
        .with_status(200)
        // Deliberately not the real rate: the seeded row is already correct, so
        // only a value no seed carries proves the sync is what wrote the row.
        .with_body(
            json!({"claude-opus-5": {"pricing_config": {"pay_as_you_go": {
                "request_token": {"price": 0.0007}, "response_token": {"price": 0.0031}
            }}}})
            .to_string(),
        )
        .expect_at_least(1)
        .create_async()
        .await;
    // A pass also refreshes OpenRouter; keep the test off the public internet.
    let _openrouter = upstream
        .mock("GET", "/openrouter/models")
        .with_status(200)
        .with_body(json!({"data": []}).to_string())
        .create_async()
        .await;

    let url = upstream.url();
    // SAFETY: the pricing-sync tests are `#[serial]`, so no other thread in this
    // process is reading the environment while these are set.
    unsafe {
        std::env::set_var("PORTKEY_PRICING_BASE_URL", &url);
        std::env::set_var("OPENROUTER_MODELS_URL", format!("{url}/openrouter/models"));
    }
    nasiko_llm_router::routing::pricing_sync::sync_once(
        &server.db,
        &server.client,
        &nasiko_llm_router::GatewayConfig::default(),
    )
    .await;
    unsafe {
        std::env::remove_var("PORTKEY_PRICING_BASE_URL");
        std::env::remove_var("OPENROUTER_MODELS_URL");
    }

    anthropic.assert_async().await;
    let input: f64 = sqlx::query_scalar(
        "SELECT input_price_per_1m::float8 FROM model_pricing \
         WHERE provider = 'anthropic' AND model = 'claude-opus-5' AND effective_until IS NULL",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(input, 7.0, "the synced book must replace the seeded rate");
    server.cleanup().await;
}
