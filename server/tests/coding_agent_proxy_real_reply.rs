//! Full-stack live E2E: a real chat turn to `/api/agents/{id}` deploys a real
//! OpenCode container, drives one ACP turn, and streams a real completion back.
//! The server is made reachable from inside the container
//! (`start_reachable_with_runtime` binds 0.0.0.0 and publishes
//! `LLM_GATEWAY_BASE_URL`) and the session-token wiring is configured, so a routed
//! model call can reach us. Run it with infra up and a real key:
//! `OPENAI_API_KEY=sk-... cargo test -p nasiko-server --test
//! coding_agent_proxy_real_reply -- --ignored --nocapture --test-threads=1`.
//!
//! MODEL ROUTING (closed in the image): driven over ACP (`opencode acp`) the
//! a2a HTTP wrapper that forces the routed model per prompt is bypassed, so an
//! OpenCode image with no baked default would fall back to its built-in free
//! model (`opencode/big-pickle`), which needs no key and routes to OpenCode's own
//! backend rather than Nasiko. A proper OpenCode ACP image closes this itself by
//! baking `model = "openai-chat/gpt-4o"` into the `opencode.json` it reads at
//! `$HOME/.config/opencode/` — the proxy only injects the router base URL + the
//! session token (and, when HITL is on, an `OPENCODE_CONFIG_CONTENT` permission
//! flip, which merges over that baked config). So this test needs an ACP image
//! that bakes the routed model; point `OPENCODE_IMAGE` at one. Under ACP the
//! model call then flows through the router and a `token_usage` row is written,
//! which this test asserts. The session-token to router to billing path itself
//! is also proven in isolation by `coding_session_router_live.rs`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{TestServer, as_member};
use nasiko_runtime::{DockerRuntime, DockerRuntimeConfig};
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "coding-proxy-live-secret";

#[tokio::test]
#[serial]
#[ignore = "requires just infra + Docker + opencode:0.1.0 + a real OPENAI_API_KEY"]
async fn coding_agent_turn_streams_a_real_completion() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();
    let key = std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .expect("set OPENAI_API_KEY to a real key to run this test");
    // SAFETY: #[serial]; set before the app reads its config at boot.
    // The gateway secret must match `config.agent_jwt_secret` (mint side) below.
    unsafe {
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", &key);
        std::env::remove_var("OPENAI_API_BASE");
    }

    let docker = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("Docker must be running");
    let server = TestServer::start_reachable_with_runtime(
        |c| c.agent_jwt_secret = JWT_SECRET.to_owned(),
        Arc::new(docker),
    )
    .await;

    // A user who owns the coding agent — the session token's billing subject.
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind("live-coder")
            .bind("live-coder@test.local")
            .fetch_one(&server.db)
            .await
            .unwrap();

    // Point OPENCODE_IMAGE at a proper OpenCode ACP image — one that bakes the
    // routed `openai-chat/gpt-4o` provider as its default model in the
    // `opencode.json` it reads under `$HOME/.config/opencode/`. An image that
    // defaults to OpenCode's built-in model bypasses the router and this test's
    // `token_usage` assertion fails.
    let image = std::env::var("OPENCODE_IMAGE").unwrap_or_else(|_| "opencode:0.1.0".to_owned());
    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, workload_kind) \
         VALUES ('live-opencoder', $1, $2, 'running', 'acp_coding_agent') RETURNING id",
    )
    .bind(uid)
    .bind(&image)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "message/stream",
        "params": { "message": {
            "role": "user",
            "messageId": "m1",
            "parts": [{ "text": "Reply with only the single digit 4 and nothing else." }]
        }}
    });
    let url = server.url(&format!("/api/agents/{agent_id}"));
    let resp = as_member(
        server.client.post(&url).json(&body),
        &uid.to_string(),
        "live-coder",
    )
    .send()
    .await
    .expect("request to coding-agent proxy");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "expected 200 SSE from the coding-agent branch"
    );

    // A real model call takes longer than the no-LLM handshake path; give the
    // whole routed round-trip (container -> router -> OpenAI -> back) room.
    let text = tokio::time::timeout(Duration::from_secs(180), resp.text())
        .await
        .expect("SSE stream should complete within 180s")
        .expect("read SSE body");

    eprintln!("=== SSE BEGIN ===\n{text}\n=== SSE END ===");

    // Full plumbing proof: the turn completed with a real agent reply streamed
    // back as A2A status updates (deploy → attach → ACP → prompt → model → SSE).
    assert!(
        text.contains("TASK_STATE_COMPLETED") && !text.contains("TASK_STATE_FAILED"),
        "the turn must complete, not fail; got: {text}"
    );
    assert!(
        text.contains("\"role\":\"ROLE_AGENT\"") && text.contains("\"text\""),
        "expected a real agent reply in the stream; got: {text}"
    );

    // The model call must route through Nasiko (session token → router → billing).
    // A `token_usage` row billed to the turn's user is the proof; the proxy forces
    // the routed model so OpenCode under ACP no longer falls back to its free
    // built-in model (see the module docs).
    let billed: Option<Uuid> = {
        let mut found = None;
        for _ in 0..30 {
            found = sqlx::query_scalar(
                "SELECT user_id FROM token_usage WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(agent_id)
            .fetch_optional(&server.db)
            .await
            .expect("query token_usage");
            if found.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        found.flatten()
    };
    eprintln!("billed subject: {billed:?} (expected {uid})");
    assert_eq!(
        billed,
        Some(uid),
        "the model call must route through Nasiko and bill the turn's user: a \
         token_usage row for {uid} is required (got {billed:?})",
    );

    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &format!("nasiko-agent-{agent_id}")])
        .output();
    server.cleanup().await;
}
