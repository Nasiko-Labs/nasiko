//! Full-stack live E2E (infra + Docker gated, `#[ignore]`): POST a chat turn to a
//! coding agent through the real `/api/agents/{id}` proxy and confirm the whole
//! server path runs.
//!
//! Because `run_turn` performs lease → deploy → attach → ACP connect →
//! `session/new` → prompt-dispatch *before* it hands back the event stream, a
//! `200 text/event-stream` response is itself proof that the entire pipeline ran:
//!   - the proxy took the coding-agent branch (NAS-840),
//!   - the one-turn lease was acquired on real Postgres (NAS-838),
//!   - the real opencode image was deployed, attached, and handshaken
//!     (NAS-837 + bridge + NAS-836),
//!   - and the SSE translation produced the stream.
//!
//! A real model answer isn't produced (opencode needs `opencode auth login`), so
//! the streamed turn ends in a terminal `failed` status — which still exercises
//! the full server path.
//!
//!   just infra
//!   cargo test -p nasiko-server --test coding_agent_proxy_e2e -- \
//!       --ignored --nocapture --test-threads=1

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{TestServer, as_member};
use nasiko_runtime::{DockerRuntime, DockerRuntimeConfig};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires `just infra` + Docker + the opencode:0.1.0 image"]
async fn coding_agent_proxy_streams_sse() {
    let docker = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("Docker must be running");
    let server = TestServer::start_with_runtime(|_| {}, Arc::new(docker)).await;

    // A user who owns the coding agent.
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind("coder")
            .bind("coder@test.local")
            .fetch_one(&server.db)
            .await
            .unwrap();

    // Register an ACP coding agent backed by the real opencode image.
    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, workload_kind) \
         VALUES ('opencoder', $1, 'opencode:0.1.0', 'running', 'acp_coding_agent') RETURNING id",
    )
    .bind(uid)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Send a chat turn through the real proxy.
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": "message/stream",
        "params": { "message": {
            "role": "user",
            "messageId": "m1",
            "parts": [{ "text": "say hello" }]
        }}
    });
    let client = reqwest::Client::new();
    let url = server.url(&format!("/api/agents/{agent_id}"));
    let resp = as_member(client.post(&url).json(&body), &uid.to_string(), "coder")
        .send()
        .await
        .expect("request to coding-agent proxy");

    // HARD proof: a 200 SSE means run_turn got all the way through
    // lease → deploy → attach → connect → session/new → prompt before streaming.
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "expected 200 SSE from the coding-agent branch"
    );
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert!(
        content_type.contains("text/event-stream"),
        "expected an SSE stream, got content-type {content_type:?}"
    );

    // Drain the stream and confirm the translation emitted A2A status-update
    // events ending in a terminal state. (Bounded so a stuck agent can't hang the
    // test — the header assertion above is the real proof.) The wire tokens are
    // the `nasiko_types::a2a` serialization: the `statusUpdate` stream variant and
    // the `TASK_STATE_*` states, exactly what an ordinary agent's stream carries.
    let text = tokio::time::timeout(Duration::from_secs(90), resp.text())
        .await
        .expect("SSE stream should complete within 90s")
        .expect("read SSE body");
    assert!(
        text.contains("statusUpdate") && text.contains("TASK_STATE_WORKING"),
        "expected A2A working status-update events, got: {text}"
    );
    assert!(
        text.contains("TASK_STATE_COMPLETED") || text.contains("TASK_STATE_FAILED"),
        "expected a terminal status, got: {text}"
    );

    // Clean up the container the turn deployed (not --rm'd by the runtime).
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &format!("nasiko-agent-{agent_id}")])
        .output();
    server.cleanup().await;
}
