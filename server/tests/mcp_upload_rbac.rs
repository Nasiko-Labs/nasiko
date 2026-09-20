//! RBAC integration tests for the MCP-server-upload MUTATION routes (Step 11
//! of docs/MCP_UPLOAD_ITERATION_PLAN.md).
//!
//! Verifies that `require_deployer` is enforced on:
//!   POST /api/mcp/connectors/upload
//!   POST /api/mcp/connectors/upload-github
//! and that the read-only build-status/build-logs routes stay at plain
//! `require_auth` (ownership-checked inside the handler, not role-gated).
//!
//! Mirrors oss/server/tests/build_rbac.rs's own pattern/assertions exactly —
//! OSS's AuthServiceImpl is allow-all, so this proves the middleware is wired
//! (never a blanket 403 for any authenticated identity), not that OSS itself
//! enforces a role hierarchy (that's EE's job, unit-tested in `nasiko-auth-ee`).
//!
//!   cargo test -p nasiko-server --test mcp_upload_rbac -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

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
    common::as_superuser(server.client.post(server.url("/api/users")), admin_id, "admin")
        .json(&json!({"username": username, "email": format!("{username}@test.local")}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

fn minimal_zip() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zw = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default();
        use std::io::Write;
        zw.start_file("Dockerfile", opts).unwrap();
        zw.write_all(b"FROM python:3.12-slim\n").unwrap();
        zw.finish().unwrap();
    }
    buf
}

/// POST /api/mcp/connectors/upload with the given identity; RBAC check only.
async fn try_upload_zip(server: &common::TestServer, user_id: &str, is_superuser: bool, role: &str) -> u16 {
    let form = reqwest::multipart::Form::new()
        .text("name", format!("rbac-test-{}", Uuid::new_v4().simple()))
        .part("source", reqwest::multipart::Part::bytes(minimal_zip()).file_name("upload.zip"));
    let token = common::sign_token(user_id, "u", is_superuser, role);
    server.client.post(server.url("/api/mcp/connectors/upload")).bearer_auth(token).multipart(form).send().await.unwrap().status().as_u16()
}

/// POST /api/mcp/connectors/upload-github with the given identity; RBAC check only.
async fn try_upload_github(server: &common::TestServer, user_id: &str, is_superuser: bool, role: &str) -> u16 {
    let token = common::sign_token(user_id, "u", is_superuser, role);
    server
        .client
        .post(server.url("/api/mcp/connectors/upload-github"))
        .bearer_auth(token)
        .json(&json!({
            "name": format!("rbac-gh-{}", Uuid::new_v4().simple()),
            "version_tag": "v1",
            "github_url": "https://evil.example.com/repo.git",
        }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

// ─── Tests: POST /api/mcp/connectors/upload ────────────────────────────────

/// OSS has no role-based RBAC — the `AuthService` grants every permission, so
/// any authenticated identity reaches the upload handler (never a 403 role gate).
#[tokio::test]
#[serial]
async fn oss_any_identity_reaches_upload_zip_handler() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let user = create_user(&server, admin_id, "user-mcp-upload").await;
    let user_id = user["id"].as_str().unwrap();

    let status = try_upload_zip(&server, user_id, false, "member").await;
    assert_ne!(status, 403, "OSS allow-all authorizer must not block any identity with a 403");

    server.cleanup().await;
}

/// team_member is the deployer threshold in EE — request passes RBAC (OSS: always).
#[tokio::test]
#[serial]
async fn team_member_can_reach_upload_zip_handler() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let deployer = create_user(&server, admin_id, "deployer-mcp-upload").await;
    let deployer_id = deployer["id"].as_str().unwrap();

    let status = try_upload_zip(&server, deployer_id, false, "team_member").await;
    assert_ne!(status, 403, "team_member must pass RBAC gate for MCP upload");

    server.cleanup().await;
}

/// Superuser always bypasses RBAC.
#[tokio::test]
#[serial]
async fn superuser_can_upload_zip() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let status = try_upload_zip(&server, admin_id, true, "admin").await;
    assert_ne!(status, 403, "superuser must not be blocked");

    server.cleanup().await;
}

// ─── Tests: POST /api/mcp/connectors/upload-github ─────────────────────────

#[tokio::test]
#[serial]
async fn oss_any_identity_reaches_upload_github_handler() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let user = create_user(&server, admin_id, "user-mcp-gh").await;
    let user_id = user["id"].as_str().unwrap();

    // Disallowed host means a 400 validation rejection, not 403 — the RBAC
    // gate runs first regardless; the point of this test is that it's never 403.
    let status = try_upload_github(&server, user_id, false, "member").await;
    assert_ne!(status, 403, "OSS allow-all authorizer must not block any identity with a 403");

    server.cleanup().await;
}

// ─── Tests: build-status / build-logs stay at plain require_auth ───────────

/// Both read routes must remain reachable by plain member-level auth (not
/// role-gated) — ownership is checked inside the handler, per Step 10.
#[tokio::test]
#[serial]
async fn member_can_reach_build_status_and_logs_routes_not_role_gated() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "rbac-alice").await;
    let alice_id = alice["id"].as_str().unwrap();

    let form = reqwest::multipart::Form::new()
        .text("name", format!("rbac-owned-{}", Uuid::new_v4().simple()))
        .part("source", reqwest::multipart::Part::bytes(minimal_zip()).file_name("upload.zip"));
    let upload_res = common::as_member(server.client.post(server.url("/api/mcp/connectors/upload")), alice_id, "rbac-alice")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(upload_res.status(), 202);
    let body: Value = upload_res.json().await.unwrap();
    let connector_id = body["data"]["connector_id"].as_str().unwrap();

    // The owner, a plain (non-deployer-role) member, must reach both GET
    // routes — proves they were never wrapped in the require_deployer layer.
    let status_res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/connectors/{connector_id}/build-status"))),
        alice_id,
        "rbac-alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(status_res.status(), 200);

    let logs_res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/connectors/{connector_id}/build-logs"))),
        alice_id,
        "rbac-alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(logs_res.status(), 200);

    server.cleanup().await;
}
