//! Integration tests for `GET /api/observability/finops/savings`.
//!
//! These exist because the savings aggregate is built with `QueryBuilder`, so none of its SQL is
//! checked at compile time. Six separate statements run per request — measured layers, actual
//! spend, three eligibility counts, per-agent, per-session, and three coverage queries — and a
//! typo in any of them is a 500 that nothing else catches. Every scope is exercised here for that
//! reason, not for the assertions on the numbers alone.
//!
//! The arithmetic assertions cover the two things most likely to be got wrong later:
//!   - the reduction is measured against the **baseline** (`actual + saved`), not the billed
//!     amount, so 250 saved on 1000 billed is 20% and not 25%;
//!   - a category with no eligible traffic returns a zero row **with a reason**, because an absent
//!     key and a real zero are indistinguishable to a consumer.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test finops_savings -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn admin_token(server: &common::TestServer) -> String {
    let body: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["data"]["token"]
        .as_str()
        .or_else(|| body["token"].as_str())
        .expect("initialize-admin returns a token")
        .to_string()
}

async fn savings(server: &common::TestServer, token: &str, query: &str) -> Value {
    let resp = server
        .client
        .get(server.url(&format!("/api/observability/finops/savings{query}")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "savings {query} failed: {}",
        resp.text().await.unwrap_or_default()
    );
    resp.json().await.unwrap()
}

/// One priced call plus the savings row that call produced.
struct Call<'a> {
    user_id: Uuid,
    agent_id: Uuid,
    flow_id: &'a str,
    input_tokens: i64,
    cost_usd: f64,
    saved_tokens: i64,
    saved_cost: f64,
}

async fn seed_call(db: &sqlx::PgPool, c: Call<'_>) {
    let Call {
        user_id,
        agent_id,
        flow_id,
        input_tokens,
        cost_usd,
        saved_tokens,
        saved_cost,
    } = c;
    sqlx::query(
        "INSERT INTO token_usage (user_id, agent_id, operation_type, provider, model,
             input_tokens, output_tokens, total_tokens, cost_usd, session_id, metadata)
         VALUES ($1,$2,'direct_llm','openai','gpt-4o',$3,0,$3,$4,$5,
                 '{\"brevity\": {\"applied\": true}}'::jsonb)",
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(input_tokens as i32)
    .bind(cost_usd)
    .bind(flow_id)
    .execute(db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO token_savings (user_id, agent_id, flow_id, provider, model, layer, program,
             bytes_before, bytes_after, saved_input_tokens, saved_cost_usd, method, token_estimated)
         VALUES ($1,$2,$3,'openai','gpt-4o','compress_payload','caveman',
                 40000,10000,$4,$5,'measured_bytes',false)",
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(flow_id)
    .bind(saved_tokens)
    .bind(saved_cost)
    .execute(db)
    .await
    .unwrap();
}

async fn seed_agent(db: &sqlx::PgPool, owner: Uuid, name: &str, compress: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agents (id, name, display_name, owner_id, status, version, description,
             url, compress_enabled)
         VALUES ($1,$2,$2,$3,'running','1.0.0','test','http://localhost:1',$4)",
    )
    .bind(id)
    .bind(name)
    .bind(owner)
    .bind(compress)
    .execute(db)
    .await
    .unwrap();
    id
}

async fn admin_user_id(db: &sqlx::PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn every_scope_runs_and_reports_both_percentages() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let user = admin_user_id(&server.db).await;
    let agent = seed_agent(&server.db, user, "savings-agent", true).await;

    // 1000 billed input tokens at $10, 250 tokens saved worth $2.50.
    seed_call(
        &server.db,
        Call {
            user_id: user,
            agent_id: agent,
            flow_id: "trace-a",
            input_tokens: 1000,
            cost_usd: 10.0,
            saved_tokens: 250,
            saved_cost: 2.5,
        },
    )
    .await;

    let body = savings(&server, &token, "?range=30d").await;
    let total = &body["data"]["total"];

    assert_eq!(total["saved_tokens"], 250);
    assert_eq!(total["actual_tokens"], 1000);
    // The reduction is against the baseline the run would have had, not against what was billed.
    assert_eq!(total["baseline_tokens"], 1250);
    assert_eq!(total["token_reduction_pct"], 20.0);
    // Both percentages are present and server-computed; a consumer never divides two fields itself.
    assert!(total["cost_reduction_pct"].is_number());

    // Every scope must execute its own SQL without erroring.
    for q in [
        "?range=30d&scope=total",
        "?range=30d&scope=agent",
        "?range=30d&scope=session",
    ] {
        let b = savings(&server, &token, q).await;
        assert!(b["data"]["total"]["saved_tokens"].is_number(), "{q}");
    }
}

