//! Integration tests for Phase 2 — Agent Lifecycle Visibility.
//!
//! Covers: GET /api/agents/deployments, /api/agents/{id}/deployment,
//!         /api/agents/uploads/{id}, /api/agents/my-uploads,
//!         /api/agents/{id}/versions.
//!
//! Uses direct DB seeding (server.db) instead of triggering real Docker builds,
//! keeping these tests fast and purely focused on the read-path endpoints.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_lifecycle -- --test-threads=1

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

/// POST /api/agents as superuser; returns the created agent JSON.
async fn create_agent(server: &common::TestServer, uid: &str, name: &str, version: &str) -> Value {
    common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
        .json(&json!({"name": name, "version": version}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn get_as_superuser(server: &common::TestServer, uid: &str, path: &str) -> reqwest::Response {
    common::as_superuser(server.client.get(server.url(path)), uid, "admin")
        .send()
        .await
        .unwrap()
}

// ─── GET /api/agents/deployments ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn list_deployments_returns_empty_array() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = get_as_superuser(&server, uid, "/api/agents/deployments").await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(body.is_array(), "expected array, got: {body}");
    assert_eq!(body.as_array().unwrap().len(), 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_deployments_requires_auth() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    let res = server
        .client
        .get(server.url("/api/agents/deployments"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_deployments_shows_seeded_record() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "deploy-vis-agent", "1.0.0").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed a build + deployment record directly.
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'deploy-vis-agent:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
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

    let res = get_as_superuser(&server, uid, "/api/agents/deployments").await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let records = body.as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0]["agent_id"].as_str().unwrap(),
        agent_id.to_string()
    );
    assert_eq!(records[0]["status"].as_str().unwrap(), "running");

    server.cleanup().await;
}

// ─── GET /api/agents/{id}/deployment ────────────────────────────────────────

#[tokio::test]
#[serial]
async fn get_agent_deployment_unknown_agent_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let random_id = Uuid::new_v4();
    let res = get_as_superuser(&server, uid, &format!("/api/agents/{random_id}/deployment")).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_agent_deployment_no_deployment_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "no-deploy-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = get_as_superuser(&server, uid, &format!("/api/agents/{agent_id}/deployment")).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_agent_deployment_returns_seeded_record() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "has-deploy-agent", "1.0.0").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'has-deploy-agent:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
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

    let res = get_as_superuser(&server, uid, &format!("/api/agents/{agent_id}/deployment")).await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["agent_id"].as_str().unwrap(), agent_id.to_string());
    assert_eq!(body["status"].as_str().unwrap(), "running");

    server.cleanup().await;
}

// ─── GET /api/agents/uploads/{id} ─────────────────────────────────────

#[tokio::test]
#[serial]
async fn get_upload_status_unknown_id_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = get_as_superuser(&server, uid, "/api/agents/uploads/nonexistent-id").await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_upload_status_returns_seeded_record() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let upload_id = "test-upload-42";

    sqlx::query(
        "INSERT INTO upload_status (upload_id, agent_name, owner_id, status) \
         VALUES ($1, 'seeded-agent', $2, 'completed'::upload_pipeline_status)",
    )
    .bind(upload_id)
    .bind(uid_uuid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = get_as_superuser(&server, uid, &format!("/api/agents/uploads/{upload_id}")).await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["upload_id"].as_str().unwrap(), upload_id);
    assert_eq!(body["agent_name"].as_str().unwrap(), "seeded-agent");
    assert_eq!(body["status"].as_str().unwrap(), "completed");

    server.cleanup().await;
}

