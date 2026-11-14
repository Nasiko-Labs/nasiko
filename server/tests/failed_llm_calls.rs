//! NAS-1018: a provider call that fails is still a call. Through the mounted LLM router, against
//! a stub provider and a real database, every failure path the router serves writes its
//! `trace_usage` row: `error_count` goes up, the unreported prompt is estimated apart from the
//! spend columns, and no cost is invented.
//!
//! Every test is `#[serial]`: `GatewayConfig::from_env()` is read once when the app boots, so
//! the stub's address has to be in the environment before `TestServer::start()`.
//!
//!   cargo test -p nasiko-server --test failed_llm_calls -- --test-threads=1

mod common;

use common::TestServer;
use serial_test::serial;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

const JWT_SECRET: &str = "failed-calls-test-secret";

/// An agent in a live flow, and what it calls the router with.
struct Caller {
    agent: Uuid,
    jwt: String,
    traceparent: String,
}

async fn seed(server: &TestServer, name: &str) -> Caller {
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("{name}-{}", Uuid::new_v4()))
            .bind(format!("{name}-{}@test.local", Uuid::new_v4()))
            .fetch_one(&server.db)
            .await
            .expect("seed user");
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(format!("{name}-agent-{}", Uuid::new_v4()))
    .bind(user)
    .fetch_one(&server.db)
    .await
    .expect("seed agent");
    let (_, traceparent) = common::open_flow(&server.db, user, agent).await;
    let jwt = nasiko_llm_router::auth::mint_agent_token(
        &agent.to_string(),
        &user.to_string(),
        JWT_SECRET,
        3600,
        jsonwebtoken::Algorithm::HS256,
    )
    .expect("mint agent token");
    Caller {
        agent,
        jwt,
        traceparent,
    }
}

/// Point both built-in spokes at `base` before the app reads its config.
fn route_providers_to(base: &str) {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", base);
        std::env::set_var("ANTHROPIC_BASE_URL", base);
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
        std::env::set_var("PLATFORM_ANTHROPIC_API_KEY", "sk-ant-platform-test");
    }
}

/// A provider that answers every call to `path` with `status`.
async fn failing_provider(path: &str, status: usize) -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", path)
        .with_status(status)
        .with_body(r#"{"error":{"message":"upstream unavailable"}}"#)
        .expect_at_least(1)
        .create_async()
        .await;
    route_providers_to(&upstream.url());
    upstream
}

async fn post(
    server: &TestServer,
    path: &str,
    caller: &Caller,
    body: serde_json::Value,
) -> reqwest::Response {
    server
        .client
        .post(server.url(path))
        .bearer_auth(&caller.jwt)
        .header("traceparent", &caller.traceparent)
        .json(&body)
        .send()
        .await
        .expect("llm request")
}

#[derive(sqlx::FromRow, Debug)]
struct Booked {
    error_count: Option<i64>,
    router_error_count: i64,
    router_estimated_input_tokens: i64,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: f64,
    cost_estimated: Option<bool>,
    failure: Option<serde_json::Value>,
}

/// The write is fire-and-forget (`usage::spawn_log`), so poll for it.
async fn wait_for_failure(server: &TestServer, agent: Uuid) -> Booked {
    for _ in 0..50 {
        let row: Option<Booked> = sqlx::query_as(
            "SELECT error_count::int8 AS error_count, router_error_count::int8 AS router_error_count,
                    router_estimated_input_tokens, input_tokens::int8 AS input_tokens,
                    output_tokens::int8 AS output_tokens, COALESCE(cost_usd, 0)::float8 AS cost_usd,
                    cost_estimated, metadata->'failure' AS failure
               FROM trace_usage WHERE agent_id = $1 AND router_error_count > 0",
        )
        .bind(agent)
        .fetch_optional(&server.db)
        .await
        .expect("query trace_usage");
        if let Some(row) = row {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("no failed call was booked for agent {agent}");
}

/// A failed call with no reported usage: counted, prompt estimated, nothing priced.
fn assert_estimated_failure(booked: &Booked, class: &str, status: Option<u16>) {
    assert_eq!(booked.error_count, Some(1), "{booked:?}");
    assert_eq!(booked.router_error_count, 1, "{booked:?}");
    assert!(booked.router_estimated_input_tokens > 0, "{booked:?}");
    assert_eq!(
        (booked.input_tokens, booked.output_tokens),
        (0, 0),
        "{booked:?}"
    );
    assert_eq!(
        booked.cost_usd, 0.0,
        "no usage reported, so no cost: {booked:?}"
    );
    assert_eq!(booked.cost_estimated, Some(true), "{booked:?}");
    let failure = booked.failure.as_ref().expect("metadata.failure");
    assert_eq!(failure["class"], class, "{failure}");
    assert_eq!(
        failure["http_status"].as_u64(),
        status.map(u64::from),
        "{failure}"
    );
    assert_eq!(failure["tokens_estimated"], true, "{failure}");
}

fn chat_body(stream: bool) -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-4o-mini",
        "stream": stream,
        "messages": [{"role": "user", "content": "summarize the repository layout in detail"}]
    })
}

