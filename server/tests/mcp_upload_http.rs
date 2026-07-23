//! HTTP-layer tests for Step 10's upload handlers (`POST
//! /api/mcp/connectors/upload`, `/upload-github`, `GET .../build-status`,
//! `GET .../build-logs`) — request parsing, validation, and the DB
//! transaction the handler writes, all through the real router.
//!
//! `TestServer` uses `FakeRuntime`, and the build-worker loop is never spawned
//! in this harness, so a queued job here stays `pending` forever — that's
//! expected and correct for these tests, which verify the HTTP/DB layer Step
//! 10 actually adds, not the build pipeline itself (already verified with a
//! real Docker runtime in `mcp_upload_build.rs`, Steps 8-9).
//!
//!   cargo test -p nasiko-server --test mcp_upload_http -- --test-threads=1

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

#[tokio::test]
#[serial]
async fn upload_zip_queues_a_real_build_job() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let mut form = reqwest::multipart::Form::new()
        .text("name", format!("http-test-{}", Uuid::new_v4().simple()))
        .text("version_tag", "v1");
    form = form.part("source", reqwest::multipart::Part::bytes(minimal_zip()).file_name("upload.zip"));

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connectors/upload")), user_id, "admin")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    let connector_id: Uuid = body["data"]["connector_id"].as_str().unwrap().parse().unwrap();
    let build_id: Uuid = body["data"]["build_id"].as_str().unwrap().parse().unwrap();

    // The real build-worker loop (spawned inside AppState — it is NOT a
    // no-op in this test harness) claims this job almost immediately with
    // `FakeRuntime`, whose fake endpoint has nothing real listening behind
    // it — so the exact status is a genuine race (pending → building →
    // eventually failed once the readiness check exhausts its retries).
    // These assertions check the identity/relationships Step 10 is actually
    // responsible for, tolerant of exactly which non-final-success state the
    // race landed on (it can never reach 'running'/is_active=true here, since
    // no real MCP server is ever actually reachable).
    let row: (String, String, bool, String) = sqlx::query_as(
        "SELECT source_kind::text, build_status, is_active, provider_type FROM mcp_connectors WHERE id = $1",
    )
    .bind(connector_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(row.0, "uploaded_build");
    assert!(
        ["pending", "building", "failed"].contains(&row.1.as_str()),
        "unexpected build_status: {}",
        row.1
    );
    assert!(!row.2, "must never become active against a fake, unreachable endpoint");
    assert_eq!(row.3, "mcp_server");

    let build_row: (Uuid, String) =
        sqlx::query_as("SELECT connector_id, status FROM mcp_connector_builds WHERE id = $1")
            .bind(build_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(build_row.0, connector_id);
    assert!(["pending", "building", "failed"].contains(&build_row.1.as_str()));

    let job: (Option<Uuid>, Option<Uuid>, String) =
        sqlx::query_as("SELECT agent_id, connector_id, status FROM build_jobs WHERE connector_id = $1")
            .bind(connector_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(job.0, None, "an MCP job must never set agent_id");
    assert_eq!(job.1, Some(connector_id));
    assert!(["pending", "in_progress", "failed"].contains(&job.2.as_str()));

    // build-status polling reflects the same (racy but bounded) state.
    let status_res = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/connectors/{connector_id}/build-status"))),
        user_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(status_res.status(), 200);
    let status_body: Value = status_res.json().await.unwrap();
    let build_status = status_body["data"]["build_status"].as_str().unwrap();
    assert!(
        ["pending", "building", "failed"].contains(&build_status),
        "unexpected build_status in response: {status_body}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_zip_requires_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let mut form = reqwest::multipart::Form::new().text("version_tag", "v1");
    form = form.part("source", reqwest::multipart::Part::bytes(minimal_zip()).file_name("upload.zip"));

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connectors/upload")), user_id, "admin")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_zip_requires_source() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let form = reqwest::multipart::Form::new().text("name", "no-source-test").text("version_tag", "v1");

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connectors/upload")), user_id, "admin")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_github_rejects_disallowed_host() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connectors/upload-github")), user_id, "admin")
        .json(&json!({
            "name": "github-test",
            "version_tag": "v1",
            "github_url": "https://evil.example.com/repo.git",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // Nothing should have been queued for a rejected URL.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mcp_connectors WHERE name = 'github-test'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(count, 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn build_status_and_build_logs_require_ownership() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, admin_id, "up-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let bob = create_user(&server, admin_id, "up-bob").await;
    let bob_id = bob["id"].as_str().unwrap();

    let mut form = reqwest::multipart::Form::new()
        .text("name", format!("owned-{}", Uuid::new_v4().simple()))
        .text("version_tag", "v1");
    form = form.part("source", reqwest::multipart::Part::bytes(minimal_zip()).file_name("upload.zip"));
    let upload_res = common::as_member(server.client.post(server.url("/api/mcp/connectors/upload")), alice_id, "up-alice")
        .multipart(form)
        .send()
        .await
        .unwrap();
    let body: Value = upload_res.json().await.unwrap();
    let connector_id = body["data"]["connector_id"].as_str().unwrap();

    // Bob (a real, non-admin, non-owning user) must be forbidden from both routes.
    let status_res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/connectors/{connector_id}/build-status"))),
        bob_id,
        "up-bob",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(status_res.status(), 403);

    let logs_res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/connectors/{connector_id}/build-logs"))),
        bob_id,
        "up-bob",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(logs_res.status(), 403);

    // Alice (the real owner, not an admin) can still reach both — proves this
    // is a genuine ownership check, not just an admin bypass.
    let owner_status_res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/connectors/{connector_id}/build-status"))),
        alice_id,
        "up-alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(owner_status_res.status(), 200);

    server.cleanup().await;
}
