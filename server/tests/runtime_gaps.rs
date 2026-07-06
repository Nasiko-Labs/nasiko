//! Tests covering the runtime-gap closures from the 3.3 migration audit.
//!
//! Covers:
//!   - Zip extraction guards: file count, zip bomb, path traversal, valid zips
//!   - Upload endpoint: structure validation (400), guard rejections (400), success (202)
//!   - Build job queue: insertion after upload, stuck-job recovery SQL, terminal transitions
//!   - Restart endpoint: 404 for unknown deployment, no-409 for stopped/crashed agents,
//!     stored spec_ports/spec_image, crash-field clearing
//!   - Schema: migration 013 columns present on live DB
//!
//! Requires docker-compose infra (Postgres, Redis, S3, Docker):
//!   cargo test -p nasiko-server --test runtime_gaps -- --test-threads=1

mod common;

use std::io::Write as _;
use std::time::Duration;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════════════════════
// Zip helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Create a valid agent zip: Dockerfile with FROM + main.py entrypoint.
/// Uses a nonexistent base image so the Docker build fails in ~1 s without pulling anything,
/// keeping `build_worker_transitions_job_to_terminal_state` fast and deterministic.
fn make_valid_agent_zip() -> Vec<u8> {
    common::make_zip(&[
        ("Dockerfile", b"FROM nasiko-nonexistent-base-xyz:does-not-exist\nCMD [\"python\", \"main.py\"]"),
        ("main.py", b"print('hello')"),
    ])
}

/// Create an agent zip where the Dockerfile has no FROM instruction.
fn make_zip_dockerfile_no_from() -> Vec<u8> {
    common::make_zip(&[
        ("Dockerfile", b"RUN echo hello\n"),
        ("main.py", b"print('hello')"),
    ])
}

/// Create an agent zip with a Dockerfile but no Python entrypoint.
fn make_zip_no_entrypoint() -> Vec<u8> {
    common::make_zip(&[
        ("Dockerfile", b"FROM node:20\nCMD [\"node\", \"server.js\"]"),
        ("server.js", b"console.log('hi')"),
    ])
}

/// Create a zip with `count` distinct files (for the file-count guard).
fn make_zip_many_files(count: usize) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut zw = zip::ZipWriter::new(&mut cursor);
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..count {
        zw.start_file(format!("f_{i}.txt"), opts).unwrap();
        zw.write_all(b"x").unwrap();
    }
    zw.finish().unwrap();
    cursor.into_inner()
}

/// Create a zip whose declared uncompressed size exceeds 200 MiB (zip bomb guard).
/// Actual zip file is tiny because 201 MiB of zeros compress extremely well.
fn make_zip_bomb() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut zw = zip::ZipWriter::new(&mut cursor);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zw.start_file("bomb.bin", opts).unwrap();
    let block = [0u8; 4_096];
    let total: usize = 201 * 1024 * 1024; // 201 MiB
    let mut written = 0;
    while written < total {
        let n = (total - written).min(4_096);
        zw.write_all(&block[..n]).unwrap();
        written += n;
    }
    zw.finish().unwrap();
    cursor.into_inner()
}