#[tokio::test]
#[serial]
async fn a_chat_call_the_provider_fails_with_503_is_booked() {
    let _upstream = failing_provider("/v1/chat/completions", 503).await;
    let server = TestServer::start().await;
    let caller = seed(&server, "failed-chat").await;

    let res = post(&server, "/v1/chat/completions", &caller, chat_body(false)).await;
    assert_eq!(res.status(), 502);

    assert_estimated_failure(
        &wait_for_failure(&server, caller.agent).await,
        "status",
        Some(503),
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_stream_the_provider_refuses_with_429_is_booked() {
    let _upstream = failing_provider("/v1/chat/completions", 429).await;
    let server = TestServer::start().await;
    let caller = seed(&server, "failed-stream").await;

    let res = post(&server, "/v1/chat/completions", &caller, chat_body(true)).await;
    // The router passes a provider 429 through so clients can back off and retry.
    assert_eq!(res.status(), 429);

    assert_estimated_failure(
        &wait_for_failure(&server, caller.agent).await,
        "status",
        Some(429),
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_anthropic_messages_call_that_fails_is_booked() {
    let _upstream = failing_provider("/messages", 500).await;
    let server = TestServer::start().await;
    let caller = seed(&server, "failed-messages").await;

    let body = serde_json::json!({
        // The surface sets only the wire format; the prefix picks the provider.
        "model": "anthropic/claude-sonnet-4-5",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "hello there"}]
    });
    let res = post(&server, "/v1/messages", &caller, body).await;
    assert!(res.status().is_server_error(), "{}", res.status());

    assert_estimated_failure(
        &wait_for_failure(&server, caller.agent).await,
        "status",
        Some(500),
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_responses_call_that_fails_is_booked() {
    let _upstream = failing_provider("/v1/responses", 503).await;
    let server = TestServer::start().await;
    let caller = seed(&server, "failed-responses").await;

    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "input": [{"role": "user", "content": "hello there"}]
    });
    let res = post(&server, "/v1/responses", &caller, body).await;
    assert_eq!(res.status(), 503, "the provider's error is passed through");

    assert_estimated_failure(
        &wait_for_failure(&server, caller.agent).await,
        "status",
        Some(503),
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_embeddings_call_that_fails_is_booked() {
    let _upstream = failing_provider("/v1/embeddings", 500).await;
    let server = TestServer::start().await;
    let caller = seed(&server, "failed-embeddings").await;

    let body = serde_json::json!({"model": "text-embedding-3-small", "input": "hello there"});
    let res = post(&server, "/v1/embeddings", &caller, body).await;
    assert_eq!(res.status(), 502);

    assert_estimated_failure(
        &wait_for_failure(&server, caller.agent).await,
        "status",
        Some(500),
    );
    server.cleanup().await;
}

/// A provider that starts a stream, reports usage in its first chunk, then drops the
/// connection mid-body. Not mockito: it cannot break a response off half-way.
async fn provider_that_breaks_off_mid_stream() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                read_request(&mut socket).await;
                let event = r#"data: {"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"role":"assistant","content":"partial"}}],"usage":{"prompt_tokens":321,"completion_tokens":4,"total_tokens":325}}"#;
                let chunk = format!("{event}\n\n");
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                            transfer-encoding: chunked\r\n\r\n";
                let body = format!("{:x}\r\n{chunk}\r\n", chunk.len());
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.flush().await;
                // Dropped without the terminating zero-length chunk: the body breaks off.
            });
        }
    });
    base
}

/// Read one request (headers plus `content-length` body) so the reply is not a reset.
async fn read_request(socket: &mut tokio::net::TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let Ok(n) = socket.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                return;
            }
        }
    }
}

#[tokio::test]
#[serial]
async fn a_stream_that_breaks_off_is_booked_with_the_usage_it_reported() {
    let base = provider_that_breaks_off_mid_stream().await;
    route_providers_to(&base);
    let server = TestServer::start().await;
    let caller = seed(&server, "broken-stream").await;

    let res = post(&server, "/v1/chat/completions", &caller, chat_body(true)).await;
    assert_eq!(res.status(), 200, "the stream had started");
    let _ = res.text().await;

    let booked = wait_for_failure(&server, caller.agent).await;
    assert_eq!(booked.error_count, Some(1), "{booked:?}");
    assert_eq!(
        (booked.input_tokens, booked.output_tokens),
        (321, 4),
        "the usage the provider reported is billed: {booked:?}"
    );
    assert_eq!(
        booked.router_estimated_input_tokens, 0,
        "nothing to estimate"
    );
    assert!(
        booked.cost_usd > 0.0,
        "reported usage is priced: {booked:?}"
    );
    let failure = booked.failure.expect("metadata.failure");
    assert_eq!(failure["class"], "stream");
    assert_eq!(failure["tokens_estimated"], false);
    server.cleanup().await;
}