#[tokio::test]
#[serial]
async fn the_agent_scope_reports_input_tokens_before_and_after() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let user = admin_user_id(&server.db).await;
    let agent = seed_agent(&server.db, user, "before-after-agent", true).await;
    seed_call(
        &server.db,
        Call {
            user_id: user,
            agent_id: agent,
            flow_id: "trace-b",
            input_tokens: 1000,
            cost_usd: 10.0,
            saved_tokens: 250,
            saved_cost: 2.5,
        },
    )
    .await;

    let body = savings(&server, &token, "?range=30d&scope=agent").await;
    let row = body["data"]["by_agent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["agent_name"] == "before-after-agent")
        .expect("the agent with savings must appear");

    assert_eq!(row["input_tokens_after"], 1000);
    // What the model would have read: what it did read, plus what was removed before it got there.
    assert_eq!(row["input_tokens_before"], 1250);
    assert_eq!(row["calls"], 1);
}

#[tokio::test]
#[serial]
async fn a_category_with_no_eligible_traffic_returns_zero_with_a_reason() {
    // An absent key and a real zero are indistinguishable to a consumer, and the difference
    // between "nobody enabled it" and "it did nothing" is the whole point of the coverage block.
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;

    let body = savings(&server, &token, "?range=30d").await;
    let ponytail = body["data"]["by_program"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["program"] == "ponytail")
        .expect("ponytail must be listed even at zero");

    assert_eq!(ponytail["saved_tokens"], 0);
    assert!(
        ponytail["note"]
            .as_str()
            .unwrap_or_default()
            .contains("coding agent"),
        "a zero row must say why, in words a non-engineer reads: {ponytail}"
    );
    // The assumption is the rate; the denominator is counted, and here it is genuinely empty.
    assert_eq!(ponytail["layers"][0]["factor"]["eligible_input_tokens"], 0);
    assert_eq!(ponytail["layers"][0]["basis"], "seed_default");
}

#[tokio::test]
#[serial]
async fn coverage_separates_optimized_from_unoptimized_spend() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let user = admin_user_id(&server.db).await;
    let on = seed_agent(&server.db, user, "opt-on", true).await;
    let off = seed_agent(&server.db, user, "opt-off", false).await;

    seed_call(
        &server.db,
        Call {
            user_id: user,
            agent_id: on,
            flow_id: "trace-c",
            input_tokens: 1000,
            cost_usd: 10.0,
            saved_tokens: 250,
            saved_cost: 2.5,
        },
    )
    .await;
    seed_call(
        &server.db,
        Call {
            user_id: user,
            agent_id: off,
            flow_id: "trace-d",
            input_tokens: 1000,
            cost_usd: 40.0,
            saved_tokens: 0,
            saved_cost: 0.0,
        },
    )
    .await;

    let body = savings(&server, &token, "?range=30d").await;
    let cov = &body["data"]["coverage"];

    assert_eq!(cov["agents_with_compress_enabled"], 1);
    assert!(cov["optimized_spend_usd"].as_f64().unwrap() >= 10.0);
    assert!(cov["unoptimized_spend_usd"].as_f64().unwrap() >= 40.0);
    // The biggest unoptimized spender is the actionable line: turn it on there next.
    assert_eq!(cov["top_unoptimized"]["agent_name"], "opt-off");
}

#[tokio::test]
#[serial]
async fn an_unknown_scope_is_rejected_rather_than_silently_defaulted() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;

    let resp = server
        .client
        .get(server.url("/api/observability/finops/savings?scope=everything"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
#[serial]
async fn the_total_scope_carries_both_rollups_so_a_panel_needs_one_request() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let user = admin_user_id(&server.db).await;
    let agent = seed_agent(&server.db, user, "both-rollups", true).await;
    seed_call(
        &server.db,
        Call {
            user_id: user,
            agent_id: agent,
            flow_id: "trace-e",
            input_tokens: 1000,
            cost_usd: 10.0,
            saved_tokens: 250,
            saved_cost: 2.5,
        },
    )
    .await;

    let body = savings(&server, &token, "?range=30d&scope=total").await;
    assert!(
        body["data"]["by_agent"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    );
    assert!(body["data"]["by_session"].is_array());

    // The narrower scopes return only what they name.
    let agents_only = savings(&server, &token, "?range=30d&scope=agent").await;
    assert_eq!(
        agents_only["data"]["by_session"].as_array().unwrap().len(),
        0
    );
}
