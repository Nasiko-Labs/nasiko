mod common;

use common::TestServer;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "llm-config-cache-test-secret";

struct Owner {
    id: Uuid,
    name: &'static str,
}

async fn stub_upstream() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"c1","object":"chat.completion","created":1,"model":"upstream",
                "choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
        )
        .expect_at_least(0)
        .create_async()
        .await;
    unsafe {
        std::env::set_var("OPENAI_API_BASE", upstream.url());
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
        std::env::set_var("LLM_CONFIG_CACHE_TTL", "600");
    }
    upstream
}

async fn seed(server: &TestServer, name: &'static str) -> (Owner, Uuid) {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(format!("{name}@local.test"))
            .fetch_one(&server.db)
            .await
            .unwrap();
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ('dummy-config-cache-agent', $1, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    (Owner { id, name }, agent)
}

async fn api(
    server: &TestServer,
    owner: &Owner,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Value {
    let mut rb = common::as_member(
        server.client.request(method.clone(), server.url(path)),
        &owner.id.to_string(),
        owner.name,
    );
    if let Some(body) = body {
        rb = rb.json(&body);
    }
    let resp = rb.send().await.unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert!(status.is_success(), "{method} {path} → {status}: {text}");
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

async fn create_config(server: &TestServer, owner: &Owner, name: &str, model: &str) -> Uuid {
    let created = api(
        server,
        owner,
        reqwest::Method::POST,
        "/api/llm-configs",
        Some(json!({ "name": name, "provider": "openai", "model": model })),
    )
    .await;
    created["data"]["id"].as_str().unwrap().parse().unwrap()
}

async fn answered_by(server: &TestServer, owner: &Owner, agent: Uuid, asked: &str) -> String {
    let (_, traceparent) = common::open_flow(&server.db, owner.id, agent).await;
    let jwt = nasiko_llm_router::auth::mint_agent_token(
        &agent.to_string(),
        &owner.id.to_string(),
        JWT_SECRET,
        3600,
        jsonwebtoken::Algorithm::HS256,
    )
    .unwrap();
    let resp = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(jwt)
        .header("traceparent", traceparent)
        .json(
            &json!({ "model": asked, "messages": [{ "role": "user", "content": "hello there" }] }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    body["model"].as_str().unwrap().to_string()
}

#[tokio::test]
#[serial]
async fn an_agents_own_config_change_applies_to_its_next_call() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed(&server, "dummy-config-cache-owner").await;
    let first = create_config(&server, &owner, "first", "model-one").await;
    let second = create_config(&server, &owner, "second", "model-two").await;
    let path = format!("/api/agents/{agent}/llm-config");

    api(
        &server,
        &owner,
        reqwest::Method::PATCH,
        &path,
        Some(json!({ "llm_config_id": first })),
    )
    .await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-1").await,
        "model-one"
    );

    api(
        &server,
        &owner,
        reqwest::Method::PATCH,
        &path,
        Some(json!({ "llm_config_id": second })),
    )
    .await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-2").await,
        "model-two"
    );

    api(
        &server,
        &owner,
        reqwest::Method::PATCH,
        &path,
        Some(json!({ "pinned_model": "model-pinned" })),
    )
    .await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-3").await,
        "model-pinned"
    );

    api(&server, &owner, reqwest::Method::DELETE, &path, None).await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-4").await,
        "asked-4"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_shared_config_change_applies_to_the_next_call() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed(&server, "dummy-config-cache-sharer").await;
    let shared = create_config(&server, &owner, "shared", "model-one").await;
    let config_path = format!("/api/llm-configs/{shared}");
    let default_path = format!("{config_path}/default");

    assert_eq!(
        answered_by(&server, &owner, agent, "asked-1").await,
        "asked-1"
    );

    api(&server, &owner, reqwest::Method::POST, &default_path, None).await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-2").await,
        "model-one"
    );

    api(
        &server,
        &owner,
        reqwest::Method::PATCH,
        &config_path,
        Some(json!({ "model": "model-two" })),
    )
    .await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-3").await,
        "model-two"
    );

    api(
        &server,
        &owner,
        reqwest::Method::DELETE,
        &default_path,
        None,
    )
    .await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-4").await,
        "asked-4"
    );

    api(&server, &owner, reqwest::Method::POST, &default_path, None).await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-5").await,
        "model-two"
    );

    api(&server, &owner, reqwest::Method::DELETE, &config_path, None).await;
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-6").await,
        "asked-6"
    );

    let created_default = api(
        &server,
        &owner,
        reqwest::Method::POST,
        "/api/llm-configs",
        Some(json!({ "name": "fresh-default", "provider": "openai", "model": "model-three", "is_default": true })),
    )
    .await;
    assert!(created_default["data"]["id"].is_string());
    assert_eq!(
        answered_by(&server, &owner, agent, "asked-7").await,
        "model-three"
    );

    server.cleanup().await;
}