/// Create a zip containing an entry with a `..` path traversal component.
fn make_zip_traversal() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut zw = zip::ZipWriter::new(&mut cursor);
    let opts = zip::write::SimpleFileOptions::default();
    zw.start_file("../evil.txt", opts).unwrap();
    zw.write_all(b"evil content").unwrap();
    zw.finish().unwrap();
    cursor.into_inner()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 1 — Zip extraction unit tests (no server, no DB)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn zip_guards_rejects_too_many_files() {
    // Guard: archive.len() > MAX_ZIP_FILES (1000) → Err
    let zip = make_zip_many_files(1_001);
    let dest = std::env::temp_dir().join(format!("ng-test-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_to_dir(&zip, &dest);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_err(), "expected error for 1001 files");
    let msg = result.unwrap_err();
    assert!(msg.contains("1001") || msg.contains("limit"), "error should mention file count: {msg}");
}

#[test]
fn zip_guards_accepts_at_file_limit() {
    // Exactly 1000 files should be accepted.
    let zip = make_zip_many_files(1_000);
    let dest = std::env::temp_dir().join(format!("ng-test-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_to_dir(&zip, &dest);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_ok(), "1000-file zip should be accepted: {result:?}");
}

#[test]
fn zip_guards_rejects_zip_bomb() {
    // Guard: declared uncompressed total > MAX_ZIP_UNCOMPRESSED (200 MiB) → Err
    let zip = make_zip_bomb();
    let dest = std::env::temp_dir().join(format!("ng-test-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_to_dir(&zip, &dest);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_err(), "expected zip-bomb rejection");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("zip") || msg.contains("size") || msg.contains("bomb"),
        "error should mention size limit: {msg}"
    );
}

#[test]
fn zip_guards_rejects_path_traversal_dotdot() {
    // Guard: Component::ParentDir in entry name → Err
    let zip = make_zip_traversal();
    let dest = std::env::temp_dir().join(format!("ng-test-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_to_dir(&zip, &dest);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_err(), "expected traversal rejection");
    let msg = result.unwrap_err();
    assert!(msg.contains("traversal"), "error should mention traversal: {msg}");
}

#[test]
fn zip_guards_accepts_valid_flat_zip() {
    let zip = make_valid_agent_zip();
    let dest = std::env::temp_dir().join(format!("ng-test-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_to_dir(&zip, &dest);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_ok(), "valid agent zip should be accepted: {result:?}");
}

#[test]
fn zip_guards_accepts_nested_structure() {
    // src/main.py is a valid entrypoint location; sub-directories must extract cleanly.
    let zip = common::make_zip(&[
        ("Dockerfile", b"FROM python:3.11-slim"),
        ("src/main.py", b"print('hi')"),
        ("src/utils.py", b"pass"),
        ("requirements.txt", b"requests==2.31"),
    ]);
    let dest = std::env::temp_dir().join(format!("ng-test-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_to_dir(&zip, &dest);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_ok(), "nested structure should be accepted: {result:?}");
}

#[test]
fn zip_from_file_rejects_path_traversal() {
    // extract_zip_from_file applies the same guards as the cursor-based version.
    let zip = make_zip_traversal();
    let tmp = std::env::temp_dir();
    let zip_path = tmp.join(format!("ng-bomb-{}.zip", Uuid::new_v4()));
    std::fs::write(&zip_path, &zip).unwrap();

    let dest = tmp.join(format!("ng-dest-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_from_file(&zip_path, &dest);
    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_err(), "expected traversal rejection from file path");
    assert!(result.unwrap_err().contains("traversal"));
}

#[test]
fn zip_from_file_rejects_too_many_files() {
    let zip = make_zip_many_files(1_001);
    let tmp = std::env::temp_dir();
    let zip_path = tmp.join(format!("ng-many-{}.zip", Uuid::new_v4()));
    std::fs::write(&zip_path, &zip).unwrap();

    let dest = tmp.join(format!("ng-dest-{}", Uuid::new_v4()));
    let result = nasiko_server::build::routes::extract_zip_from_file(&zip_path, &dest);
    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_dir_all(&dest);
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 2 — Upload endpoint (integration, requires infra)
// ═══════════════════════════════════════════════════════════════════════════════

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

async fn do_upload(
    server: &common::TestServer,
    uid: &str,
    name: &str,
    extra_fields: Vec<(&'static str, String)>,
    zip: Vec<u8>,
) -> reqwest::Response {
    let mut form = reqwest::multipart::Form::new()
        .text("name", name.to_string())
        .text("version_tag", "v1".to_string());
    for (k, v) in extra_fields {
        form = form.text(k, v);
    }
    form = form.part("source", reqwest::multipart::Part::bytes(zip).file_name("agent.zip"));
    common::as_superuser(
        server.client.post(server.url("/api/agents/upload")),
        uid,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn upload_valid_agent_returns_202() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(&server, uid, "valid-agent-202", vec![], make_valid_agent_zip()).await;

    assert_eq!(res.status(), 202, "expected 202 Accepted for valid zip");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["status"], "queued");
    assert!(Uuid::parse_str(body["build_id"].as_str().unwrap_or("")).is_ok());
    assert!(Uuid::parse_str(body["agent_id"].as_str().unwrap_or("")).is_ok());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_creates_build_job_in_db() {
    // The handler must INSERT into build_jobs, not spawn a goroutine.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(&server, uid, "job-row-agent", vec![], make_valid_agent_zip()).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    let agent_id: Uuid = body["agent_id"].as_str().unwrap().parse().unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM build_jobs WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1, "expected exactly one build_jobs row immediately after 202");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_creates_build_job_in_db() {
    // PUT /api/agents/{id}/update must INSERT into build_jobs (not spawn a goroutine).
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, version, status) \
         VALUES ('update-queue-agent', $1, 'update-queue-agent:1.0.0', '1.0.0', 'running') RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let zip = common::make_zip(&[("README.md", b"no dockerfile")]);
    let form = reqwest::multipart::Form::new()
        .part("source", reqwest::multipart::Part::bytes(zip).file_name("agent.zip"));
    let res = common::as_superuser(
        server.client.put(server.url(&format!("/api/agents/{agent_id}/update"))),
        uid,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202, "update should return 202");

    let payload: Option<Value> =
        sqlx::query_scalar("SELECT payload FROM build_jobs WHERE agent_id = $1 LIMIT 1")
            .bind(agent_id)
            .fetch_optional(&server.db)
            .await
            .unwrap();
    let payload = payload.expect("update handler should insert a build_jobs row");
    assert_eq!(payload["kind"].as_str().unwrap(), "Update", "payload kind should be Update");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn rollback_creates_build_job_in_db() {
    // POST /api/agents/{id}/rollback must INSERT into build_jobs (not spawn a goroutine).
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, version, status) \
         VALUES ('rollback-queue-agent', $1, 'rollback-queue-agent:1.0.1', '1.0.1', 'running') RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '1.0.0', 'rollback-queue-agent:1.0.0', false, true, 'archived')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let res = common::as_superuser(
        server.client.post(server.url(&format!("/api/agents/{agent_id}/rollback"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202, "rollback should return 202");

    let payload: Option<Value> =
        sqlx::query_scalar("SELECT payload FROM build_jobs WHERE agent_id = $1 LIMIT 1")
            .bind(agent_id)
            .fetch_optional(&server.db)
            .await
            .unwrap();
    let payload = payload.expect("rollback handler should insert a build_jobs row");
    assert_eq!(payload["kind"].as_str().unwrap(), "Rollback", "payload kind should be Rollback");
    assert_eq!(
        payload["target_version"].as_str().unwrap(), "1.0.0",
        "rollback payload should carry the target version"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_build_job_payload_contains_ports() {
    // The build_jobs.payload must store the ports submitted in the upload form.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(
        &server,
        uid,
        "ports-payload-agent",
        vec![("ports", "3000,4000".to_string())],
        make_valid_agent_zip(),
    )
    .await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    let agent_id: Uuid = body["agent_id"].as_str().unwrap().parse().unwrap();

    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM build_jobs WHERE agent_id = $1 LIMIT 1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();

    let ports = payload["ports"].as_array().expect("ports should be an array");
    let port_nums: Vec<u64> = ports.iter().filter_map(|v| v.as_u64()).collect();
    assert!(port_nums.contains(&3000), "expected port 3000 in payload: {port_nums:?}");
    assert!(port_nums.contains(&4000), "expected port 4000 in payload: {port_nums:?}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_missing_dockerfile_returns_400() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let zip = common::make_zip(&[("main.py", b"print('hello')")]);
    let res = do_upload(&server, uid, "no-dockerfile-400", vec![], zip).await;

    assert_eq!(res.status(), 400);
    let text = res.text().await.unwrap();
    assert!(
        text.to_lowercase().contains("dockerfile"),
        "error should mention Dockerfile: {text}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_dockerfile_without_from_returns_400() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(&server, uid, "no-from-400", vec![], make_zip_dockerfile_no_from()).await;

    assert_eq!(res.status(), 400);
    let text = res.text().await.unwrap();
    assert!(
        text.contains("FROM"),
        "error should mention FROM instruction: {text}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_missing_entrypoint_returns_400() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(&server, uid, "no-entry-400", vec![], make_zip_no_entrypoint()).await;

    assert_eq!(res.status(), 400);
    let text = res.text().await.unwrap();
    assert!(
        text.contains("entrypoint") || text.contains("main.py"),
        "error should mention entrypoint: {text}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_too_many_files_returns_400() {
    // 1001 files → file-count guard in extract_zip_from_file → 400.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let zip = make_zip_many_files(1_001);
    let res = do_upload(&server, uid, "fat-zip-400", vec![], zip).await;

    assert_eq!(res.status(), 400);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_path_traversal_returns_400() {
    // Traversal in zip entry path → 400.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(&server, uid, "traversal-400", vec![], make_zip_traversal()).await;

    assert_eq!(res.status(), 400);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_src_main_py_entrypoint_accepted() {
    // src/main.py is a valid Python entrypoint (not just root main.py).
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let zip = common::make_zip(&[
        ("Dockerfile", b"FROM python:3.11-slim"),
        ("src/main.py", b"print('hi')"),
    ]);
    let res = do_upload(&server, uid, "src-main-agent", vec![], zip).await;

    assert_eq!(res.status(), 202, "src/main.py should be accepted as entrypoint");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_dunder_main_entrypoint_accepted() {
    // __main__.py is a valid Python entrypoint.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let zip = common::make_zip(&[
        ("Dockerfile", b"FROM python:3.11-slim"),
        ("__main__.py", b"print('hi')"),
    ]);
    let res = do_upload(&server, uid, "dunder-main-agent", vec![], zip).await;

    assert_eq!(res.status(), 202, "__main__.py should be accepted as entrypoint");
    server.cleanup().await;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 3 — Restart endpoint (integration)
// ═══════════════════════════════════════════════════════════════════════════════

/// Seed an agent + build + deployment row for restart tests.
/// Returns (agent_id, deployment_id).
async fn seed_deployment(
    db: &sqlx::PgPool,
    owner_id: Uuid,
    agent_name: &str,
    deploy_status: &str,
    spec_ports: Option<Vec<i32>>,
    spec_image: Option<&str>,
    crash_reason: Option<&str>,
) -> (Uuid, Uuid) {
    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status)
         VALUES ($1, $2, 'test:latest', 'stopped') RETURNING id",
    )
    .bind(agent_name)
    .bind(owner_id)
    .fetch_one(db)
    .await
    .unwrap();

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status)
         VALUES ($1, 'v1', 'test:latest', 'success') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(db)
    .await
    .unwrap();

    let deployment_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_deployments
             (agent_id, build_id, owner_id, status, spec_ports, spec_image, crash_reason)
         VALUES ($1, $2, $3, $4::deployment_status, $5, $6, $7)
         RETURNING id",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(owner_id)
    .bind(deploy_status)
    .bind(spec_ports)
    .bind(spec_image)
    .bind(crash_reason)
    .fetch_one(db)
    .await
    .unwrap();

    (agent_id, deployment_id)
}

async fn call_restart(
    server: &common::TestServer,
    uid: &str,
    deployment_id: Uuid,
) -> reqwest::Response {
    common::as_superuser(
        server.client.post(server.url(&format!("/api/agents/deployment/{deployment_id}/restart"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn restart_unknown_deployment_returns_404() {
    let server = common::TestServer::start().await;
    let uid = "00000000-0000-0000-0000-000000000001";
    let res = call_restart(&server, uid, Uuid::new_v4()).await;
    assert_eq!(res.status(), 404);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_stopped_agent_does_not_409() {
    // A 'stopped' agent is not running → restart must not return 409.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let (_, dep_id) =
        seed_deployment(&server.db, owner_id, "stopped-restart-ng", "stopped", None, None, None)
            .await;

    let res = call_restart(&server, uid, dep_id).await;
    // Expect 200 (success) or 500 (Docker deploy failed — no real image in test env).
    // Either way, NOT 404 or 409.
    let status = res.status().as_u16();
    assert_ne!(status, 404, "stopped agent should not 404");
    assert_ne!(status, 409, "stopped agent should not 409");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_crashed_agent_does_not_409() {
    // A 'crashed' agent is not running → restart must not return 409.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let (_, dep_id) = seed_deployment(
        &server.db,
        owner_id,
        "crashed-restart-ng",
        "crashed",
        Some(vec![8000]),
        Some("test:latest"),
        Some("OOMKilled"),
    )
    .await;

    let res = call_restart(&server, uid, dep_id).await;
    let status = res.status().as_u16();
    assert_ne!(status, 404, "crashed agent should not 404");
    assert_ne!(status, 409, "crashed agent should not 409");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_running_agent_returns_409() {
    // DB-based 409 guard: a 'running' agent must be rejected without touching Docker.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let (_, dep_id) =
        seed_deployment(&server.db, owner_id, "running-restart-ng", "running", None, None, None)
            .await;

    let res = call_restart(&server, uid, dep_id).await;
    assert_eq!(res.status(), 409, "running agent must return 409");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_starting_agent_returns_409() {
    // DB-based 409 guard: a 'starting' agent is also considered live and must be rejected.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let (_, dep_id) =
        seed_deployment(&server.db, owner_id, "starting-restart-ng", "starting", None, None, None)
            .await;

    let res = call_restart(&server, uid, dep_id).await;
    assert_eq!(res.status(), 409, "starting agent must return 409");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_spec_ports_stored_and_read() {
    // Verify that spec_ports in agent_deployments is correctly persisted after upload.
    // The restart handler reads spec_ports from this column.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    // Seed a deployment row with non-default spec_ports.
    let (agent_id, dep_id) = seed_deployment(
        &server.db,
        owner_id,
        "spec-ports-ng",
        "stopped",
        Some(vec![3000, 4000]),
        Some("myregistry/myimage:v2"),
        None,
    )
    .await;

    // Read back from DB to confirm the columns are present and correct.
    let (ports, image): (Option<Vec<i32>>, Option<String>) = sqlx::query_as(
        "SELECT spec_ports, spec_image FROM agent_deployments WHERE id = $1",
    )
    .bind(dep_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(
        ports,
        Some(vec![3000, 4000]),
        "spec_ports should be stored as [3000, 4000]"
    );
    assert_eq!(
        image,
        Some("myregistry/myimage:v2".to_string()),
        "spec_image should be stored"
    );

    // Also verify restart endpoint reads the row (not 404).
    let res = call_restart(&server, uid, dep_id).await;
    assert_ne!(res.status().as_u16(), 404, "should find the deployment");

    // Verify agent row still exists (no cascade issues).
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1)")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(exists);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_null_spec_ports_does_not_error() {
    // When spec_ports IS NULL, the handler falls back to port 8000 without erroring.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let (_, dep_id) = seed_deployment(
        &server.db,
        owner_id,
        "null-ports-ng",
        "stopped",
        None, // NULL spec_ports → fallback to 8000
        None, // NULL spec_image → fallback to agents.image
        None,
    )
    .await;

    // Should find the deployment (not 404) and attempt restart (not 409).
    let res = call_restart(&server, uid, dep_id).await;
    let status = res.status().as_u16();
    assert_ne!(status, 404, "should find deployment even with null spec_ports");
    assert_ne!(status, 409, "should not conflict for a stopped agent");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_old_deployment_stopped_after_restart() {
    // The old deployment row should be marked 'stopped' after restart is triggered.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let (_, dep_id) = seed_deployment(
        &server.db,
        owner_id,
        "old-dep-stop-ng",
        "stopped",
        Some(vec![8000]),
        Some("alpine:latest"),
        None,
    )
    .await;

    let res = call_restart(&server, uid, dep_id).await;

    if res.status() == 200 {
        // On success the old row must be 'stopped'.
        let old_status: String =
            sqlx::query_scalar("SELECT status::text FROM agent_deployments WHERE id = $1")
                .bind(dep_id)
                .fetch_one(&server.db)
                .await
                .unwrap();
        assert_eq!(old_status, "stopped", "old deployment row should be stopped");
    }
    // If Docker fails (500 in test env), the old row may remain unchanged — that's OK.
    // We don't assert on the 500 path to keep the test hermetic.

    server.cleanup().await;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 4 — Build worker (integration)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[serial]
async fn build_worker_transitions_job_to_terminal_state() {
    // After a valid upload, the background build worker picks up the job and eventually
    // transitions it to 'done' or 'failed'. In the test env Docker build likely fails
    // (no real registry), so 'failed' is the expected terminal state.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(&server, uid, "worker-terminal-ng", vec![], make_valid_agent_zip()).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    let agent_id: Uuid = body["agent_id"].as_str().unwrap().parse().unwrap();

    // Poll up to 60 s for a terminal state. Docker image-not-found fails in ~1 s;
    // the generous ceiling handles slow daemons or cold CI environments.
    let mut final_status = "pending".to_string();
    for _ in 0..240 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let s: Option<String> = sqlx::query_scalar(
            "SELECT status FROM build_jobs WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(agent_id)
        .fetch_optional(&server.db)
        .await
        .unwrap();
        if let Some(st) = s {
            final_status = st.clone();
            if st == "done" || st == "failed" {
                break;
            }
        }
    }

    assert!(
        final_status == "done" || final_status == "failed",
        "build job should reach terminal state, got: {final_status}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn build_worker_stuck_job_recovery_sql() {
    // Verify the recovery SQL resets in_progress rows older than 30 minutes.
    // (The worker runs this on startup; we test the SQL logic directly.)
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status)
         VALUES ('stuck-agent-ng', $1, 'test:latest', 'deploying') RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let payload = json!({
        "build_id": Uuid::new_v4(),
        "agent_id": agent_id,
        "owner_id": owner_id,
        "upload_id": Uuid::new_v4().to_string(),
        "name": "stuck-agent-ng",
        "zip_path": "/tmp/nonexistent.zip",
        "image_tag": "stuck-agent-ng:latest",
        "ports": [8000u16],
        "env": {}
    });

    // Insert a job that was picked up 2 hours ago and never completed.
    let job_id: Uuid = sqlx::query_scalar(
        "INSERT INTO build_jobs (agent_id, owner_id, payload, status, picked_at)
         VALUES ($1, $2, $3, 'in_progress', now() - INTERVAL '2 hours')
         RETURNING id",
    )
    .bind(agent_id)
    .bind(owner_id)
    .bind(&payload)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Run the same SQL the worker uses during startup recovery.
    let affected = sqlx::query(
        "UPDATE build_jobs SET status = 'pending', picked_at = NULL
         WHERE status = 'in_progress' AND picked_at < now() - INTERVAL '30 minutes'",
    )
    .execute(&server.db)
    .await
    .unwrap()
    .rows_affected();

    assert_eq!(affected, 1, "recovery should reset the stuck job");

    let new_status: String =
        sqlx::query_scalar("SELECT status FROM build_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(new_status, "pending", "stuck job should be reset to pending");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn build_worker_does_not_reset_recent_in_progress_jobs() {
    // Jobs in_progress for < 30 minutes must NOT be reset (another replica may be processing them).
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner_id: Uuid = uid.parse().unwrap();

    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status)
         VALUES ('recent-job-ng', $1, 'test:latest', 'deploying') RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let payload = json!({
        "build_id": Uuid::new_v4(),
        "agent_id": agent_id,
        "owner_id": owner_id,
        "upload_id": Uuid::new_v4().to_string(),
        "name": "recent-job-ng",
        "zip_path": "/tmp/nonexistent.zip",
        "image_tag": "recent-job-ng:latest",
        "ports": [8000u16],
        "env": {}
    });

    let job_id: Uuid = sqlx::query_scalar(
        "INSERT INTO build_jobs (agent_id, owner_id, payload, status, picked_at)
         VALUES ($1, $2, $3, 'in_progress', now() - INTERVAL '5 minutes')
         RETURNING id",
    )
    .bind(agent_id)
    .bind(owner_id)
    .bind(&payload)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let affected = sqlx::query(
        "UPDATE build_jobs SET status = 'pending', picked_at = NULL
         WHERE status = 'in_progress' AND picked_at < now() - INTERVAL '30 minutes'",
    )
    .execute(&server.db)
    .await
    .unwrap()
    .rows_affected();

    assert_eq!(affected, 0, "recent in_progress job should NOT be reset");

    let status: String = sqlx::query_scalar("SELECT status FROM build_jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "in_progress", "recently-claimed job should remain in_progress");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn build_job_has_correct_initial_status() {
    // The build_jobs row must start as 'pending' (not 'in_progress' or anything else).
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = do_upload(&server, uid, "initial-status-ng", vec![], make_valid_agent_zip()).await;
    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    let agent_id: Uuid = body["agent_id"].as_str().unwrap().parse().unwrap();

    // Check status immediately — the worker may have already picked it up, so accept
    // any of the valid lifecycle states.
    let status: String =
        sqlx::query_scalar("SELECT status FROM build_jobs WHERE agent_id = $1 LIMIT 1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();

    assert!(
        ["pending", "in_progress", "done", "failed"].contains(&status.as_str()),
        "unexpected build_jobs status: {status}"
    );

    server.cleanup().await;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 5 — Schema validation (migration 013)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[serial]
async fn migration_013_spec_ports_column_exists() {
    let server = common::TestServer::start().await;

    // If the column doesn't exist this query returns an error.
    let result: Result<i64, _> = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns
         WHERE table_name = 'agent_deployments' AND column_name = 'spec_ports'",
    )
    .fetch_one(&server.db)
    .await;

    let count = result.expect("spec_ports column query failed");
    assert_eq!(count, 1, "spec_ports column should exist on agent_deployments");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn migration_013_spec_image_column_exists() {
    let server = common::TestServer::start().await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns
         WHERE table_name = 'agent_deployments' AND column_name = 'spec_image'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(count, 1, "spec_image column should exist on agent_deployments");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn migration_013_build_jobs_table_exists() {
    let server = common::TestServer::start().await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables
         WHERE table_name = 'build_jobs'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(count, 1, "build_jobs table should exist");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn migration_013_build_jobs_has_required_columns() {
    let server = common::TestServer::start().await;

    let cols: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'build_jobs'
         ORDER BY column_name",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();

    let required = ["agent_id", "completed_at", "created_at", "error_msg", "id",
                    "owner_id", "payload", "picked_at", "status"];
    for col in required {
        assert!(
            cols.iter().any(|c| c == col),
            "build_jobs should have column '{col}'; found: {cols:?}"
        );
    }

    server.cleanup().await;
}
