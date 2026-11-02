//! NAS-1137: an agent's LLM token names the owner at deploy time, and the
//! router used that claim to pick whose provider keys pay. After an ownership
//! transfer, and before a redeploy, the agent kept spending the previous
//! owner's keys. A token whose owner claim no longer matches the agent row is
//! now refused (in either direction nobody's keys are spent by the wrong
//! party); a redeploy mints a fresh one.
//!
//! `#[serial]`: the router reads its config from the environment at boot.
//!   cargo test -p nasiko-server --test llm_owner_transfer -- --test-threads=1
mod common;

use common::TestServer;
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "owner-transfer-test-secret";

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@local.test"))
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn seed_agent(server: &TestServer, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ('dummy-transfer-agent', $1, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

fn token(agent: Uuid, owner: Uuid) -> String {
    nasiko_llm_router::auth::mint_agent_token(
        &agent.to_string(),
        &owner.to_string(),
        JWT_SECRET,
        3600,
        jsonwebtoken::Algorithm::HS256,
    )
    .unwrap()
}

async fn stub_upstream() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"c1","object":"chat.completion","created":1,"model":"gpt-4o-mini",
                "choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
        )
        .expect_at_least(0)
        .create_async()
        .await;
    upstream
        .mock("POST", "/embeddings")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"object":"list","model":"text-embedding-3-small",
                "data":[{"object":"embedding","index":0,"embedding":[0.1]}],
                "usage":{"prompt_tokens":1,"total_tokens":1}}"#,
        )
        .expect_at_least(0)
        .create_async()
        .await;
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe {
        std::env::set_var("OPENAI_API_BASE", upstream.url());
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
    }
    upstream
}

async fn call(server: &TestServer, path: &str, bearer: &str, traceparent: &str) -> u16 {
    let body = if path.ends_with("embeddings") {
        serde_json::json!({"model": "text-embedding-3-small", "input": "hi"})
    } else {
        serde_json::json!({"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hi"}]})
    };
    server
        .client
        .post(server.url(path))
        .bearer_auth(bearer)
        .header("traceparent", traceparent)
        .json(&body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Usage rows are written fire-and-forget: wait for `want`, then a little longer
/// so a stray extra row would also land.
async fn usage_rows_settled(server: &TestServer, agent: Uuid, want: i64) -> i64 {
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM token_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap()
    };
    for _ in 0..50 {
        if count().await >= want {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    count().await
}

#[tokio::test]
#[serial]
async fn a_token_from_before_a_transfer_is_refused() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let previous = seed_user(&server, "dummy-previous-owner").await;
    let current = seed_user(&server, "dummy-current-owner").await;
    let agent = seed_agent(&server, previous).await;
    let (_, traceparent) = common::open_flow(&server.db, current, agent).await;
    let stale = token(agent, previous);

    // Served while the claim is true.
    assert_eq!(
        call(&server, "/v1/chat/completions", &stale, &traceparent).await,
        200
    );

    assert_eq!(usage_rows_settled(&server, agent, 1).await, 1);

    sqlx::query("UPDATE agents SET owner_id = $2 WHERE id = $1")
        .bind(agent)
        .bind(current)
        .execute(&server.db)
        .await
        .unwrap();

    for path in ["/v1/chat/completions", "/v1/embeddings"] {
        assert_eq!(
            call(&server, path, &stale, &traceparent).await,
            403,
            "{path}"
        );
    }
    // A token minted for the current owner (what a redeploy does) works.
    let fresh = token(agent, current);
    assert_eq!(
        call(&server, "/v1/chat/completions", &fresh, &traceparent).await,
        200
    );

    // Two served calls, two usage rows: the refused ones billed nothing.
    assert_eq!(usage_rows_settled(&server, agent, 2).await, 2);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deleted_agents_token_is_refused() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let owner = seed_user(&server, "dummy-owner").await;
    let agent = seed_agent(&server, owner).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;
    let jwt = token(agent, owner);

    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();

    assert_eq!(
        call(&server, "/v1/chat/completions", &jwt, &traceparent).await,
        403
    );

    server.cleanup().await;
}
