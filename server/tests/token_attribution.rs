//! End-to-end token attribution through the mounted LLM router.
//!
//! The bug this PR exists to fix was a *billing* bug: an agent's LLM spend
//! landed on the agent's owner instead of the user whose message triggered it,
//! and `session_id` was left NULL so nothing aggregated per conversation. The
//! rule that replaced it is strict — the `traceparent` must name a live flow the
//! calling agent participates in, or the call is refused before any tokens are
//! spent (`oss/docs/TOKEN_ATTRIBUTION.md`).
//!
//! Every branch of `attribution::resolve` is unit-tested inside the router
//! crate. What was untested is the part those unit tests cannot reach: that a
//! real HTTP request, through the real handler, against a real database,
//! actually writes a `trace_usage` row billed to the *flow's* user — and that a
//! refused call writes none at all. That is what this file covers.
//!
//! Every test is `#[serial]`: `GatewayConfig::from_env()` is read once when the
//! app boots, so the upstream stub's address has to be in the environment
//! before `TestServer::start()`.
//!
//!   cargo test -p nasiko-server --test token_attribution -- --test-threads=1

mod common;

use common::TestServer;
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "attribution-test-secret";

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@test.local"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

async fn seed_agent(server: &TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

/// The agent's `OPENAI_API_KEY` is a Nasiko-issued identity JWT, not a provider
/// key — it names the agent and its owner, and deliberately says nothing about
/// which user is chatting. That is the whole reason attribution needs the
/// traceparent.
fn agent_jwt(agent_id: Uuid, owner_id: Uuid) -> String {
    nasiko_llm_router::auth::mint_agent_token(
        &agent_id.to_string(),
        &owner_id.to_string(),
        JWT_SECRET,
        3600,
        jsonwebtoken::Algorithm::HS256,
    )
    .expect("mint agent token")
}

/// Point the router at a stub upstream and give it a platform key, before the
/// app reads its config. Returns the stub so the caller keeps it alive.
async fn stub_upstream() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"chatcmpl-1","object":"chat.completion","created":1,
                "model":"gpt-4o-mini",
                "choices":[{"index":0,"message":{"role":"assistant","content":"hi"},
                            "finish_reason":"stop"}],
                "usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}"#,
        )
        .expect_at_least(0)
        .create_async()
        .await;
    upstream
        .mock("POST", "/v1/embeddings")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"object":"list","model":"text-embedding-3-small",
                "data":[{"object":"embedding","index":0,"embedding":[0.1,0.2]}],
                "usage":{"prompt_tokens":5,"total_tokens":5}}"#,
        )
        .expect_at_least(0)
        .create_async()
        .await;

    // SAFETY: serialized by #[serial] within this test binary.
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", upstream.url());
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
    }
    upstream
}

fn chat_body() -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{"role": "user", "content": "hello"}]
    })
}

async fn post_llm(
    server: &TestServer,
    path: &str,
    bearer: &str,
    traceparent: Option<&str>,
    body: &serde_json::Value,
) -> reqwest::Response {
    let mut req = server
        .client
        .post(server.url(path))
        .bearer_auth(bearer)
        .json(body);
    if let Some(tp) = traceparent {
        req = req.header("traceparent", tp);
    }
    req.send().await.expect("llm request")
}

