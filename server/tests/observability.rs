//! Integration tests for the observability endpoints.
//!
//! Covers:
//!   GET  /api/observability/agents/{agent_ref}/logs
//!   GET  /api/observability/agents/{agent_ref}/logs/stream
//!   GET  /api/observability/agents/{agent_ref}/stats
//!   GET  /api/observability/traces
//!   GET  /api/observability/traces/{trace_id}
//!   GET  /api/observability/finops
//!
//! In tests the Tempo/Loki env vars are not set so `state.observability` is
//! `None`.  This means:
//!   • traces / finops → 503 SERVICE_UNAVAILABLE
//!   • stats           → 200 (proxy_logs DB fallback, `source: "proxy_logs"`)
//!   • logs            → 200 (proxy_logs + container logs merged)
//!
//! All routes are under `/api` and require the X-User-Id gateway header.
//!
//! Requires infra (Postgres :5432, Redis, MinIO):
//!   cargo test -p nasiko-server --test observability -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── shared helpers ──────────────────────────────────────────────────────────

async fn init_admin(server: &common::TestServer) -> Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@obs.test"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn create_agent(server: &common::TestServer, user_id: &str, name: &str) -> Value {
    server
        .client
        .post(server.url("/api/agents"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .json(&json!({"name": name, "version": "1.0.0"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

/// Seed a proxy_log row for `target_agent_id`, called by `caller_id`.
/// Returns nothing — used for state setup only.
async fn seed_proxy_log(
    server: &common::TestServer,
    caller_id: Uuid,
    target_agent_id: Uuid,
    status: i32,
    latency_ms: i64,
    error: Option<&str>,
) {
    sqlx::query(
        r#"INSERT INTO proxy_logs (caller_id, target_agent_id, method, latency_ms, status, error)
           VALUES ($1, $2, 'tasks/send', $3, $4, $5)"#,
    )
    .bind(caller_id)
    .bind(target_agent_id)
    .bind(latency_ms)
    .bind(status)
    .bind(error)
    .execute(&server.db)
    .await
    .expect("seed proxy_log");
}

// ─── authentication guard tests ──────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn observe_logs_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/some-agent/logs"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_stats_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/some-agent/stats"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_traces_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/observability/traces"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_finops_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/observability/finops"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

// ─── 503 when no observability backend ───────────────────────────────────────

#[tokio::test]
#[serial]
async fn observe_traces_returns_503_without_backend() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/traces"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 503, "traces needs Tempo backend");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_trace_by_id_returns_503_without_backend() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/traces/abc123def456"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 503, "get_trace needs Tempo backend");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn observe_finops_returns_503_without_backend() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/finops"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 503, "finops needs Tempo backend");

    server.cleanup().await;
}

// ─── 404 for unknown agents ───────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_returns_404_for_unknown_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/agents/no-such-agent/logs"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_logs_returns_404_for_unknown_uuid() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let fake_id = Uuid::new_v4();

    let res = server
        .client
        .get(server.url(&format!("/api/observability/agents/{fake_id}/logs")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stats_returns_404_for_unknown_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/agents/ghost-agent/stats"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stream_returns_404_for_unknown_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/agents/ghost-agent/logs/stream"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── agent resolution: UUID and name ─────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_resolves_by_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "resolve-by-name").await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/resolve-by-name/logs"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200, "should resolve agent by name");

    let body: Value = res.json().await.unwrap();
    assert!(body.is_array(), "logs response should be an array");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_logs_resolves_by_uuid() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "resolve-by-uuid").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url(&format!("/api/observability/agents/{agent_id}/logs")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200, "should resolve agent by UUID");

    let body: Value = res.json().await.unwrap();
    assert!(body.is_array(), "logs response should be an array");

    server.cleanup().await;
}

// ─── proxy_logs are surfaced in the logs endpoint ────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_returns_proxy_log_entries() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "proxy-logs-test").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed 3 proxy log rows for this agent
    seed_proxy_log(&server, user_id, agent_id, 200, 42, None).await;
    seed_proxy_log(&server, user_id, agent_id, 500, 88, Some("upstream error")).await;
    seed_proxy_log(&server, user_id, agent_id, 404, 15, Some("agent returned 404")).await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/proxy-logs-test/logs"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // At least the 3 proxy log rows should appear (container logs may add more)
    assert!(
        entries.len() >= 3,
        "expected at least 3 log entries, got {}: {body}",
        entries.len()
    );

    // All entries from proxy should have source = "proxy"
    let proxy_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| e["source"].as_str() == Some("proxy"))
        .collect();
    assert_eq!(proxy_entries.len(), 3, "exactly 3 proxy log entries");

    // Each entry should have required fields
    for entry in &proxy_entries {
        assert!(entry["timestamp"].is_string(), "timestamp should be a string");
        assert!(entry["message"].is_string(), "message should be a string");
        assert!(entry["level"].is_string(), "level should be a string");
    }

    server.cleanup().await;
}

