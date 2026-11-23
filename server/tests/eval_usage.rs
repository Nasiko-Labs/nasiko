//! Replay evals end to end below the router: the `eval` coding-agent identity, the router's write
//! of a call's attempt tag (`trace_usage.eval_attempt`), and `/api/evals/{usage,price}` reading it
//! back with the same price book.
//!
//! Requires infra (Postgres :5432, Redis, S3); each test runs on its own throwaway database:
//!   cargo test -p nasiko-server --test eval_usage -- --test-threads=1

#[allow(dead_code)]
mod common;

use nasiko_llm_router::usage::{CallFailure, UsageRecord, log_usage};
use nasiko_pricing::{NoUsageLedger, PricingEngine, PromptConvention, RawUsage};
use serde_json::{Value, json};
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

/// The custom provider label a llama-server registered as "llamacpp" gets.
const LOCAL_PROVIDER: &str = "llamacpp";
const LOCAL_MODEL: &str = "flint";

async fn seed_user(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash, role)
         VALUES ($1, $1 || '@test.local', 'x', 'member') RETURNING id",
    )
    .bind(username)
    .fetch_one(db)
    .await
    .unwrap()
}

/// Registers the caller's `eval` identity through the API, as the eval runner does.
async fn register_eval_agent(server: &common::TestServer, user: Uuid, username: &str) -> Value {
    let response = common::as_member(
        server
            .client
            .post(server.url("/api/agents/coding-integrations")),
        &user.to_string(),
        username,
    )
    .json(&json!({ "integration_id": "eval" }))
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), 201);
    response.json().await.unwrap()
}

/// One served call of the eval agent with no flow, as the router records it.
fn call(owner: Uuid, agent: Uuid, model: (&str, &str), attempt: &str) -> UsageRecord {
    UsageRecord {
        owner_id: owner.to_string(),
        agent_id: agent.to_string(),
        operation_type: "direct_llm",
        provider: model.0.into(),
        model: model.1.into(),
        usage: Some(
            serde_json::from_value(json!({
                "prompt_tokens": 1000, "completion_tokens": 200, "total_tokens": 1200,
                "prompt_tokens_details": { "cached_tokens": 300 }
            }))
            .unwrap(),
        ),
        cached_tokens: None,
        reasoning_tokens: None,
        latency_ms: 10,
        streaming: false,
        finish_reason: Some("stop".into()),
        flow_id: None,
        attribution_source: None,
        platform_paid: true,
        compress_metadata: None,
        brevity_metadata: None,
        compact_tools_metadata: None,
        routing_metadata: None,
        compress_bytes: None,
        compact_tools_bytes: None,
        request_bytes: None,
        failure: None,
        eval_attempt: Some(attempt.into()),
    }
}

async fn write(db: &PgPool, pricing: &PricingEngine, record: UsageRecord) {
    log_usage(db.clone(), pricing, &NoUsageLedger, record)
        .await
        .unwrap();
}

/// What the operator does to make a local model free: a price row for the provider's label.
async fn price_local_model_at_zero(db: &PgPool) {
    sqlx::query(
        "INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m,
                                    cache_read_price_per_1m, cache_creation_price_per_1m, notes)
         VALUES ($1, $2, 0, 0, 0, 0, 'operator: local model, free')",
    )
    .bind(LOCAL_PROVIDER)
    .bind(LOCAL_MODEL)
    .execute(db)
    .await
    .unwrap();
}

async fn usage(server: &common::TestServer, user: Uuid, query: &str) -> (u16, Value) {
    let response = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/evals/usage?{query}"))),
        &user.to_string(),
        "evaluator",
    )
    .send()
    .await
    .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

