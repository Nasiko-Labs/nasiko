//! Integration tests for Phase 3 — Agent Update & Rollback.
//!
//! Covers: PUT /api/agents/{id}/update and POST /api/agents/{id}/rollback.
//!
//! Update tests use zips without a Dockerfile so the background build fails
//! fast and deterministically — no actual Docker image is produced. This lets
//! us verify 202 acceptance, version-bump logic, and build-failure handling
//! without requiring a runnable Dockerfile.
//!
//! Rollback tests seed agent_versions directly via server.db so they can test
//! eligibility checks and response shape independently of a real build pipeline.
//!
//! Requires infra (Postgres :5432, Redis, S3, Docker socket):
//!   cargo test -p nasiko-server --test agent_update_rollback -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use std::time::Duration;
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

/// Create an agent via catalog and return its JSON (synchronous, no Docker).
async fn create_agent(server: &common::TestServer, uid: &str, name: &str, version: &str) -> Value {
    let res = server
        .client
        .post(server.url("/api/agents"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .json(&json!({"name": name, "version": version}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "create_agent failed");
    res.json::<Value>().await.unwrap()
}

/// PUT /api/agents/{id}/update as superuser. `version` is the multipart
/// "version" field (strategy keyword or explicit semver); omit to send none.
async fn do_update(
    server: &common::TestServer,
    uid: &str,
    agent_id: &str,
    version: Option<&str>,
    source: Option<Vec<u8>>,
) -> reqwest::Response {
    let mut form = reqwest::multipart::Form::new();
    if let Some(v) = version {
        form = form.text("version", v.to_string());
    }
    if let Some(zip) = source {
        form = form.part("source", reqwest::multipart::Part::bytes(zip).file_name("agent.zip"));
    }
    server
        .client
        .put(server.url(&format!("/api/agents/{agent_id}/update")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .multipart(form)
        .send()
        .await
        .unwrap()
}

/// POST /api/agents/{id}/rollback as superuser.
async fn do_rollback(
    server: &common::TestServer,
    uid: &str,
    agent_id: &str,
    body: Option<Value>,
) -> reqwest::Response {
    let req = server
        .client
        .post(server.url(&format!("/api/agents/{agent_id}/rollback")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin");
    match body {
        Some(b) => req.json(&b).send().await.unwrap(),
        None => req.send().await.unwrap(),
    }
}

/// Poll GET /api/builds/{build_id} until the status is terminal.
async fn wait_for_terminal_build(
    server: &common::TestServer,
    uid: &str,
    build_id: &str,
) -> String {
    for _ in 0..60 {
        let res = server
            .client
            .get(server.url(&format!("/api/builds/{build_id}")))
            .header("x-user-id", uid)
            .header("x-username", "admin")
            .header("x-is-superuser", "true")
            .send()
            .await
            .unwrap();
        if res.status() == 200 {
            let body: Value = res.json().await.unwrap();
            if let Some(s) = body["status"].as_str()
                && (s == "success" || s == "failed")
            {
                return s.to_string();
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("build {build_id} never reached a terminal status");
}

const NO_DOCKERFILE: (&str, &[u8]) = ("README.md", b"no dockerfile here");

// ─── PUT /api/agents/{id}/update — validation ────────────────────────────────

#[tokio::test]
#[serial]
async fn update_unknown_agent_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let random_id = Uuid::new_v4().to_string();
    let zip = common::make_zip(&[NO_DOCKERFILE]);
    let res = do_update(&server, uid, &random_id, None, Some(zip)).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_requires_auth() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "auth-guard-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let form = reqwest::multipart::Form::new()
        .part("source", reqwest::multipart::Part::bytes(zip).file_name("agent.zip"));
    let res = server
        .client
        .put(server.url(&format!("/api/agents/{agent_id}/update")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_rejects_non_zip_extension() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "ext-check-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();

    let form = reqwest::multipart::Form::new().part(
        "source",
        // .tar.gz instead of .zip → 400
        reqwest::multipart::Part::bytes(b"fake data".to_vec()).file_name("agent.tar.gz"),
    );
    let res = server
        .client
        .put(server.url(&format!("/api/agents/{agent_id}/update")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_non_semver_agent_without_explicit_version_returns_400() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // "latest" is not valid semver → auto-bump is impossible.
    let agent = create_agent(&server, uid, "nonsemver-agent", "latest").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let res = do_update(&server, uid, agent_id, None, Some(zip)).await;
    assert_eq!(res.status(), 400);
    let text = res.text().await.unwrap();
    assert!(text.contains("not valid semver"), "expected semver error, got: {text}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_explicit_version_less_than_current_returns_409() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "version-guard-agent", "2.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    // 1.0.0 < 2.0.0 → conflict
    let res = do_update(&server, uid, agent_id, Some("1.0.0"), Some(zip)).await;
    assert_eq!(res.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_explicit_version_equal_to_current_returns_409() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "eq-version-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    // Providing exact current version → conflict (must be strictly greater)
    let res = do_update(&server, uid, agent_id, Some("1.0.0"), Some(zip)).await;
    assert_eq!(res.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_duplicate_version_in_agent_versions_returns_409() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "dup-version-agent", "1.0.0").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed agent_versions with the would-be next version.
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '1.0.1', 'dup-version-agent:1.0.1', false, false, 'archived')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let zip = common::make_zip(&[NO_DOCKERFILE]);
    // Auto-bump from 1.0.0 → 1.0.1 which already exists → 409
    let res = do_update(&server, uid, agent_id.to_string().as_str(), None, Some(zip)).await;
    assert_eq!(res.status(), 409);
    let text = res.text().await.unwrap();
    assert!(text.contains("already exists"), "expected 'already exists' message, got: {text}");

    server.cleanup().await;
}

// ─── PUT /api/agents/{id}/update — version bump logic ────────────────────────

#[tokio::test]
#[serial]
async fn update_auto_bumps_patch_version() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "patch-bump-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let res = do_update(&server, uid, agent_id, None, Some(zip)).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["new_version"].as_str().unwrap(), "1.0.1");
    assert_eq!(body["previous_version"].as_str().unwrap(), "1.0.0");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_minor_strategy_bumps_minor() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "minor-bump-agent", "1.2.3").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let res = do_update(&server, uid, agent_id, Some("minor"), Some(zip)).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["new_version"].as_str().unwrap(), "1.3.0");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_major_strategy_bumps_major() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "major-bump-agent", "1.2.3").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let res = do_update(&server, uid, agent_id, Some("major"), Some(zip)).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["new_version"].as_str().unwrap(), "2.0.0");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_explicit_semver_accepted() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "explicit-ver-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let res = do_update(&server, uid, agent_id, Some("3.1.4"), Some(zip)).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["new_version"].as_str().unwrap(), "3.1.4");
    assert_eq!(body["previous_version"].as_str().unwrap(), "1.0.0");
    assert_eq!(body["status"].as_str().unwrap(), "queued");
    assert!(uuid::Uuid::parse_str(body["build_id"].as_str().unwrap()).is_ok());
    assert_eq!(body["agent_id"].as_str().unwrap(), agent_id);

    server.cleanup().await;
}

// ─── PUT /api/agents/{id}/update — background task ───────────────────────────

#[tokio::test]
#[serial]
async fn update_marks_build_failed_without_dockerfile() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "no-dockerfile-update-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let body: Value = do_update(&server, uid, agent_id, None, Some(zip))
        .await
        .json()
        .await
        .unwrap();
    let build_id = body["build_id"].as_str().unwrap();

    let status = wait_for_terminal_build(&server, uid, build_id).await;
    assert_eq!(status, "failed", "build without Dockerfile must fail");

    // Agent version should be rolled back to the original.
    let agent_res: Value = server
        .client
        .get(server.url(&format!("/api/agents/{agent_id}")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        agent_res["version"].as_str().unwrap(),
        "1.0.0",
        "version should be rolled back after failed build"
    );

    server.cleanup().await;
}

// ─── POST /api/agents/{id}/rollback — validation ─────────────────────────────

#[tokio::test]
#[serial]
async fn rollback_unknown_agent_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let random_id = Uuid::new_v4().to_string();
    let res = do_rollback(&server, uid, &random_id, None).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_requires_auth() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "rollback-auth-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = server
        .client
        .post(server.url(&format!("/api/agents/{agent_id}/rollback")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_no_eligible_version_returns_409() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // Agent exists but has no agent_versions rows → nothing to roll back to.
    let agent = create_agent(&server, uid, "no-rollback-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = do_rollback(&server, uid, agent_id, None).await;
    assert_eq!(res.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_malformed_json_returns_422() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "bad-json-rollback-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = server
        .client
        .post(server.url(&format!("/api/agents/{agent_id}/rollback")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("content-type", "application/json")
        .body("{not valid json}")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 422);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_specific_version_not_found_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "missing-ver-rollback-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = do_rollback(&server, uid, agent_id, Some(json!({"target_version": "9.9.9"}))).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_version_not_eligible_returns_400() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "ineligible-rollback-agent", "1.0.0").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed a version with can_rollback = false.
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '0.9.0', 'ineligible-rollback-agent:0.9.0', false, false, 'archived')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let uid = admin["user_id"].as_str().unwrap();
    let res = do_rollback(&server, uid, &agent_id.to_string(), Some(json!({"target_version": "0.9.0"}))).await;
    assert_eq!(res.status(), 400);
    let text = res.text().await.unwrap();
    assert!(text.contains("not rollback-eligible"), "expected eligibility error, got: {text}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_eligible_version_returns_202_with_correct_fields() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "eligible-rollback-agent", "1.0.1").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed a rollback-eligible previous version.
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '1.0.0', 'eligible-rollback-agent:1.0.0', false, true, 'archived')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let res = do_rollback(&server, uid, &agent_id.to_string(), None).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();

    assert_eq!(body["rolled_back_to"].as_str().unwrap(), "1.0.0");
    assert_eq!(body["rolled_back_from"].as_str().unwrap(), "1.0.1");
    assert_eq!(body["status"].as_str().unwrap(), "queued");
    assert_eq!(body["agent_id"].as_str().unwrap(), agent_id.to_string());
    assert!(uuid::Uuid::parse_str(body["build_id"].as_str().unwrap()).is_ok());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_to_specific_eligible_version() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "targeted-rollback-agent", "1.2.0").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // Seed two rollback-eligible versions; we'll pick the older one.
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status)
         VALUES
           ($1, '1.0.0', 'targeted-rollback-agent:1.0.0', false, true, 'archived'),
           ($1, '1.1.0', 'targeted-rollback-agent:1.1.0', false, true, 'archived')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let res = do_rollback(
        &server,
        uid,
        &agent_id.to_string(),
        Some(json!({"target_version": "1.0.0"})),
    )
    .await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["rolled_back_to"].as_str().unwrap(), "1.0.0");

    server.cleanup().await;
}
