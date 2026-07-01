mod common;

use serde_json::{Value, json};
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

// ─── helpers ────────────────────────────────────────────────────────────────

const SUPERUSER_ID: &str = "35601cc7-d0c4-4db8-9ec6-f5b305494c56";

fn as_superuser(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.header("x-user-id", SUPERUSER_ID)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
}

/// Build a JSON-RPC `message/stream` body with no metadata — no agent_id means
/// the request goes through the routing engine.
fn stream_body(text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "message/stream",
        "id": Uuid::new_v4().to_string(),
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{ "text": text }]
            }
        }
    })
}

/// Build a JSON-RPC body targeting a specific agent_id (direct or orchestrator path).
fn stream_body_for_agent(text: &str, agent_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "message/stream",
        "id": Uuid::new_v4().to_string(),
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{ "text": text }]
            },
            "metadata": { "agent_id": agent_id }
        }
    })
}

// ─── tests ──────────────────────────────────────────────────────────────────

/// Routing engine returns NoAgentsAvailable when DB has no running agents → 503.
#[tokio::test]
#[serial]
async fn test_routing_engine_no_agents_returns_503() {
    let server = common::TestServer::start().await;

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body("hello, which agent can help me?")),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32603);

    server.cleanup().await;
}

/// Explicit `agent_id = "orchestrator"` goes to the ReAct orchestrator path.
/// With no agents registered the orchestrator has nothing to call → 503.
#[tokio::test]
#[serial]
async fn test_explicit_orchestrator_no_agents_returns_503() {
    let server = common::TestServer::start().await;

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body_for_agent("hello", "orchestrator")),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32603);

    server.cleanup().await;
}

/// Explicit `agent_id` pointing to a non-existent agent returns 404.
#[tokio::test]
#[serial]
async fn test_explicit_agent_not_found_returns_404() {
    let server = common::TestServer::start().await;

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body_for_agent("hello", "agent-that-does-not-exist")),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 404);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32604);

    server.cleanup().await;
}

/// Missing `params` in JSON-RPC body → 400 InvalidRequest.
#[tokio::test]
#[serial]
async fn test_a2a_missing_params_returns_400() {
    let server = common::TestServer::start().await;

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&json!({ "jsonrpc": "2.0", "method": "message/stream", "id": "1" })),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32602);

    server.cleanup().await;
}

/// Text parts join to an empty string → 400 (handler: `if text.is_empty()`).
#[tokio::test]
#[serial]
async fn test_a2a_empty_text_returns_400() {
    let server = common::TestServer::start().await;

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body("")),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32602);

    server.cleanup().await;
}

/// `GET /api/router/stats` with no log rows returns `{"data":[],"total":0}`.
#[tokio::test]
#[serial]
async fn test_router_stats_empty_returns_valid_json() {
    let server = common::TestServer::start().await;

    let resp = as_superuser(server.client.get(server.url("/api/router/stats")))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["data"], json!([]));
    assert_eq!(body["total"], 0);

    server.cleanup().await;
}

/// Upload endpoint with no `query` field returns 400.
#[tokio::test]
#[serial]
async fn test_upload_no_query_field_returns_400() {
    let server = common::TestServer::start().await;

    let form = reqwest::multipart::Form::new().text("other_field", "some value");

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a/upload"))
            .multipart(form),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32602);

    server.cleanup().await;
}

/// Upload endpoint with an empty `query` field returns 400.
#[tokio::test]
#[serial]
async fn test_upload_empty_query_returns_400() {
    let server = common::TestServer::start().await;

    let form = reqwest::multipart::Form::new().text("query", "");

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a/upload"))
            .multipart(form),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32602);

    server.cleanup().await;
}

/// Upload endpoint with a valid query but no running agents returns 503.
/// Verifies the upload path also flows through the routing engine.
#[tokio::test]
#[serial]
async fn test_upload_valid_query_no_agents_returns_503() {
    let server = common::TestServer::start().await;

    let form = reqwest::multipart::Form::new().text("query", "summarize this document");

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a/upload"))
            .multipart(form),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32603);

    server.cleanup().await;
}

// ─── DB helper tests ─────────────────────────────────────────────────────────

/// Insert a test user matching SUPERUSER_ID so FK constraints are satisfied.
async fn insert_test_user(db: &PgPool) {
    sqlx::query(
        "INSERT INTO users (id, username, email, is_superuser)
         VALUES ($1, 'testadmin', 'testadmin@router-test.com', true)
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::parse_str(SUPERUSER_ID).unwrap())
    .execute(db)
    .await
    .expect("insert test user");
}

/// Insert a running agent owned by SUPERUSER_ID.
/// `url` is set to a non-existent host so the routing engine selects it but
/// the actual HTTP call fails predictably (500 Internal, not 503 NoAgents).
async fn insert_running_agent(db: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, description, status, owner_id, url, skills, tags)
         VALUES ($1, 'Test agent for router integration', 'running', $2,
                 'http://router-test-nonexistent.local:8080', '[]'::jsonb, '{}')
         RETURNING id",
    )
    .bind(name)
    .bind(Uuid::parse_str(SUPERUSER_ID).unwrap())
    .fetch_one(db)
    .await
    .expect("insert running agent")
}

