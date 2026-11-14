mod common;

use common::TestServer;
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "body-limit-test-secret";
const MIB: usize = 1024 * 1024;

async fn seed_agent(server: &TestServer) -> (Uuid, Uuid) {
    let owner: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email) VALUES ('dummy-body-limit-owner', 'dummy-body-limit-owner@local.test') RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ('dummy-body-limit-agent', $1, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap();
    (owner, agent)
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
        .mock("POST", "/v1/chat/completions")
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
    unsafe {
        std::env::set_var("OPENAI_API_BASE", upstream.url());
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
    }
    upstream
}

fn chat_body_of(total_bytes: usize) -> Vec<u8> {
    let head = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":""#;
    let tail = r#""}]}"#;
    let padding = total_bytes - head.len() - tail.len();
    let mut body = Vec::with_capacity(total_bytes);
    body.extend_from_slice(head.as_bytes());
    body.resize(head.len() + padding, b'a');
    body.extend_from_slice(tail.as_bytes());
    body
}

async fn post(
    server: &TestServer,
    path: &str,
    bearer: &str,
    traceparent: &str,
    body: Vec<u8>,
    encoding: Option<&str>,
) -> reqwest::Response {
    let mut request = server
        .client
        .post(server.url(path))
        .bearer_auth(bearer)
        .header("traceparent", traceparent)
        .header("content-type", "application/json");
    if let Some(encoding) = encoding {
        request = request.header("content-encoding", encoding);
    }
    request.body(body).send().await.unwrap()
}

#[tokio::test]
#[serial]
async fn a_request_above_the_old_2_mib_cap_from_a_known_agent_is_served() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed_agent(&server).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;

    let response = post(
        &server,
        "/v1/chat/completions",
        &token(agent, owner),
        &traceparent,
        chat_body_of(2 * MIB + 100 * 1024),
        None,
    )
    .await;

    assert_eq!(response.status().as_u16(), 200);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_request_over_48_mib_is_refused_with_the_limit_named() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed_agent(&server).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;

    let response = post(
        &server,
        "/v1/chat/completions",
        &token(agent, owner),
        &traceparent,
        chat_body_of(48 * MIB + 1),
        None,
    )
    .await;

    assert_eq!(response.status().as_u16(), 413);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["detail"],
        "request body is over the router's 48 MiB limit"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_responses_route_names_the_limit_in_its_own_error_shape() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed_agent(&server).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;

    let response = post(
        &server,
        "/v1/responses",
        &token(agent, owner),
        &traceparent,
        chat_body_of(48 * MIB + 1),
        None,
    )
    .await;

    assert_eq!(response.status().as_u16(), 413);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["error"]["message"],
        "request body is over the router's 48 MiB limit"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_body_that_is_not_json_keeps_its_original_refusal() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed_agent(&server).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;
    let bearer = token(agent, owner);

    let not_json = post(
        &server,
        "/v1/chat/completions",
        &bearer,
        &traceparent,
        b"{not json".to_vec(),
        None,
    )
    .await;
    assert_eq!(not_json.status().as_u16(), 400);

    let no_content_type = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(&bearer)
        .header("traceparent", &traceparent)
        .body(r#"{"model":"gpt-4o-mini","messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(no_content_type.status().as_u16(), 415);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_unknown_caller_is_refused_before_its_body_is_read() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed_agent(&server).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;

    for path in ["/v1/chat/completions", "/v1/messages", "/v1/responses"] {
        let response = post(
            &server,
            path,
            "not-a-valid-token",
            &traceparent,
            chat_body_of(40 * MIB),
            None,
        )
        .await;
        assert_eq!(response.status().as_u16(), 401, "{path}");
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_compressed_body_is_measured_after_it_is_unpacked() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed_agent(&server).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;
    let packed = zstd::encode_all(&chat_body_of(49 * MIB)[..], 3).unwrap();
    assert!(packed.len() < MIB);

    let response = post(
        &server,
        "/v1/chat/completions",
        &token(agent, owner),
        &traceparent,
        packed,
        Some("zstd"),
    )
    .await;

    assert_eq!(response.status().as_u16(), 413);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn embeddings_keep_the_2_mib_cap() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let (owner, agent) = seed_agent(&server).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;
    let head = r#"{"model":"text-embedding-3-small","input":""#;
    let mut body = head.as_bytes().to_vec();
    body.resize(head.len() + 2 * MIB + 1024, b'a');
    body.extend_from_slice(br#""}"#);

    let response = post(
        &server,
        "/v1/embeddings",
        &token(agent, owner),
        &traceparent,
        body,
        None,
    )
    .await;

    assert_eq!(response.status().as_u16(), 413);
    server.cleanup().await;
}