/// The usage row is written fire-and-forget (`usage::spawn_log`), so the
/// response can land before the insert commits. Poll rather than sleep once.
async fn wait_for_usage_row(
    server: &TestServer,
    agent_id: Uuid,
) -> Option<(Option<Uuid>, Option<String>, i32, serde_json::Value)> {
    for _ in 0..50 {
        let row: Option<(Option<Uuid>, Option<String>, i32, serde_json::Value)> = sqlx::query_as(
            "SELECT user_id, session_id,
                    (input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens)::int,
                    metadata
               FROM trace_usage WHERE agent_id = $1 AND priced_by = 'router'
              ORDER BY materialized_at DESC LIMIT 1",
        )
        .bind(agent_id)
        .fetch_optional(&server.db)
        .await
        .expect("query trace_usage");
        if row.is_some() {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    None
}

async fn usage_row_count(server: &TestServer, agent_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM trace_usage WHERE agent_id = $1 AND priced_by = 'router'",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .expect("count trace_usage")
}

// ─── The billing rule ────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn chat_bills_the_flow_user_not_the_agent_owner() {
    // The regression this whole change exists to prevent. The agent belongs to
    // one person and the conversation belongs to another; the spend must follow
    // the conversation. Before strict attribution this row was written against
    // the JWT's owner_id with a NULL session_id.
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;

    let agent_owner = seed_user(&server, "attr-agent-owner").await;
    let chatting_user = seed_user(&server, "attr-chatting-user").await;
    let agent = seed_agent(&server, agent_owner, "attr-agent").await;
    let (flow_id, traceparent) = common::open_flow(&server.db, chatting_user, agent).await;

    let res = post_llm(
        &server,
        "/v1/chat/completions",
        &agent_jwt(agent, agent_owner),
        Some(&traceparent),
        &chat_body(),
    )
    .await;
    assert_eq!(res.status(), 200, "attributed call must be served");

    let (user_id, session_id, total_tokens, metadata) = wait_for_usage_row(&server, agent)
        .await
        .expect("a usage row must be written for a served call");

    assert_eq!(
        user_id,
        Some(chatting_user),
        "spend must be billed to the flow's user, not the agent's owner ({agent_owner})"
    );
    assert_ne!(
        user_id,
        Some(agent_owner),
        "billing the agent owner is the exact bug this rule replaced"
    );
    assert_eq!(
        session_id.as_deref(),
        Some(flow_id.as_str()),
        "session_id must carry the flow id so per-conversation usage aggregates"
    );
    assert_eq!(total_tokens, 18, "token counts must come from the response");
    assert_eq!(
        metadata.get("attribution").and_then(|v| v.as_str()),
        Some("traceparent"),
        "the attribution source must be recorded for auditability"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn embeddings_bill_the_flow_user_too() {
    // Embeddings previously attempted no attribution at all, so every embedding
    // call was billed to the agent owner. It now follows the same rule as chat.
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;

    let agent_owner = seed_user(&server, "attr-emb-owner").await;
    let chatting_user = seed_user(&server, "attr-emb-user").await;
    let agent = seed_agent(&server, agent_owner, "attr-emb-agent").await;
    let (flow_id, traceparent) = common::open_flow(&server.db, chatting_user, agent).await;

    let body = serde_json::json!({"model": "text-embedding-3-small", "input": "hello"});
    let res = post_llm(
        &server,
        "/v1/embeddings",
        &agent_jwt(agent, agent_owner),
        Some(&traceparent),
        &body,
    )
    .await;
    assert_eq!(res.status(), 200);

    let (user_id, session_id, _, _) = wait_for_usage_row(&server, agent)
        .await
        .expect("embeddings must write a usage row");
    assert_eq!(user_id, Some(chatting_user));
    assert_eq!(session_id.as_deref(), Some(flow_id.as_str()));

    server.cleanup().await;
}

// ─── The refusals, and that they cost nothing ────────────────────────────────

#[tokio::test]
#[serial]
async fn chat_without_traceparent_is_refused_and_bills_nothing() {
    // "No flow, no tokens." The call must be refused *before* the upstream is
    // reached — an unattributed call would burn tokens outside every cascade
    // limit and land on the agent owner's bill.
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;

    let agent_owner = seed_user(&server, "attr-notp-owner").await;
    let agent = seed_agent(&server, agent_owner, "attr-notp-agent").await;

    let res = post_llm(
        &server,
        "/v1/chat/completions",
        &agent_jwt(agent, agent_owner),
        None,
        &chat_body(),
    )
    .await;
    assert_eq!(
        res.status(),
        403,
        "a call with no traceparent must be refused"
    );

    assert_eq!(
        usage_row_count(&server, agent).await,
        0,
        "a refused call must not be billed to anyone"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn chat_naming_another_agents_flow_is_refused() {
    // Without the participant check, agent B could quote agent A's traceparent
    // and spend against a user it was never dispatched for — draining that
    // user's token budget and misbilling them.
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;

    let owner = seed_user(&server, "attr-xflow-owner").await;
    let victim = seed_user(&server, "attr-xflow-victim").await;
    let insider = seed_agent(&server, owner, "attr-xflow-insider").await;
    let outsider = seed_agent(&server, owner, "attr-xflow-outsider").await;

    // The flow belongs to `insider`; `outsider` is not a participant.
    let (_, traceparent) = common::open_flow(&server.db, victim, insider).await;

    let res = post_llm(
        &server,
        "/v1/chat/completions",
        &agent_jwt(outsider, owner),
        Some(&traceparent),
        &chat_body(),
    )
    .await;
    assert_eq!(
        res.status(),
        403,
        "a non-participant naming someone else's flow must be refused"
    );
    assert_eq!(usage_row_count(&server, outsider).await, 0);
    assert_eq!(
        usage_row_count(&server, insider).await,
        0,
        "and nothing may be charged to the flow's real agent either"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn chat_on_a_completed_flow_is_refused() {
    // A finished conversation is not a billing target: reusing its trace id
    // after the fact would attribute spend to a user who has stopped talking.
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;

    let owner = seed_user(&server, "attr-done-owner").await;
    let user = seed_user(&server, "attr-done-user").await;
    let agent = seed_agent(&server, owner, "attr-done-agent").await;
    let (flow_id, traceparent) = common::open_flow(&server.db, user, agent).await;

    sqlx::query("UPDATE flows SET status = 'completed', completed_at = now() WHERE flow_id = $1")
        .bind(&flow_id)
        .execute(&server.db)
        .await
        .expect("complete the flow");

    let res = post_llm(
        &server,
        "/v1/chat/completions",
        &agent_jwt(agent, owner),
        Some(&traceparent),
        &chat_body(),
    )
    .await;
    assert_eq!(res.status(), 403);
    assert_eq!(usage_row_count(&server, agent).await, 0);

    server.cleanup().await;
}