/// With 1 running agent the routing engine selects it (Stage 1 skips Ollama —
/// count=1 < shortlist_threshold=15). The agent URL is unreachable so the call
/// fails with 500 Internal (not 503 NoAgentsAvailable), proving routing ran.
#[tokio::test]
#[serial]
async fn test_routing_single_agent() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    insert_running_agent(&server.db, "single-test-agent").await;

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body("what can you help me with?")),
    )
    .send()
    .await
    .unwrap();

    // 500 means the engine selected an agent and tried to reach it — not 503
    assert_ne!(resp.status(), 503, "expected routing to select the agent, not NoAgentsAvailable");

    server.cleanup().await;
}

/// With 5 running agents (< shortlist_threshold=15) Stage 1 returns all without
/// calling Ollama. Stage 3 falls back to candidates[0] (no LLM key in tests).
/// Routing produces a selection and the handler tries the agent → 500.
#[tokio::test]
#[serial]
async fn test_routing_lte_15_agents() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    for i in 0..5 {
        insert_running_agent(&server.db, &format!("catalog-agent-{i}")).await;
    }

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body("help me write code")),
    )
    .send()
    .await
    .unwrap();

    assert_ne!(resp.status(), 503, "expected an agent to be selected from the catalog");

    server.cleanup().await;
}

/// With 16+ agents the engine attempts Ollama embeddings for Stage 1.
/// Ollama is not running in CI, so VectorStore falls back to disabled mode
/// (returns all agents). Stage 3 picks the first. Verifies the Ollama-disabled
/// path does not cause NoAgentsAvailable.
#[tokio::test]
#[serial]
async fn test_routing_ollama_disabled_gt_threshold() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    for i in 0..16 {
        insert_running_agent(&server.db, &format!("ollama-test-agent-{i}")).await;
    }

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body("summarize some document")),
    )
    .send()
    .await
    .unwrap();

    // VectorStore disabled → all 16 pass Stage 1 → agent selected → 500 (unreachable)
    assert_ne!(resp.status(), 503, "Ollama fallback should not block routing");

    server.cleanup().await;
}

/// Requires Ollama with nomic-embed-text and a real LLM. Skipped in CI.
#[tokio::test]
#[serial]
#[ignore = "requires Ollama and OpenAI-compatible LLM"]
async fn test_routing_gt_15_agents_with_embeddings() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    for i in 0..20 {
        insert_running_agent(&server.db, &format!("embed-agent-{i}")).await;
    }

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body("write a python script")),
    )
    .send()
    .await
    .unwrap();

    assert_ne!(resp.status(), 503);
    server.cleanup().await;
}

/// Requires an OpenAI-compatible LLM to exercise Stage 3 selection. Skipped in CI.
#[tokio::test]
#[serial]
#[ignore = "requires OpenAI-compatible LLM"]
async fn test_routing_fallback_to_first_candidate() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    insert_running_agent(&server.db, "fallback-agent").await;

    let _ = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body("any question")),
    )
    .send()
    .await
    .unwrap();

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM router_request_log WHERE fallback_used = false")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(count > 0, "expect at least one non-fallback selection when LLM is available");

    server.cleanup().await;
}

/// Requires LLM + populated session history. Skipped in CI.
#[tokio::test]
#[serial]
#[ignore = "requires OpenAI-compatible LLM and populated session history"]
async fn test_routing_with_history() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    insert_running_agent(&server.db, "history-agent").await;

    let session_id = Uuid::new_v4().to_string();
    let body = json!({
        "jsonrpc": "2.0",
        "method": "message/stream",
        "id": Uuid::new_v4().to_string(),
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{ "text": "follow-up question" }]
            },
            "metadata": { "session_id": session_id }
        }
    });

    let resp = as_superuser(server.client.post(server.url("/api/a2a")).json(&body))
        .send()
        .await
        .unwrap();

    assert_ne!(resp.status(), 503);
    server.cleanup().await;
}

/// Upload path: multipart with query + file, 1 running agent.
/// Verifies the upload handler flows through the routing engine.
/// Returns 500 (agent unreachable) not 503 (no agents).
#[tokio::test]
#[serial]
async fn test_routing_with_file_upload() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    insert_running_agent(&server.db, "upload-test-agent").await;

    let form = reqwest::multipart::Form::new()
        .text("query", "summarize this file")
        .part(
            "document",
            reqwest::multipart::Part::bytes(b"hello world".as_ref())
                .file_name("test.txt")
                .mime_str("text/plain")
                .unwrap(),
        );

    let resp = as_superuser(
        server
            .client
            .post(server.url("/api/a2a/upload"))
            .multipart(form),
    )
    .send()
    .await
    .unwrap();

    // 500 = routing selected the agent, not 503 = no agents
    assert_ne!(resp.status(), 503, "upload routing should select the agent");

    server.cleanup().await;
}

/// After a successful routing decision the engine spawns a background log write.
/// This test waits 300 ms for the spawn to complete then asserts the row exists.
#[tokio::test]
#[serial]
async fn test_router_log_written() {
    let server = common::TestServer::start().await;
    insert_test_user(&server.db).await;
    insert_running_agent(&server.db, "log-test-agent").await;

    let query_text = "unique-query-for-log-test";

    let _ = as_superuser(
        server
            .client
            .post(server.url("/api/a2a"))
            .json(&stream_body(query_text)),
    )
    .send()
    .await
    .unwrap();

    // Background spawn writes the log — give it time to complete
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM router_request_log WHERE query = $1")
            .bind(query_text)
            .fetch_one(&server.db)
            .await
            .unwrap();

    assert_eq!(count, 1, "routing engine should have written exactly one log row");

    server.cleanup().await;
}
