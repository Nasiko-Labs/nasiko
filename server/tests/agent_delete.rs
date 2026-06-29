//! Integration tests for DELETE /api/catalog/agents/{id}.
//!
//! Verifies:
//!   - Auth / ownership guards (401, 403, 404)
//!   - Clean 200 + JSON deletion report on success; 404 on repeat delete
//!   - FK CASCADE: agent_deployments, agent_builds, agent_versions, proxy_logs
//!   - FK SET NULL: chat_sessions.agent_id becomes NULL
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_delete -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── helpers ────────────────────────────────────────────────────────────────

async fn init_admin(server: &common::TestServer) -> Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn create_agent(server: &common::TestServer, uid: &str, name: &str) -> Value {
    server
        .client
        .post(server.url("/api/catalog/agents"))
        .header("x-user-id", uid)
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

async fn delete_as_superuser(server: &common::TestServer, uid: &str, agent_id: &str) -> reqwest::Response {
    server
        .client
        .delete(server.url(&format!("/api/catalog/agents/{agent_id}")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap()
}

// ─── auth / guard tests ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn delete_agent_requires_auth() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let random_id = Uuid::new_v4();

    let res = server
        .client
        .delete(server.url(&format!("/api/catalog/agents/{random_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_unknown_agent_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let random_id = Uuid::new_v4();
    let res = delete_as_superuser(&server, uid, &random_id.to_string()).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_agent_by_non_owner_returns_403() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // Agent owned by admin.
    let agent = create_agent(&server, uid, "owned-agent").await;
    let agent_id = agent["id"].as_str().unwrap();

    // Create a second user.
    let other: Value = server
        .client
        .post(server.url("/api/users"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .json(&json!({"username": "other", "email": "other@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap();

    let res = server
        .client
        .delete(server.url(&format!("/api/catalog/agents/{agent_id}")))
        .header("x-user-id", other_id)
        .header("x-username", "other")
        .header("x-is-superuser", "false")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

// ─── basic delete ────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn delete_agent_returns_200_with_report() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "delete-me").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = delete_as_superuser(&server, uid, agent_id).await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(body["deleted"].as_bool().unwrap());
    assert_eq!(body["agent_id"].as_str().unwrap(), agent_id);
    assert!(body["runtime_errors"].is_array());

    // Second delete → 404.
    let res = delete_as_superuser(&server, uid, agent_id).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── CASCADE tests ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn delete_cascades_builds_and_versions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "cascade-build-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'cascade-build-agent:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback) \
         VALUES ($1, '1.0.0', 'cascade-build-agent:1.0.0', true, false)",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id) \
         VALUES ($1, $2, 'running', $3)",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(uid_uuid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let build_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_builds WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(build_count, 0, "agent_builds should cascade-delete");

    let version_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_versions WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(version_count, 0, "agent_versions should cascade-delete");

    let deploy_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_deployments WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(deploy_count, 0, "agent_deployments should cascade-delete");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_cascades_proxy_logs() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "proxy-log-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    sqlx::query(
        "INSERT INTO proxy_logs (caller_id, target_agent_id, method, latency_ms, status) \
         VALUES ($1, $2, 'POST', 42, 200)",
    )
    .bind(uid_uuid)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let log_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM proxy_logs WHERE target_agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(log_count, 1, "pre-condition: log row inserted");

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let log_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM proxy_logs WHERE target_agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(log_count_after, 0, "proxy_logs should cascade-delete");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_nullifies_chat_sessions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "chat-session-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let session_id = "test-session-for-delete";
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title) \
         VALUES ($1, $2, $3, 'Test Session')",
    )
    .bind(session_id)
    .bind(uid_uuid)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let agent_id_in_session: Option<Uuid> =
        sqlx::query_scalar("SELECT agent_id FROM chat_sessions WHERE session_id = $1")
            .bind(session_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(
        agent_id_in_session.is_none(),
        "chat_sessions.agent_id should be NULL after agent delete"
    );

    server.cleanup().await;
}

// ─── deletion report ─────────────────────────────────────────────────────────

// Regression guard: agent_deployments.namespace is the K8s namespace ('nasiko-agents'),
// NOT the container name. A deployed agent with no k8s_deployment_name (Docker OSS path)
// must NOT produce spurious runtime_errors entries for the namespace value.
#[tokio::test]
#[serial]
async fn delete_report_has_no_spurious_runtime_errors_for_docker_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "docker-teardown-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'docker-teardown-agent:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Docker row: namespace='nasiko-agents' (default), k8s_deployment_name=NULL.
    // The teardown query selects k8s_deployment_name; since it is NULL here, no extra
    // container IDs are added and 'nasiko-agents' is never passed to runtime.destroy.
    sqlx::query(
        "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id) \
         VALUES ($1, $2, 'running', $3)",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(uid_uuid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    assert!(body["deleted"].as_bool().unwrap());
    assert_eq!(body["agent_id"].as_str().unwrap(), agent_id.to_string());

    // runtime.destroy on a never-started container returns an error — that is expected.
    // There must be at most ONE entry (for the agent name itself), never a second one
    // for the K8s namespace string 'nasiko-agents'.
    let errors = body["runtime_errors"].as_array().unwrap();
    assert!(
        errors.len() <= 1,
        "expected at most 1 runtime_error (agent name, may not exist), got {}: {errors:?}",
        errors.len()
    );
    if let Some(err) = errors.first() {
        let msg = err.as_str().unwrap();
        assert!(
            !msg.contains("nasiko-agents"),
            "runtime_errors must not reference the K8s namespace: {msg}"
        );
    }

    server.cleanup().await;
}
