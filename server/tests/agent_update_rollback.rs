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

/// Seed a second, plain `users` row — needed whenever a test acts as a
/// superuser distinct from the `init_admin` account, since `agent_builds
/// .triggered_by` and similar columns carry a real FK to `users(id)`.
async fn seed_user(server: &common::TestServer, user_id: &str) {
    let uid: Uuid = user_id.parse().unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(format!("user_{}", &user_id[..8]))
    .bind(format!("user_{}@test.example", &user_id[..8]))
    .execute(&server.db)
    .await
    .expect("seed_user");
}

/// Create an agent via catalog and return its JSON (synchronous, no Docker).
async fn create_agent(server: &common::TestServer, uid: &str, name: &str, version: &str) -> Value {
    let res = common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
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
        form = form.part(
            "source",
            reqwest::multipart::Part::bytes(zip).file_name("agent.zip"),
        );
    }
    common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent_id}/update"))),
        uid,
        "admin",
    )
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
    let req = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}/rollback"))),
        uid,
        "admin",
    );
    match body {
        Some(b) => req.json(&b).send().await.unwrap(),
        None => req.send().await.unwrap(),
    }
}

/// Poll GET /api/builds/{build_id} until the status is terminal.
async fn wait_for_terminal_build(server: &common::TestServer, uid: &str, build_id: &str) -> String {
    for _ in 0..60 {
        let res = common::as_superuser(
            server
                .client
                .get(server.url(&format!("/api/builds/{build_id}"))),
            uid,
            "admin",
        )
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

    let form = reqwest::multipart::Form::new().part(
        "source",
        reqwest::multipart::Part::bytes(zip).file_name("agent.zip"),
    );
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
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent_id}/update"))),
        uid,
        "admin",
    )
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
    assert!(
        text.contains("not valid semver"),
        "expected semver error, got: {text}"
    );

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
async fn update_explicit_prerelease_version_returns_400() {
    // A pre-release like "1.2.3-beta" passes generic SemVer parsing (and is
    // greater than "1.0.0"), but `record_version_change` only ever accepts a
    // plain `x.y.z`. Before this was validated up front, a prerelease would
    // sail through this check, the build/deploy would proceed, and only the
    // version-history write afterward would silently fail — leaving the
    // agent running a version with no matching row in `agent_versions`.
    // It must now be rejected here, before any build starts.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "prerelease-agent", "1.0.0").await;
    let agent_id = agent["id"].as_str().unwrap();
    let zip = common::make_zip(&[NO_DOCKERFILE]);

    let res = do_update(&server, uid, agent_id, Some("1.2.3-beta"), Some(zip)).await;
    assert_eq!(res.status(), 400);
    let text = res.text().await.unwrap();
    assert!(
        text.contains("x.y.z"),
        "expected plain x.y.z format error, got: {text}"
    );

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
    assert!(
        text.contains("already exists"),
        "expected 'already exists' message, got: {text}"
    );

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
    let agent_res: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    // GET /api/agents/{id} wraps the agent in a { data, status_code, message }
    // envelope (SingleResponse in oss/server/src/catalog/routes.rs).
    assert_eq!(
        agent_res["data"]["version"].as_str().unwrap(),
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

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}/rollback"))),
        uid,
        "admin",
    )
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

    let res = do_rollback(
        &server,
        uid,
        agent_id,
        Some(json!({"target_version": "9.9.9"})),
    )
    .await;
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
    let res = do_rollback(
        &server,
        uid,
        &agent_id.to_string(),
        Some(json!({"target_version": "0.9.0"})),
    )
    .await;
    assert_eq!(res.status(), 400);
    let text = res.text().await.unwrap();
    assert!(
        text.contains("not rollback-eligible"),
        "expected eligibility error, got: {text}"
    );

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

// ─── Owner-identity integrity: a superuser acting on someone else's agent ───
//
// Regression coverage for a real bug: `update`/`rollback` fetched the agent's
// real owner from the DB, then discarded it and queued the background job
// with the *caller's* id instead. When a superuser (not the owner) updates or
// rolls back someone else's agent, this silently pointed the agent's
// LLM-router wiring — and the persisted `agent_deployments.owner_id` used by
// every future restart — at the superuser's account instead of the real
// owner's. These tests seed an agent owned by one user, then act on it as a
// *different* superuser, and assert the queued job carries the real owner.

#[tokio::test]
#[serial]
async fn rollback_by_different_superuser_queues_job_with_real_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_uid = admin["user_id"].as_str().unwrap();
    let caller_uid = Uuid::new_v4().to_string();
    seed_user(&server, &caller_uid).await;

    let agent = create_agent(&server, owner_uid, "rollback-owner-guard-agent", "1.0.1").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '1.0.0', 'rollback-owner-guard-agent:1.0.0', false, true, 'archived')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    // A different superuser (not the agent's owner) triggers the rollback.
    let res = do_rollback(&server, &caller_uid, &agent_id.to_string(), None).await;
    assert_eq!(res.status(), 202);

    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM build_jobs WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(
        payload["agent_owner_id"].as_str().unwrap(),
        owner_uid,
        "queued rollback job must carry the agent's real owner, not the caller"
    );
    assert_eq!(
        payload["caller_id"].as_str().unwrap(),
        caller_uid,
        "the acting superuser should still be recorded as the caller for audit purposes"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_by_different_superuser_queues_job_with_real_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_uid = admin["user_id"].as_str().unwrap();
    let caller_uid = Uuid::new_v4().to_string();
    seed_user(&server, &caller_uid).await;

    let agent = create_agent(&server, owner_uid, "update-owner-guard-agent", "1.0.0").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // A different superuser (not the agent's owner) triggers the update.
    let zip = common::make_zip(&[NO_DOCKERFILE]);
    let res = do_update(&server, &caller_uid, &agent_id.to_string(), None, Some(zip)).await;
    assert_eq!(res.status(), 202);

    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM build_jobs WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(
        payload["agent_owner_id"].as_str().unwrap(),
        owner_uid,
        "queued update job must carry the agent's real owner, not the caller"
    );
    assert_eq!(
        payload["owner_id"].as_str().unwrap(),
        caller_uid,
        "the acting superuser should still be recorded as owner_id for status/audit tracking"
    );

    server.cleanup().await;
}
