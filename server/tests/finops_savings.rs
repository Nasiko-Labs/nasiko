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

/// Seed an orchestrator-side savings row — the shape IP-3, IP-4 and context selection write.
///
/// `model`/`provider` are NULL and `saved_cost_usd` is 0 because none of those layers has a model
/// response in scope when it measures. The matching `token_usage` row is what the read path prices
/// them from, so `with_llm_call = false` reproduces a turn that never reached a model.
#[allow(clippy::too_many_arguments)]
async fn seed_unpriced_saving(
    db: &sqlx::PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    flow_id: &str,
    saved_tokens: i64,
    with_llm_call: bool,
    layer: &str,
    program_name: &str,
) {
    if with_llm_call {
        sqlx::query(
            "INSERT INTO token_usage (user_id, agent_id, operation_type, provider, model,
                 input_tokens, output_tokens, total_tokens, cost_usd, session_id)
             VALUES ($1,$2,'direct_llm','openai','gpt-4o',100,0,100,1.0,$3)",
        )
        .bind(user_id)
        .bind(agent_id)
        .bind(flow_id)
        .execute(db)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO token_savings (user_id, agent_id, flow_id, provider, model, layer, program,
             saved_input_tokens, saved_cost_usd, method, token_estimated)
         VALUES ($1,$2,$3,NULL,NULL,$5,$6,$4,0,'measured_bytes',true)",
    )
    .bind(user_id)
    .bind(agent_id)
    .bind(flow_id)
    .bind(saved_tokens)
    .bind(layer)
    .bind(program_name)
    .execute(db)
    .await
    .unwrap();
}

fn program<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["data"]["by_program"]
        .as_array()
        .expect("by_program is an array")
        .iter()
        .find(|p| p["program"] == name)
        .unwrap_or_else(|| panic!("no {name} program in {}", body["data"]["by_program"]))
}

/// The point of 4c. The orchestrator-side layers write tokens and no cost, because no model is
/// chosen when they measure; the read path prices them from the flow's own first LLM call. Without
/// the join these rows report a large token saving next to $0, which reads as "worth nothing".
#[tokio::test]
#[serial]
async fn orchestrator_side_savings_are_priced_from_the_flows_own_llm_call() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let user = admin_user_id(&server.db).await;
    let agent = seed_agent(&server.db, user, "priced-at-read", true).await;

    // gpt-4o input is $2.50/1M, so 400_000 saved tokens is exactly $1.00.
    seed_unpriced_saving(
        &server.db,
        user,
        agent,
        "trace-priced",
        400_000,
        true,
        "context_selection",
        "pacms",
    )
    .await;

    let body = savings(&server, &token, "?range=30d").await;
    let pacms = program(&body, "pacms");

    assert_eq!(pacms["saved_tokens"], 400_000);
    let cost = pacms["saved_cost_usd"].as_f64().expect("a cost figure");
    assert!(
        (cost - 1.0).abs() < 1e-6,
        "expected $1.00 from the flow's gpt-4o rate, got {cost}"
    );
    // A priced row has nothing to apologise for.
    assert!(
        pacms["note"].is_null(),
        "unexpected note: {}",
        pacms["note"]
    );
}

/// The honest half: a turn that never reached a model cannot be priced, and the response says so
/// instead of letting a zero stand next to a real token figure.
#[tokio::test]
#[serial]
async fn savings_that_cannot_be_priced_say_so_rather_than_reporting_zero_silently() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let user = admin_user_id(&server.db).await;
    let agent = seed_agent(&server.db, user, "unpriceable", true).await;

    seed_unpriced_saving(
        &server.db,
        user,
        agent,
        "trace-unpriced",
        12_345,
        false,
        "context_selection",
        "pacms",
    )
    .await;

    let body = savings(&server, &token, "?range=30d").await;
    let pacms = program(&body, "pacms");

    assert_eq!(pacms["saved_tokens"], 12_345);
    assert_eq!(pacms["saved_cost_usd"].as_f64(), Some(0.0));
    let note = pacms["note"]
        .as_str()
        .expect("an unpriced program explains itself");
    assert!(note.contains("12345"), "{note}");
    assert!(note.contains("could not be priced"), "{note}");
}

/// The same read-time pricing, for the other orchestrator-side layer.
///
/// `PRICED_COST` keys on `ts.model IS NULL` rather than on the layer, so IP-4 is priced by exactly
/// the same expression as context selection — but "it should follow" is not evidence, and these two
/// layers are the pair most likely to be changed together and broken apart. IP-4 rolls up under
/// `caveman`, so this also proves the join did not disturb IP-1's own already-priced row sitting in
/// the same program.
#[tokio::test]
#[serial]
async fn history_compression_is_priced_the_same_way_without_disturbing_ip1() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let user = admin_user_id(&server.db).await;
    let agent = seed_agent(&server.db, user, "ip4-priced", true).await;

    // IP-1: a row that priced itself at write time — $2.50 on 250 tokens.
    seed_call(
        &server.db,
        Call {
            user_id: user,
            agent_id: agent,
            flow_id: "trace-ip1",
            input_tokens: 1000,
            cost_usd: 10.0,
            saved_tokens: 250,
            saved_cost: 2.5,
        },
    )
    .await;

    // IP-4: tokens only, priced at read time. 400_000 at gpt-4o's $2.50/1M is $1.00.
    seed_unpriced_saving(
        &server.db,
        user,
        agent,
        "trace-ip4",
        400_000,
        true,
        "compress_history",
        "caveman",
    )
    .await;

    let body = savings(&server, &token, "?range=30d").await;
    let caveman = program(&body, "caveman");

    let layers = caveman["layers"].as_array().expect("layer breakdown");
    let ip4 = layers
        .iter()
        .find(|l| l["layer"] == "compress_history")
        .expect("compress_history must appear once it writes");
    let ip4_cost = ip4["saved_cost_usd"].as_f64().expect("a cost figure");
    assert!(
        (ip4_cost - 1.0).abs() < 1e-6,
        "IP-4 should price to $1.00 from the flow's gpt-4o rate, got {ip4_cost}"
    );

    // IP-1 keeps the figure its own writer computed at the blended effective rate. Re-pricing it
    // here at list would overstate it — the cached prefix is not billed at fresh-input rate.
    let ip1 = layers
        .iter()
        .find(|l| l["layer"] == "compress_payload")
        .expect("compress_payload row");
    assert_eq!(ip1["saved_cost_usd"].as_f64(), Some(2.5));

    // And the program total is the sum of both, not one or the other.
    let total = caveman["saved_cost_usd"].as_f64().expect("program cost");
    assert!(
        (total - 3.5).abs() < 1e-6,
        "caveman should total $3.50 (IP-1 $2.50 + IP-4 $1.00), got {total}"
    );
}
