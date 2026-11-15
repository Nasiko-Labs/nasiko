//! What each call through the mounted LLM router did with compact tool schemas reaches its flow's
//! `trace_usage.metadata.compact_tools` block: applied, skipped and why, or retried after a reply
//! that did not decode, with the tool-definition bytes sent against what native would have sent.
//! Against a stub provider and a real database, one test per path the chat handler takes.
//!
//! Every test is `#[serial]`: `GatewayConfig::from_env()` is read once when the app boots, so the
//! stub's address and `TOKEN_COMPACT_TOOLS` have to be in the environment before
//! `TestServer::start()`.
//!
//!   cargo test -p nasiko-server --test compact_tools_usage -- --test-threads=1

mod common;

use std::sync::{Arc, Mutex};

use common::TestServer;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "compact-tools-usage-test-secret";

/// The compact call a model answers with when it follows the call-format instruction.
const COMPACT_CALL: &str = r#"<<call get_weather {"city":"Pune"}>>"#;

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

/// Point OpenAI at `base` and set the compact-tools flag before the app reads its config.
fn configure(base: &str, compact_tools: bool) {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", base);
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
        std::env::set_var(
            "TOKEN_COMPACT_TOOLS",
            if compact_tools { "true" } else { "false" },
        );
    }
}

/// A provider that answers the chat calls it receives with `replies`, in order (the last repeats).
async fn provider(replies: Vec<Value>) -> mockito::ServerGuard {
    let seen = Arc::new(Mutex::new(0usize));
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |_| {
            let mut n = seen.lock().unwrap_or_else(|e| e.into_inner());
            let reply = &replies[(*n).min(replies.len() - 1)];
            *n += 1;
            json!({
                "id": "chatcmpl-c", "object": "chat.completion", "model": "gpt-4o-mini",
                "choices": [{ "index": 0, "message": reply, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 50, "completion_tokens": 5, "total_tokens": 55 }
            })
            .to_string()
            .into_bytes()
        })
        .create_async()
        .await;
    upstream
}

/// A provider that streams one short reply.
async fn streaming_provider() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    let chunk = json!({
        "id": "c1", "object": "chat.completion.chunk", "created": 1, "model": "gpt-4o-mini",
        "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "Sunny." },
                      "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 5, "total_tokens": 55 }
    });
    upstream
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        .create_async()
        .await;
    upstream
}

fn weather_request(stream: bool) -> Value {
    json!({
        "model": "gpt-4o-mini",
        "stream": stream,
        "messages": [{ "role": "user", "content": "Weather in Pune?" }],
        "tools": [{ "type": "function", "function": {
            "name": "get_weather",
            "description": "Weather for a city.",
            "parameters": {
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }
        }}]
    })
}

async fn post(server: &TestServer, caller: &Caller, body: Value) -> reqwest::Response {
    server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(&caller.jwt)
        .header("traceparent", &caller.traceparent)
        .json(&body)
        .send()
        .await
        .expect("llm request")
}

/// The flow's `compact_tools` block once `calls` calls are in. The write is fire-and-forget
/// (`usage::spawn_log`), so poll for it.
async fn compact_tools_block(server: &TestServer, agent: Uuid, calls: i64) -> Value {
    let mut last = Value::Null;
    for _ in 0..50 {
        let block: Option<Value> = sqlx::query_scalar(
            "SELECT metadata->'compact_tools' FROM trace_usage WHERE agent_id = $1",
        )
        .bind(agent)
        .fetch_optional(&server.db)
        .await
        .expect("query trace_usage")
        .flatten();
        if let Some(block) = block {
            if block["calls"] == calls {
                return block;
            }
            last = block;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("no compact_tools block with {calls} calls for agent {agent}; last seen: {last}");
}

fn bytes(block: &Value, key: &str) -> i64 {
    block[key]
        .as_i64()
        .unwrap_or_else(|| panic!("{key} in {block}"))
}

#[tokio::test]
#[serial]
async fn a_compacted_call_whose_reply_decodes_is_counted_as_applied() {
    let upstream = provider(vec![
        json!({ "role": "assistant", "content": COMPACT_CALL }),
    ])
    .await;
    configure(&upstream.url(), true);
    let server = TestServer::start().await;
    let caller = seed(&server, "compact-applied").await;

    let res = post(&server, &caller, weather_request(false)).await;
    assert_eq!(res.status(), 200);

    let block = compact_tools_block(&server, caller.agent, 1).await;
    assert_eq!(block["applied"], 1, "{block}");
    assert!(
        bytes(&block, "tool_bytes_sent") < bytes(&block, "tool_bytes_native"),
        "signatures are smaller than the schema they replace: {block}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_reply_that_does_not_decode_is_counted_as_retried_then_resent_natively() {
    let undecodable = json!({
        "role": "assistant",
        "content": r#"<<call get_weather {"town":"Pune"}>>"#
    });
    let native = json!({
        "role": "assistant", "content": null,
        "tool_calls": [{ "id": "call_native", "type": "function",
                         "function": { "name": "get_weather", "arguments": "{\"city\":\"Pune\"}" } }]
    });
    let upstream = provider(vec![undecodable, native]).await;
    configure(&upstream.url(), true);
    let server = TestServer::start().await;
    let caller = seed(&server, "compact-retried").await;

    let res = post(&server, &caller, weather_request(false)).await;
    assert_eq!(res.status(), 200);

    // Two provider calls in one flow: the compact one that failed to decode, and the native re-send.
    let block = compact_tools_block(&server, caller.agent, 2).await;
    assert_eq!(block["retried"], 1, "{block}");
    assert_eq!(block["native_resend"], 1, "{block}");
    assert!(block.get("applied").is_none(), "{block}");
    let native_bytes = bytes(&block, "tool_bytes_native") / 2;
    assert!(
        bytes(&block, "tool_bytes_sent") < 2 * native_bytes,
        "one compact call and one native: {block}"
    );
    assert!(
        bytes(&block, "tool_bytes_sent") > native_bytes,
        "the native re-send carried the full schemas: {block}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn with_compact_tools_off_a_call_is_counted_as_skipped_disabled() {
    let upstream = provider(vec![json!({ "role": "assistant", "content": "Sunny." })]).await;
    configure(&upstream.url(), false);
    let server = TestServer::start().await;
    let caller = seed(&server, "compact-off").await;

    let res = post(&server, &caller, weather_request(false)).await;
    assert_eq!(res.status(), 200);

    let block = compact_tools_block(&server, caller.agent, 1).await;
    assert_eq!(block["skipped_disabled"], 1, "{block}");
    assert!(bytes(&block, "tool_bytes_native") > 0, "{block}");
    assert_eq!(
        block["tool_bytes_sent"], block["tool_bytes_native"],
        "the full schemas went out: {block}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_streamed_call_is_counted_as_skipped_streaming() {
    let upstream = streaming_provider().await;
    configure(&upstream.url(), true);
    let server = TestServer::start().await;
    let caller = seed(&server, "compact-stream").await;

    let res = post(&server, &caller, weather_request(true)).await;
    assert_eq!(res.status(), 200);
    // The usage row is written when the stream ends.
    let _ = res.text().await;

    let block = compact_tools_block(&server, caller.agent, 1).await;
    assert_eq!(block["skipped_streaming"], 1, "{block}");
    assert_eq!(
        block["tool_bytes_sent"], block["tool_bytes_native"],
        "{block}"
    );
    server.cleanup().await;
}