#[tokio::test]
#[serial]
async fn a_runs_attempts_are_totalled_from_the_routers_rows_and_only_the_callers() {
    let server = common::TestServer::start().await;
    let db = &server.db;
    let user = seed_user(db, "evaluator").await;
    let other = seed_user(db, "someone-else").await;
    let registered = register_eval_agent(&server, user, "evaluator").await;
    assert_eq!(registered["coding_agent_integration_id"], "eval");
    assert_eq!(registered["name"], "evaluator-nasiko-eval");
    let agent: Uuid = registered["id"].as_str().unwrap().parse().unwrap();
    let other_agent: Uuid = register_eval_agent(&server, other, "someone-else").await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    price_local_model_at_zero(db).await;
    let book = PricingEngine::new(db.clone());
    let local = (LOCAL_PROVIDER, LOCAL_MODEL);
    let hosted = ("openai", "gpt-4o");

    write(db, &book, call(user, agent, local, "run-a:t1:1")).await;
    write(db, &book, call(user, agent, local, "run-a:t1:1")).await;
    write(db, &book, call(user, agent, hosted, "run-a:t2:1")).await;
    let failed = UsageRecord {
        usage: None,
        finish_reason: Some("failed:status".into()),
        request_bytes: Some(4_000),
        failure: Some(CallFailure::new("status", Some(503))),
        ..call(user, agent, hosted, "run-a:t2:1")
    };
    write(db, &book, failed).await;
    // Another run whose id merely starts with the same characters, and another user's same tag.
    write(db, &book, call(user, agent, local, "run-ab:t1:1")).await;
    write(db, &book, call(other, other_agent, local, "run-a:t1:1")).await;

    let (status, page) = usage(&server, user, "run=run-a").await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["has_more"], false);
    let attempts = page["data"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{page}");

    let free = &attempts[0];
    assert_eq!(free["attempt"], "run-a:t1:1");
    assert_eq!(free["calls"], 2);
    assert_eq!(free["failed_calls"], 0);
    assert_eq!(free["models"], json!([LOCAL_MODEL]));
    // OpenAI-style usage: the cached share is lifted out of the prompt count.
    assert_eq!(free["input_tokens"], 1400);
    assert_eq!(free["cache_read_tokens"], 600);
    assert_eq!(free["output_tokens"], 400);
    assert_eq!(free["cost_usd"], 0.0);
    assert_eq!(free["worst_price_source"], "Exact");
    assert_eq!(free["estimated_calls"], 0);

    let hosted_price = book
        .price(
            Some("openai"),
            "gpt-4o",
            RawUsage {
                input: 700,
                output: 200,
                cache_read: 300,
                cache_creation: 0,
                total: Some(1200),
            },
            PromptConvention::Exclusive,
            chrono::Utc::now(),
        )
        .await;
    let paid = &attempts[1];
    assert_eq!(paid["attempt"], "run-a:t2:1");
    assert_eq!(paid["calls"], 2);
    assert_eq!(paid["failed_calls"], 1);
    assert_eq!(paid["models"], json!(["gpt-4o"]));
    let cost = paid["cost_usd"].as_f64().unwrap();
    assert!(
        (cost - hosted_price.cost.total_usd).abs() < 1e-9,
        "{cost} vs {}",
        hosted_price.cost.total_usd
    );
    // The failed call's prompt was never reported, so its cost may be understated.
    assert!(paid["estimated_calls"].as_i64().unwrap() >= 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_long_run_pages_by_cursor_and_a_bad_run_is_a_coded_400() {
    let server = common::TestServer::start().await;
    let db = &server.db;
    let user = seed_user(db, "evaluator").await;
    let agent: Uuid = register_eval_agent(&server, user, "evaluator").await["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let book = PricingEngine::offline();
    for attempt in ["r:1", "r:2", "r:3"] {
        write(
            db,
            &book,
            call(user, agent, (LOCAL_PROVIDER, LOCAL_MODEL), attempt),
        )
        .await;
    }

    let (_, first) = usage(&server, user, "run=r&limit=2").await;
    assert_eq!(first["has_more"], true);
    assert_eq!(first["next_cursor"], "r:2");
    let tags: Vec<&str> = first["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["attempt"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["r:1", "r:2"]);

    let (_, rest) = usage(&server, user, "run=r&limit=2&cursor=r:2").await;
    assert_eq!(rest["has_more"], false);
    assert_eq!(rest["next_cursor"], Value::Null);
    assert_eq!(rest["data"][0]["attempt"], "r:3");
    // No price for flint in the offline book: the least certain source.
    assert_eq!(rest["data"][0]["worst_price_source"], "Default");
    assert_eq!(rest["data"][0]["estimated_calls"], 1);

    let (status, error) = usage(&server, user, "run=r%25").await;
    assert_eq!(status, 400);
    assert_eq!(error["code"], "invalid_run");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn price_uses_the_routers_price_book() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server.db, "evaluator").await;
    price_local_model_at_zero(&server.db).await;
    let post = |body: Value| {
        common::as_member(
            server.client.post(server.url("/api/evals/price")),
            &user.to_string(),
            "evaluator",
        )
        .json(&body)
        .send()
    };

    let free = post(json!({
        "provider": LOCAL_PROVIDER, "model": LOCAL_MODEL,
        "input_tokens": 5_000_000, "output_tokens": 1_000_000,
        "cache_read_tokens": 0, "cache_creation_tokens": 0
    }))
    .await
    .unwrap();
    assert_eq!(free.status(), 200);
    let free: Value = free.json().await.unwrap();
    assert_eq!(
        free["data"],
        json!({ "cost_usd": 0.0, "source": "Exact", "estimated": false })
    );

    let listed: Value = post(json!({
        "provider": "anthropic", "model": "claude-sonnet-4-20250514",
        "input_tokens": 1_000_000, "output_tokens": 0
    }))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(
        listed["data"]["cost_usd"].as_f64().unwrap() > 0.0,
        "{listed}"
    );

    let bad = post(json!({ "input_tokens": 1 })).await.unwrap();
    assert_eq!(bad.status(), 422);
    let bad: Value = bad.json().await.unwrap();
    assert_eq!(bad["code"], "invalid_body");

    server.cleanup().await;
}