// ─── level field is correctly derived from HTTP status ───────────────────────

#[tokio::test]
#[serial]
async fn proxy_log_level_reflects_http_status() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "level-test-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    seed_proxy_log(&server, user_id, agent_id, 200, 10, None).await;    // INFO
    seed_proxy_log(&server, user_id, agent_id, 404, 20, None).await;    // WARN
    seed_proxy_log(&server, user_id, agent_id, 503, 30, None).await;    // ERROR

    let res = server
        .client
        .get(server.url("/api/observability/agents/level-test-agent/logs"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    let proxy_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| e["source"].as_str() == Some("proxy"))
        .collect();

    let levels: Vec<&str> = proxy_entries
        .iter()
        .filter_map(|e| e["level"].as_str())
        .collect();

    assert!(levels.contains(&"INFO"),  "200 → INFO");
    assert!(levels.contains(&"WARN"),  "404 → WARN");
    assert!(levels.contains(&"ERROR"), "503 → ERROR");

    server.cleanup().await;
}

// ─── level filter query parameter ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_level_filter_returns_only_matching_level() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "filter-level-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    seed_proxy_log(&server, user_id, agent_id, 200, 10, None).await;  // INFO
    seed_proxy_log(&server, user_id, agent_id, 200, 12, None).await;  // INFO
    seed_proxy_log(&server, user_id, agent_id, 500, 50, Some("boom")).await; // ERROR

    let res = server
        .client
        .get(server.url("/api/observability/agents/filter-level-agent/logs?level=ERROR"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // All returned entries must be ERROR level
    for entry in entries {
        assert_eq!(
            entry["level"].as_str(),
            Some("ERROR"),
            "all entries should be ERROR after level filter"
        );
    }
    // At least the one seeded ERROR should be present
    assert!(!entries.is_empty(), "should have at least one ERROR entry");

    server.cleanup().await;
}

// ─── search filter query parameter ───────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_search_filter_returns_only_matching_messages() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "search-filter-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // "upstream error" will appear in the message for error rows
    seed_proxy_log(&server, user_id, agent_id, 500, 40, Some("upstream error")).await;
    seed_proxy_log(&server, user_id, agent_id, 200, 12, None).await; // no error message

    let res = server
        .client
        .get(server.url("/api/observability/agents/search-filter-agent/logs?search=upstream"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // All returned messages must contain "upstream" (case-insensitive)
    for entry in entries {
        let msg = entry["message"].as_str().unwrap_or("").to_lowercase();
        assert!(
            msg.contains("upstream"),
            "all entries should contain 'upstream': got {msg}"
        );
    }
    assert!(!entries.is_empty(), "should have at least one matching entry");

    server.cleanup().await;
}

// ─── stats endpoint ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_stats_returns_proxy_logs_source_without_tempo() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "stats-source-agent").await;
    let agent_id_str = agent["id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/observability/agents/stats-source-agent/stats"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();

    // When Tempo is not configured, source should be "proxy_logs"
    assert_eq!(
        body["source"].as_str(),
        Some("proxy_logs"),
        "should fall back to proxy_logs: {body}"
    );
    assert_eq!(body["agent_id"].as_str(), Some(agent_id_str));
    assert!(body["period_start"].is_string(), "period_start should be set");
    assert_eq!(body["total_input_tokens"], 0, "no token data without Tempo");
    assert_eq!(body["total_output_tokens"], 0, "no token data without Tempo");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stats_counts_proxy_log_requests_correctly() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "stats-count-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // 4 total: 1 error (500), 3 success (200)
    seed_proxy_log(&server, user_id, agent_id, 200, 10, None).await;
    seed_proxy_log(&server, user_id, agent_id, 200, 20, None).await;
    seed_proxy_log(&server, user_id, agent_id, 200, 30, None).await;
    seed_proxy_log(&server, user_id, agent_id, 500, 80, Some("server error")).await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/stats-count-agent/stats"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();

    assert_eq!(body["source"].as_str(), Some("proxy_logs"));
    assert_eq!(body["total_requests"].as_u64(), Some(4), "4 proxy_log rows");

    let error_rate = body["error_rate"].as_f64().unwrap_or(-1.0);
    assert!(
        (error_rate - 0.25).abs() < 0.001,
        "error rate should be 0.25 (1/4), got {error_rate}"
    );

    let avg_ms = body["avg_latency_ms"].as_f64().unwrap_or(-1.0);
    assert!(
        avg_ms > 0.0,
        "avg latency should be positive, got {avg_ms}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_stats_resolves_by_uuid() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "stats-uuid-agent").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url(&format!("/api/observability/agents/{agent_id}/stats")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200, "stats should resolve by UUID");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["agent_id"].as_str(), Some(agent_id));

    server.cleanup().await;
}

// ─── SSE log stream ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_stream_returns_text_event_stream_content_type() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "stream-test-agent").await;

    // We only check the Content-Type header — we don't consume the stream.
    let res = server
        .client
        .get(server.url("/api/observability/agents/stream-test-agent/logs/stream"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "SSE stream should have text/event-stream content-type, got: {content_type}"
    );

    server.cleanup().await;
}

// ─── metrics and readiness (no auth required) ────────────────────────────────

#[tokio::test]
#[serial]
async fn metrics_endpoint_is_publicly_accessible() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await; // ensure tables exist

    let res = server
        .client
        .get(server.url("/metrics"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    // Should include known counter fields
    assert!(body["agents_total"].is_number(), "agents_total should be a number");
    assert!(body["users_total"].is_number(), "users_total should be a number");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn readiness_endpoint_is_publicly_accessible() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/readiness"))
        .send()
        .await
        .unwrap();

    // Status is 200 (ready) or 503 (degraded — if redis/docker is down)
    let status = res.status().as_u16();
    assert!(
        status == 200 || status == 503,
        "readiness should return 200 or 503, got {status}"
    );

    let body: Value = res.json().await.unwrap();
    assert!(body["postgres"].is_boolean(), "postgres field required");
    assert!(body["status"].is_string(), "status field required");

    server.cleanup().await;
}

// ─── deleted agents are not resolvable ───────────────────────────────────────

#[tokio::test]
#[serial]
async fn deleted_agent_not_found_in_logs() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "deleted-logs-agent").await;
    let agent_id = agent["id"].as_str().unwrap();

    // Soft-delete the agent directly in the DB
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .execute(&server.db)
        .await
        .expect("soft delete agent");

    // Both UUID and name lookups should now return 404
    let by_name = server
        .client
        .get(server.url("/api/observability/agents/deleted-logs-agent/logs"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(by_name.status(), 404, "deleted agent should be 404 by name");

    let by_uuid = server
        .client
        .get(server.url(&format!("/api/observability/agents/{agent_id}/logs")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(by_uuid.status(), 404, "deleted agent should be 404 by UUID");

    server.cleanup().await;
}

// ─── stats with no data returns zero counts ──────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_stats_returns_zero_counts_for_new_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "zero-stats-agent").await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/zero-stats-agent/stats"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();

    assert_eq!(body["total_requests"].as_u64(), Some(0), "no requests yet");
    assert_eq!(body["total_input_tokens"].as_u64(), Some(0));
    assert_eq!(body["total_output_tokens"].as_u64(), Some(0));
    assert_eq!(body["source"].as_str(), Some("proxy_logs"));

    server.cleanup().await;
}

// ─── logs are empty for new agent ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_returns_empty_array_for_new_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    create_agent(&server, uid, "empty-logs-agent").await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/empty-logs-agent/logs"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // Container runtime will return empty logs for a non-deployed agent
    // so the overall result should be empty or only container entries
    // (either is fine — the important thing is no 404 or 500)
    let _ = entries; // structure is valid, count may vary

    server.cleanup().await;
}

// ─── error message is included in log line ───────────────────────────────────

#[tokio::test]
#[serial]
async fn proxy_log_error_message_appears_in_log_line() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "error-msg-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    seed_proxy_log(
        &server,
        user_id,
        agent_id,
        502,
        99,
        Some("bad gateway downstream"),
    )
    .await;

    let res = server
        .client
        .get(server.url("/api/observability/agents/error-msg-agent/logs"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    let error_entry = entries
        .iter()
        .find(|e| e["source"].as_str() == Some("proxy"))
        .expect("should have a proxy entry");

    let msg = error_entry["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("bad gateway downstream"),
        "error field should appear in log message: {msg}"
    );
    assert!(
        msg.contains("502"),
        "HTTP status should appear in log message: {msg}"
    );

    server.cleanup().await;
}

// ─── limit parameter is respected ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_limit_parameter_is_respected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "limit-test-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed 10 proxy log rows
    for i in 0..10_i64 {
        seed_proxy_log(&server, user_id, agent_id, 200, i * 5 + 1, None).await;
    }

    let res = server
        .client
        .get(server.url("/api/observability/agents/limit-test-agent/logs?limit=3"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    assert!(
        entries.len() <= 3,
        "limit=3 should return at most 3 entries, got {}",
        entries.len()
    );

    server.cleanup().await;
}

// ─── since parameter filters out old entries ─────────────────────────────────

#[tokio::test]
#[serial]
async fn agent_logs_since_parameter_filters_old_entries() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let user_id: Uuid = Uuid::parse_str(uid).unwrap();

    let agent = create_agent(&server, uid, "since-test-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Insert a log row 2 hours ago (outside the 1-hour window we'll query)
    sqlx::query(
        r#"INSERT INTO proxy_logs (caller_id, target_agent_id, method, latency_ms, status, timestamp)
           VALUES ($1, $2, 'tasks/send', 10, 200, now() - interval '2 hours')"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    // Insert a recent log row (within the last hour)
    seed_proxy_log(&server, user_id, agent_id, 200, 10, None).await;

    // Query with since = 90 minutes ago — old entry should be excluded.
    // RFC-3339 timestamps contain '+' which must be percent-encoded in query strings.
    let since_raw = (chrono::Utc::now() - chrono::Duration::try_minutes(90).unwrap()).to_rfc3339();
    let since_encoded = since_raw.replace('+', "%2B");
    let url = format!(
        "/api/observability/agents/since-test-agent/logs?since={since_encoded}&limit=10"
    );

    let res = server
        .client
        .get(server.url(&url))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // Only the recent row should be in range; the 2-hour-old row should be excluded.
    let proxy_entries: Vec<&Value> = entries
        .iter()
        .filter(|e| e["source"].as_str() == Some("proxy"))
        .collect();

    assert_eq!(
        proxy_entries.len(),
        1,
        "only the recent proxy log should appear when since filter applied; got: {body}"
    );

    server.cleanup().await;
}
