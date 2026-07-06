//! RBAC integration tests for build and deploy routes.
//!
//! Verifies that `require_deployer` is enforced on:
//!   POST /api/builds
//!   POST /api/agents/upload
//!   GET  /api/builds/agent/{agent_id}  (ownership scoping)
//!
//! The server validates JWT Bearer tokens directly.
//! Tests use the common JWT helpers to simulate different identity contexts.
//!
//! Requires: `docker compose --profile infra up -d postgres redis`
//! Run: `cargo test -p nasiko-server --test build_rbac -- --test-threads=1`

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── Helpers ────────────────────────────────────────────────────────────────

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

async fn create_user(server: &common::TestServer, admin_id: &str, username: &str) -> Value {
    common::as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": username, "email": format!("{username}@test.local")}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

/// Create an agent owned by the given user (superuser headers).
async fn create_agent(server: &common::TestServer, owner_id: &str) -> Value {
    common::as_superuser(
        server.client.post(server.url("/api/agents")),
        owner_id,
        "admin",
    )
    .json(&json!({"name": format!("agent-{}", Uuid::new_v4().simple())}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

/// POST /api/builds with given identity; multipart role check only (no real zip).
async fn try_trigger_build(
    server: &common::TestServer,
    user_id: &str,
    is_superuser: bool,
    role: &str,
    agent_id: &str,
) -> u16 {
    let form = reqwest::multipart::Form::new()
        .text("agent_id", agent_id.to_string())
        .text("version_tag", "v1");
    let token = common::sign_token(user_id, "u", is_superuser, role);
    server
        .client
        .post(server.url("/api/builds"))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// POST /api/agents/upload with given identity; RBAC check only (no real zip).
async fn try_upload_deploy(
    server: &common::TestServer,
    user_id: &str,
    is_superuser: bool,
    role: &str,
) -> u16 {
    let form = reqwest::multipart::Form::new().text("name", "test-agent");
    let token = common::sign_token(user_id, "u", is_superuser, role);
    server
        .client
        .post(server.url("/api/agents/upload"))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// GET /api/builds/agent/{agent_id} with given identity.
async fn try_list_builds(
    server: &common::TestServer,
    user_id: &str,
    is_superuser: bool,
    role: &str,
    agent_id: &str,
) -> u16 {
    let token = common::sign_token(user_id, "u", is_superuser, role);
    server
        .client
        .get(server.url(&format!("/api/builds/agent/{agent_id}")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

// ─── Tests: POST /api/builds ──────────────────────────────────────────

/// OSS has no role-based RBAC — the `AuthService` grants every permission, so any
/// authenticated identity reaches the build handler (404/400 for bad input, never
/// a 403 role gate). Role-based blocking only exists in the EE `EeAuthService`.
#[tokio::test]
#[serial]
async fn oss_any_identity_reaches_build_handler() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let user = create_user(&server, admin_id, "user-build").await;
    let user_id = user["id"].as_str().unwrap();

    let status = try_trigger_build(&server, user_id, false, "member", &Uuid::new_v4().to_string()).await;
    assert_ne!(status, 403, "OSS allow-all authorizer must not block any identity with a 403");

    server.cleanup().await;
}

/// team_member is the deployer threshold — request passes RBAC and reaches the
/// handler (which returns 400/404 for missing source, not 403).
#[tokio::test]
#[serial]
async fn team_member_can_reach_build_handler() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let deployer = create_user(&server, admin_id, "deployer-build").await;
    let deployer_id = deployer["id"].as_str().unwrap();

    let status = try_trigger_build(&server, deployer_id, false, "team_member", &Uuid::new_v4().to_string()).await;
    // Handler is reached — 400 (missing source) or 404 (agent not found), never 403.
    assert_ne!(status, 403, "team_member must pass RBAC gate");

    server.cleanup().await;
}

/// Superuser always bypasses RBAC — OSS default identity.
#[tokio::test]
#[serial]
async fn superuser_can_trigger_build() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let status = try_trigger_build(&server, admin_id, true, "admin", &Uuid::new_v4().to_string()).await;
    assert_ne!(status, 403, "superuser must not be blocked");

    server.cleanup().await;
}

// ─── Tests: POST /api/agents/upload ──────────────────────────────

#[tokio::test]
#[serial]
async fn oss_any_identity_reaches_upload_handler() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let user = create_user(&server, admin_id, "user-upload").await;
    let user_id = user["id"].as_str().unwrap();

    let status = try_upload_deploy(&server, user_id, false, "member").await;
    assert_ne!(status, 403, "OSS allow-all authorizer must not block any identity with a 403");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn team_member_can_reach_upload_handler() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let deployer = create_user(&server, admin_id, "deployer-upload").await;
    let deployer_id = deployer["id"].as_str().unwrap();

    let status = try_upload_deploy(&server, deployer_id, false, "team_member").await;
    // Handler reached — 400 (missing source zip), never 403.
    assert_ne!(status, 403, "team_member must pass RBAC gate for upload-and-deploy");

    server.cleanup().await;
}

// ─── Tests: GET /api/builds/agent/{id} (ownership) ───────────────────

/// Non-owner cannot see build history for another user's agent.
#[tokio::test]
#[serial]
async fn non_owner_cannot_list_builds_for_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    // Admin owns the agent.
    let agent = create_agent(&server, admin_id).await;
    let agent_id = agent["id"].as_str().unwrap();

    // A different deployer-level user (passes RBAC) must not see admin's builds.
    let other = create_user(&server, admin_id, "other-deployer").await;
    let other_id = other["id"].as_str().unwrap();

    let status = try_list_builds(&server, other_id, false, "team_member", agent_id).await;
    assert_eq!(status, 403, "non-owner must not see another agent's build history");

    server.cleanup().await;
}

/// The agent owner can see their own build history.
#[tokio::test]
#[serial]
async fn owner_can_list_builds_for_own_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, admin_id).await;
    let agent_id = agent["id"].as_str().unwrap();

    // Admin is superuser — should always see own agent builds.
    let status = try_list_builds(&server, admin_id, true, "admin", agent_id).await;
    assert_eq!(status, 200, "owner (superuser) must see own agent builds");

    server.cleanup().await;
}

/// Superuser sees build history for any agent regardless of ownership.
#[tokio::test]
#[serial]
async fn superuser_sees_all_agent_builds() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, admin_id).await;
    let agent_id = agent["id"].as_str().unwrap();

    // A separate superuser identity.
    let status = try_list_builds(&server, admin_id, true, "admin", agent_id).await;
    assert_eq!(status, 200);

    server.cleanup().await;
}
