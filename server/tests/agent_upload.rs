//! Tests for `POST /api/agents/upload-and-deploy` and `GET /api/agents/deploy-status/{id}`
//! (the `agents` module DB persistence, P1).
//!
//! These exercise the synchronous persistence (agent upsert + build record) and the
//! async status transitions **without a real Docker build**: the uploaded zips contain
//! no `Dockerfile`, so `execute_upload_and_deploy` fails fast at the Dockerfile check
//! and never invokes the runtime — making the terminal `failed` state deterministic.
//!
//! Requires infra (Postgres :5432, Redis, S3, Docker) like the rest of the suite:
//!   `docker compose --profile infra up -d`
//!   `cargo test -p nasiko-server --test agent_upload -- --test-threads=1`

mod common;

use std::time::Duration;

use serde_json::{Value, json};
use serial_test::serial;

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

/// POST a multipart upload-and-deploy request as the given superuser.
async fn upload(
    server: &common::TestServer,
    user_id: &str,
    fields: Vec<(&'static str, String)>,
    source: Option<Vec<u8>>,
) -> reqwest::Response {
    let mut form = reqwest::multipart::Form::new();
    for (k, v) in fields {
        form = form.text(k, v);
    }
    if let Some(zip) = source {
        form = form.part("source", reqwest::multipart::Part::bytes(zip).file_name("agent.zip"));
    }
    server
        .client
        .post(server.url("/api/agents/upload-and-deploy"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .multipart(form)
        .send()
        .await
        .unwrap()
}

async fn get_build(server: &common::TestServer, user_id: &str, build_id: &str) -> reqwest::Response {
    server
        .client
        .get(server.url(&format!("/api/build/builds/{build_id}")))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap()
}

#[allow(dead_code)]
/// Poll the build record until its status is terminal, or panic on timeout.
async fn wait_for_terminal_status(
    server: &common::TestServer,
    user_id: &str,
    build_id: &str,
) -> String {
    for _ in 0..40 {
        let res = get_build(server, user_id, build_id).await;
        if res.status() == 200 {
            let body: Value = res.json().await.unwrap();
            if let Some(s) = body["status"].as_str()
                && (s == "success" || s == "failed")
            {
                return s.to_string();
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("build {build_id} did not reach a terminal status in time");
}

const NO_DOCKERFILE_ZIP_ENTRY: (&str, &[u8]) = ("README.md", b"no dockerfile here");

/// A zip that passes the upload endpoint's structural validation (Dockerfile + main.py) but
/// will fail deterministically at the Docker build step in the test environment — no real
/// image build is triggered by the validation layer, so status transitions are still testable.
fn make_valid_structure_zip() -> Vec<u8> {
    common::make_zip(&[
        ("Dockerfile", b"FROM python:3.11-slim\nCMD [\"python\", \"main.py\"]"),
        ("main.py", b"print('hello')"),
    ])
}

// ─── validation ──────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn upload_requires_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let zip = common::make_zip(&[NO_DOCKERFILE_ZIP_ENTRY]);
    let res = upload(&server, uid, vec![("version_tag", "v1".into())], Some(zip)).await;

    assert_eq!(res.status(), 400);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_requires_source() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = upload(&server, uid, vec![("name", "demo".into())], None).await;

    assert_eq!(res.status(), 400);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_without_identity_returns_401() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    // No x-user-id header → require_auth rejects before the handler.
    let zip = common::make_zip(&[NO_DOCKERFILE_ZIP_ENTRY]);
    let form = reqwest::multipart::Form::new()
        .text("name", "demo")
        .text("version_tag", "v1")
        .part("source", reqwest::multipart::Part::bytes(zip).file_name("agent.zip"));
    let res = server
        .client
        .post(server.url("/api/agents/upload-and-deploy"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    server.cleanup().await;
}

// ─── persistence ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn upload_persists_agent_and_build_record() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let zip = make_valid_structure_zip();
    let res = upload(
        &server,
        uid,
        vec![("name", "demo-agent".into()), ("version_tag", "v1".into())],
        Some(zip),
    )
    .await;

    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    let agent_id = body["agent_id"].as_str().unwrap();
    let build_id = body["build_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(agent_id).is_ok());
    assert!(uuid::Uuid::parse_str(build_id).is_ok());
    assert_eq!(body["status"], "queued");

    // Agent persisted in the catalog.
    let agents: Value = server
        .client
        .get(server.url("/api/catalog/agents"))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = agents
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert!(names.contains(&"demo-agent"), "catalog should list the agent: {names:?}");

    // Build record persisted and its enum status decodes through build/routes.rs.
    let build_res = get_build(&server, uid, build_id).await;
    assert_eq!(build_res.status(), 200);
    let build: Value = build_res.json().await.unwrap();
    assert_eq!(build["agent_id"].as_str().unwrap(), agent_id);
    assert!(build["status"].is_string());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_reuses_agent_on_repeat_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let first: Value = upload(
        &server,
        uid,
        vec![("name", "repeat".into()), ("version_tag", "v1".into())],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();

    let second: Value = upload(
        &server,
        uid,
        vec![("name", "repeat".into()), ("version_tag", "v2".into())],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();

    // Same agent reused (owner-scoped upsert), distinct build records.
    assert_eq!(first["agent_id"], second["agent_id"]);
    assert_ne!(first["build_id"], second["build_id"]);

    server.cleanup().await;
}

// ─── status transitions ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn upload_marks_build_failed_without_dockerfile() {
    // Validation now happens synchronously in the upload handler before a build record
    // is created, so a missing Dockerfile yields an immediate 400 rather than a queued
    // job that later transitions to "failed".
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = upload(
        &server,
        uid,
        vec![("name", "nodockerfile".into()), ("version_tag", "v1".into())],
        Some(common::make_zip(&[NO_DOCKERFILE_ZIP_ENTRY])),
    )
    .await;

    assert_eq!(res.status(), 400, "missing Dockerfile must be rejected immediately");
    let body = res.text().await.unwrap();
    assert!(
        body.to_lowercase().contains("dockerfile"),
        "error message should mention Dockerfile: {body}"
    );

    server.cleanup().await;
}

// ─── deploy-status SSE ───────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn deploy_status_unknown_build_reports_not_found() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let random = uuid::Uuid::new_v4();
    let res = server
        .client
        .get(server.url(&format!("/api/agents/deploy-status/{random}")))
        .header("x-user-id", uid)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    // Unknown build → the stream emits a single not_found event and closes.
    let body = res.text().await.unwrap();
    assert!(body.contains("not_found"), "expected not_found event, got: {body}");

    server.cleanup().await;
}