/// A non-owner must not be able to read another user's upload status by
/// guessing/knowing the upload_id (IDOR — this handler previously had no
/// `Claims` param at all, unlike its owner-scoped sibling `list_upload_status`).
#[tokio::test]
#[serial]
async fn get_upload_status_denies_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_uid = admin["user_id"].as_str().unwrap();
    let owner_uuid: Uuid = owner_uid.parse().unwrap();

    let stranger_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ('upload-idor-stranger', 'upload-idor-stranger@test.local', false) RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();

    let upload_id = "test-upload-idor";
    sqlx::query(
        "INSERT INTO upload_status (upload_id, agent_name, owner_id, status) \
         VALUES ($1, 'private-agent', $2, 'completed'::upload_pipeline_status)",
    )
    .bind(upload_id)
    .bind(owner_uuid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/uploads/{upload_id}"))),
        &stranger_id.to_string(),
        "upload-idor-stranger",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        404,
        "a non-owner must not see another user's upload status"
    );

    // The owner and a superuser must still be able to read it.
    let res_owner = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/uploads/{upload_id}"))),
        owner_uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res_owner.status(),
        200,
        "the owner must still be able to read their own upload status"
    );

    server.cleanup().await;
}

// ─── GET /api/agents/my-uploads ─────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn list_upload_agents_returns_empty_initially() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = get_as_superuser(&server, uid, "/api/agents/my-uploads").await;
    assert_eq!(res.status(), 200);
    // The handler wraps the list in a `{ data, status_code, message }`
    // envelope (UploadAgentsListResponse).
    let body: Value = res.json().await.unwrap();
    assert!(body["data"].is_array());
    assert_eq!(body["data"].as_array().unwrap().len(), 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_upload_agents_scoped_to_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    // Create a real second user so upload_status.owner_id FK is satisfied.
    let other_resp: Value =
        common::as_superuser(server.client.post(server.url("/api/users")), uid, "admin")
            .json(&json!({"username": "other", "email": "other@test.local"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let other_user: Uuid = other_resp["id"].as_str().unwrap().parse().unwrap();

    sqlx::query(
        "INSERT INTO upload_status (upload_id, agent_name, owner_id, status) VALUES
         ($1, 'mine',   $2, 'completed'::upload_pipeline_status),
         ($3, 'theirs', $4, 'completed'::upload_pipeline_status)",
    )
    .bind("upload-mine")
    .bind(uid_uuid)
    .bind("upload-theirs")
    .bind(other_user)
    .execute(&server.db)
    .await
    .unwrap();

    // Non-superuser sees only their own record.
    let res = common::as_member(
        server.client.get(server.url("/api/agents/my-uploads")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let records = body["data"].as_array().unwrap();
    assert_eq!(
        records.len(),
        1,
        "non-superuser should see only own uploads"
    );
    assert_eq!(records[0]["agent_name"].as_str().unwrap(), "mine");

    // Superuser sees both.
    let res = get_as_superuser(&server, uid, "/api/agents/my-uploads").await;
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        2,
        "superuser should see all uploads"
    );

    server.cleanup().await;
}

// ─── GET /api/agents/{id}/versions ──────────────────────────────────

#[tokio::test]
#[serial]
async fn list_versions_empty_for_new_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "version-test-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = get_as_superuser(&server, uid, &format!("/api/agents/{agent_id}/versions")).await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_versions_returns_seeded_versions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "versioned-agent", "1.0.0").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed two versions directly.
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status)
         VALUES
           ($1, '1.0.0', 'versioned-agent:1.0.0', false, true,  'archived'),
           ($1, '1.0.1', 'versioned-agent:1.0.1', true,  false, 'active')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let res = get_as_superuser(&server, uid, &format!("/api/agents/{agent_id}/versions")).await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let versions = body.as_array().unwrap();
    assert_eq!(versions.len(), 2);

    // Ordered by created_at DESC → 1.0.1 first.
    let first = &versions[0];
    assert_eq!(first["version"].as_str().unwrap(), "1.0.1");
    assert!(first["is_active"].as_bool().unwrap());
    assert!(!first["can_rollback"].as_bool().unwrap());

    let second = &versions[1];
    assert_eq!(second["version"].as_str().unwrap(), "1.0.0");
    assert!(second["can_rollback"].as_bool().unwrap());

    server.cleanup().await;
}
