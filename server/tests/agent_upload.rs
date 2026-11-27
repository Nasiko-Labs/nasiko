//! Tests for `POST /api/agents/upload` and `GET /api/agents/deploy-status/{id}`
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
        form = form.part(
            "source",
            reqwest::multipart::Part::bytes(zip).file_name("agent.zip"),
        );
    }
    common::as_superuser(
        server.client.post(server.url("/api/agents/upload")),
        user_id,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap()
}

async fn get_build(
    server: &common::TestServer,
    user_id: &str,
    build_id: &str,
) -> reqwest::Response {
    common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/builds/{build_id}"))),
        user_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
}

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
        (
            "Dockerfile",
            b"FROM python:3.11-slim\nCMD [\"python\", \"main.py\"]",
        ),
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
    let res = upload(
        &server,
        uid,
        vec![("version_tag", "1.0.0".into())],
        Some(zip),
    )
    .await;

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

    // No auth token → require_auth rejects before the handler.
    let zip = common::make_zip(&[NO_DOCKERFILE_ZIP_ENTRY]);
    let form = reqwest::multipart::Form::new()
        .text("name", "demo")
        .text("version_tag", "1.0.0")
        .part(
            "source",
            reqwest::multipart::Part::bytes(zip).file_name("agent.zip"),
        );
    let res = server
        .client
        .post(server.url("/api/agents/upload"))
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
        vec![
            ("name", "demo-agent".into()),
            ("version_tag", "1.0.0".into()),
        ],
        Some(zip),
    )
    .await;

    assert_eq!(res.status(), 202);
    // The handler wraps the payload in a `{ data, status_code, message }`
    // envelope (UploadAndDeployResponse).
    let body: Value = res.json().await.unwrap();
    let agent_id = body["data"]["agent_id"].as_str().unwrap();
    let build_id = body["data"]["build_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(agent_id).is_ok());
    assert!(uuid::Uuid::parse_str(build_id).is_ok());
    assert_eq!(body["data"]["status"], "queued");

    // Agent persisted in the catalog.
    let agents: Value =
        common::as_superuser(server.client.get(server.url("/api/agents")), uid, "admin")
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
    assert!(
        names.contains(&"demo-agent"),
        "catalog should list the agent: {names:?}"
    );

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
async fn upload_persists_build_job_atomically_with_agent_and_build() {
    // Regression for SRV-5: the agent upsert, build record insert, and build_jobs
    // insert now commit as one transaction. Before that fix these were 3 separate
    // pool statements — if the build_jobs insert failed after the first two
    // committed, the agent was stuck in "deploying" with no job that would ever
    // move it out of that state. Assert the job row exists right alongside the
    // agent/build rows the 202 response reports.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let zip = make_valid_structure_zip();
    let res = upload(
        &server,
        uid,
        vec![
            ("name", "atomic-agent".into()),
            ("version_tag", "1.0.0".into()),
        ],
        Some(zip),
    )
    .await;

    assert_eq!(res.status(), 202);
    let body: Value = res.json().await.unwrap();
    // agent_id sits inside the { data, ... } response envelope.
    let agent_id: uuid::Uuid = body["data"]["agent_id"].as_str().unwrap().parse().unwrap();

    let job_count: i64 = sqlx::query_scalar("SELECT count(*) FROM build_jobs WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        job_count, 1,
        "build_jobs row should have committed atomically alongside the agent/build rows"
    );

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
        vec![("name", "repeat".into()), ("version_tag", "1.0.0".into())],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();

    let second: Value = upload(
        &server,
        uid,
        vec![("name", "repeat".into()), ("version_tag", "1.0.1".into())],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();

    // Same agent reused (owner-scoped upsert), distinct build records.
    // Both payloads sit inside the { data, ... } response envelope.
    assert_eq!(first["data"]["agent_id"], second["data"]["agent_id"]);
    assert_ne!(first["data"]["build_id"], second["data"]["build_id"]);
    assert!(first["data"]["agent_id"].is_string());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reupload_without_writable_field_keeps_stored_writable() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let fetch_writable = |agent_id: uuid::Uuid| {
        sqlx::query_as::<_, (bool, Option<String>)>(
            "SELECT writable, writable_path FROM agents WHERE id = $1",
        )
        .bind(agent_id)
        .fetch_one(&server.db)
    };

    // First upload opts in to persistent storage at a custom mount point.
    let first: Value = upload(
        &server,
        uid,
        vec![
            ("name", "keeps-writable".into()),
            ("version_tag", "1.0.0".into()),
            ("writable", "true".into()),
            ("writable_path", "/app/data".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    let agent_id: uuid::Uuid = first["data"]["agent_id"].as_str().unwrap().parse().unwrap();

    // Plain re-upload of a new version. The CLI omits both writable fields
    // unless the flags are passed, and absent must mean "keep the stored
    // values" — an unconditional overwrite here silently detached the agent
    // from its volume (and moved the mount back to /workspace).
    let second: Value = upload(
        &server,
        uid,
        vec![
            ("name", "keeps-writable".into()),
            ("version_tag", "1.0.1".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(first["data"]["agent_id"], second["data"]["agent_id"]);

    let (writable, writable_path) = fetch_writable(agent_id).await.unwrap();
    assert!(
        writable,
        "re-upload without the field must keep writable=true"
    );
    assert_eq!(
        writable_path.as_deref(),
        Some("/app/data"),
        "re-upload without the field must not move the mount"
    );

    // Explicit `writable=false` is still honored — carry-forward applies only
    // when the field is absent, opting out stays possible.
    let res = upload(
        &server,
        uid,
        vec![
            ("name", "keeps-writable".into()),
            ("version_tag", "1.0.2".into()),
            ("writable", "false".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await;
    assert_eq!(res.status(), 202);

    let (writable, _) = fetch_writable(agent_id).await.unwrap();
    assert!(
        !writable,
        "explicit writable=false must still disable the mount"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_rejects_an_unknown_parking_policy() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = upload(
        &server,
        uid,
        vec![
            ("name", "bad-parking".into()),
            ("version_tag", "1.0.0".into()),
            ("parking_policy", "sometimes".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await;

    assert_eq!(res.status(), 400);
    let registered: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agents WHERE name = 'bad-parking')")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!registered, "a refused upload registers no agent");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_stores_the_parking_policy_and_keeps_it_when_omitted() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let parking_policy = |agent_id: uuid::Uuid| {
        sqlx::query_scalar::<_, String>("SELECT parking_policy FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
    };
    let upload_version = |version: &'static str, policy: Option<&'static str>| {
        let mut fields = vec![
            ("name", "parking-choice".to_string()),
            ("version_tag", version.to_string()),
        ];
        if let Some(policy) = policy {
            fields.push(("parking_policy", policy.to_string()));
        }
        upload(&server, uid, fields, Some(make_valid_structure_zip()))
    };

    // A new agent with no policy gets `auto`.
    let first: Value = upload_version("1.0.0", None).await.json().await.unwrap();
    let agent_id: uuid::Uuid = first["data"]["agent_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(parking_policy(agent_id).await.unwrap(), "auto");

    // A named policy is stored.
    let res = upload_version("1.0.1", Some("resident")).await;
    assert_eq!(res.status(), 202);
    assert_eq!(parking_policy(agent_id).await.unwrap(), "resident");

    // A re-upload that names none keeps it.
    let res = upload_version("1.0.2", None).await;
    assert_eq!(res.status(), 202);
    assert_eq!(parking_policy(agent_id).await.unwrap(), "resident");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upload_rejects_reused_version_before_build() {
    // Re-uploading a version already in this agent's history must be rejected
    // synchronously, before a build is even queued — otherwise the post-build
    // write silently overwrites that history row (the exact collapse this
    // feature fixes). Create the agent via the catalog directly so its
    // initial version ("1.0.0") is seeded in `agent_versions` without needing
    // a real build to complete first.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent: Value =
        common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
            .json(&json!({"name": "dup-upload-agent", "version": "1.0.0"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let agent_id: uuid::Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let res = upload(
        &server,
        uid,
        vec![
            ("name", "dup-upload-agent".into()),
            ("version_tag", "1.0.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await;

    assert_eq!(res.status(), 409, "re-using an existing version must 409");
    let text = res.text().await.unwrap();
    assert!(
        text.contains("already exists"),
        "expected 'already exists' message, got: {text}"
    );

    // No build was queued for the rejected upload.
    let job_count: i64 = sqlx::query_scalar("SELECT count(*) FROM build_jobs WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        job_count, 0,
        "a rejected duplicate-version upload must not queue a build"
    );

    server.cleanup().await;
}

async fn list_versions(server: &common::TestServer, uid: &str, agent_id: &str) -> Vec<Value> {
    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent_id}/versions"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "list_versions should succeed");
    let body: Value = res.json().await.unwrap();
    body["data"].as_array().unwrap().clone()
}

fn find_version<'a>(versions: &'a [Value], version: &str) -> &'a Value {
    versions
        .iter()
        .find(|v| v["version"] == version)
        .unwrap_or_else(|| panic!("version {version} not found in {versions:?}"))
}

#[tokio::test]
#[serial]
async fn upload_activates_version_in_history_once_build_completes() {
    // Regression coverage for routing `execute_upload_and_deploy`'s post-build
    // write through the shared `record_version_change` recorder instead of its
    // own raw SQL: confirms the real end-to-end effect (not just the 202
    // response) — the completed build's version ends up active in history,
    // and a second upload correctly archives the first and marks it
    // rollback-eligible.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let first: Value = upload(
        &server,
        uid,
        vec![
            ("name", "activation-agent".into()),
            ("version_tag", "1.0.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    let agent_id = first["data"]["agent_id"].as_str().unwrap().to_string();
    let first_build_id = first["data"]["build_id"].as_str().unwrap();
    assert_eq!(
        wait_for_terminal_status(&server, uid, first_build_id).await,
        "success"
    );

    let versions = list_versions(&server, uid, &agent_id).await;
    assert_eq!(versions.len(), 1);
    let v1 = find_version(&versions, "1.0.0");
    assert_eq!(v1["is_active"], true);
    assert_eq!(v1["status"], "active");
    assert_eq!(v1["can_rollback"], false);

    let second: Value = upload(
        &server,
        uid,
        vec![
            ("name", "activation-agent".into()),
            ("version_tag", "1.1.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    let second_build_id = second["data"]["build_id"].as_str().unwrap();
    assert_eq!(
        wait_for_terminal_status(&server, uid, second_build_id).await,
        "success"
    );

    let versions = list_versions(&server, uid, &agent_id).await;
    assert_eq!(versions.len(), 2);
    let v1 = find_version(&versions, "1.0.0");
    assert_eq!(v1["is_active"], false);
    assert_eq!(v1["status"], "archived");
    assert_eq!(
        v1["can_rollback"], true,
        "the version genuinely running before must become rollback-eligible"
    );
    let v2 = find_version(&versions, "1.1.0");
    assert_eq!(v2["is_active"], true);
    assert_eq!(v2["status"], "active");
    assert_eq!(v2["previous_version"], "1.0.0");

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
        vec![
            ("name", "nodockerfile".into()),
            ("version_tag", "1.0.0".into()),
        ],
        Some(common::make_zip(&[NO_DOCKERFILE_ZIP_ENTRY])),
    )
    .await;

    assert_eq!(
        res.status(),
        400,
        "missing Dockerfile must be rejected immediately"
    );
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
    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/agents/deploys/{random}/stream"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 200);
    // Unknown build → the stream emits a single not_found event and closes.
    let body = res.text().await.unwrap();
    assert!(
        body.contains("not_found"),
        "expected not_found event, got: {body}"
    );

    server.cleanup().await;
}

// ─── a deploy the runtime declines ───────────────────────────────────────────

/// What a layer that declines to start workloads answers: its own code, and words for the user.
const DECLINED_CODE: &str = "start_refused";
const DECLINED_MESSAGE: &str = "This agent cannot be started right now. Try again later.";

/// An agent's version, image and status.
async fn agent_state(
    server: &common::TestServer,
    agent_id: uuid::Uuid,
) -> Option<(String, Option<String>, String)> {
    sqlx::query_as("SELECT version, image, status FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_optional(&server.db)
        .await
        .unwrap()
}

/// The reasons `GET /api/agents/uploads/{id}` gives for a failed upload.
async fn upload_error_details(
    server: &common::TestServer,
    user_id: &str,
    upload_id: &str,
) -> Vec<String> {
    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/agents/uploads/{upload_id}"))),
        user_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    serde_json::from_value(body["error_details"].clone()).unwrap()
}

/// Polls the upload's status until it is `failed`. A new agent's failed build is removed with the
/// agent, so its status is the one record of the failure that is left.
async fn wait_for_failed_upload(server: &common::TestServer, user_id: &str, upload_id: &str) {
    for _ in 0..40 {
        let res = common::as_superuser(
            server
                .client
                .get(server.url(&format!("/api/agents/uploads/{upload_id}"))),
            user_id,
            "admin",
        )
        .send()
        .await
        .unwrap();
        if res.status() == 200 {
            let body: Value = res.json().await.unwrap();
            if body["status"] == "failed" {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("upload {upload_id} did not fail in time");
}

/// An agent a CLI push left: a row with no build history, so a failed re-upload that treats it as
/// a first deploy deletes it. Stopped, as an agent is that has nothing to run.
async fn seed_stopped_cli_agent(
    server: &common::TestServer,
    owner_id: &str,
    name: &str,
) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, version, image, status) \
         VALUES ($1, $2::uuid, '1.0.0', 'nasiko/existing:1.0.0', 'stopped') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// The re-upload of an agent that is not running is admitted, builds, and then the runtime declines
/// to start it. That is no failure of the agent: it is put back as it was (not deleted, not
/// `failed`), and the user is told the declining layer's words, not a generic reason.
#[tokio::test]
#[serial]
async fn a_declined_deploy_of_a_reupload_puts_the_existing_agent_back_and_tells_why() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = seed_stopped_cli_agent(&server, uid, "declined-reupload").await;
    server
        .runtime
        .set_decline_deploy(Some((DECLINED_CODE, DECLINED_MESSAGE)));

    let queued: Value = upload(
        &server,
        uid,
        vec![
            ("name", "declined-reupload".into()),
            ("version_tag", "1.1.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(
        queued["data"]["agent_id"].as_str().unwrap(),
        agent_id.to_string(),
        "the upload is a re-upload of the existing agent"
    );
    let build_id = queued["data"]["build_id"].as_str().unwrap();
    assert_eq!(
        wait_for_terminal_status(&server, uid, build_id).await,
        "failed"
    );

    assert_eq!(
        agent_state(&server, agent_id).await,
        Some((
            "1.0.0".to_owned(),
            Some("nasiko/existing:1.0.0".to_owned()),
            "stopped".to_owned()
        )),
        "the agent keeps the version, image and status it had"
    );
    assert_eq!(
        upload_error_details(&server, uid, build_id).await,
        vec![DECLINED_MESSAGE.to_owned()]
    );
    server.cleanup().await;
}

/// Polls the agent until `reached` holds of its version, image and status: the restore that ends a
/// failed build runs after the build's own status is written.
async fn wait_for_agent_state(
    server: &common::TestServer,
    agent_id: uuid::Uuid,
    reached: impl Fn(&(String, Option<String>, String)) -> bool,
) -> (String, Option<String>, String) {
    let mut state = None;
    for _ in 0..40 {
        state = agent_state(server, agent_id).await;
        if let Some(state) = state.as_ref().filter(|state| reached(state)) {
            return state.clone();
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("agent {agent_id} did not reach the expected state, last read {state:?}");
}

/// A second upload of a not-running agent, submitted while the first is still queued, reads the
/// first one's `deploying` placeholder as the agent's state. Putting that back when the runtime
/// declines would leave the row `deploying` with no job left to move it: it ends `failed`, with
/// the row kept.
#[tokio::test]
#[serial]
async fn a_declined_deploy_that_read_a_deploying_placeholder_ends_failed_not_deploying() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = seed_stopped_cli_agent(&server, uid, "declined-behind-another").await;
    sqlx::query("UPDATE agents SET status = 'deploying' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .runtime
        .set_decline_deploy(Some((DECLINED_CODE, DECLINED_MESSAGE)));

    let queued: Value = upload(
        &server,
        uid,
        vec![
            ("name", "declined-behind-another".into()),
            ("version_tag", "1.1.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    let build_id = queued["data"]["build_id"].as_str().unwrap();
    assert_eq!(
        wait_for_terminal_status(&server, uid, build_id).await,
        "failed"
    );

    let (_, _, status) =
        wait_for_agent_state(&server, agent_id, |(_, _, status)| status != "deploying").await;
    assert_eq!(status, "failed");
    server.cleanup().await;
}

/// A stop issued while the build runs is the agent's status now: a declined deploy puts the
/// version and image back but does not overwrite the status with the one the upload read.
#[tokio::test]
#[serial]
async fn a_declined_deploy_does_not_overwrite_a_status_changed_during_the_build() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = seed_stopped_cli_agent(&server, uid, "stopped-during-build").await;
    sqlx::query("UPDATE agents SET status = 'running' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .runtime
        .set_decline_deploy(Some((DECLINED_CODE, DECLINED_MESSAGE)));
    let build_gate = server.runtime.hold_builds();

    let queued: Value = upload(
        &server,
        uid,
        vec![
            ("name", "stopped-during-build".into()),
            ("version_tag", "1.1.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    let build_id = queued["data"]["build_id"].as_str().unwrap();
    for _ in 0..40 {
        if server.runtime.builds_started() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(server.runtime.builds_started(), 1, "the build is running");

    sqlx::query("UPDATE agents SET status = 'stopped' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
    build_gate.notify_one();
    assert_eq!(
        wait_for_terminal_status(&server, uid, build_id).await,
        "failed"
    );

    let restored =
        wait_for_agent_state(&server, agent_id, |(version, _, _)| version == "1.0.0").await;
    assert_eq!(
        restored,
        (
            "1.0.0".to_owned(),
            Some("nasiko/existing:1.0.0".to_owned()),
            "stopped".to_owned()
        ),
        "the stop stays, the version and image are put back"
    );
    server.cleanup().await;
}

/// The contrast: a deploy that fails for any other reason keeps the generic reason.
#[tokio::test]
#[serial]
async fn a_deploy_that_fails_for_another_reason_keeps_the_generic_reason() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    server.runtime.set_fail_deploy(true);

    let queued: Value = upload(
        &server,
        uid,
        vec![
            ("name", "failing-newcomer".into()),
            ("version_tag", "1.0.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    let build_id = queued["data"]["build_id"].as_str().unwrap();
    wait_for_failed_upload(&server, uid, build_id).await;

    assert_eq!(
        upload_error_details(&server, uid, build_id).await,
        vec!["upload and deploy failed".to_owned()]
    );
    server.cleanup().await;
}

/// A declined deploy of a brand-new agent has nothing to put back: it follows the old path (the
/// agent never ran, so its row goes), with the declining layer's words as the reason.
#[tokio::test]
#[serial]
async fn a_declined_deploy_of_a_new_agent_removes_it_and_tells_why() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    server
        .runtime
        .set_decline_deploy(Some((DECLINED_CODE, DECLINED_MESSAGE)));

    let queued: Value = upload(
        &server,
        uid,
        vec![
            ("name", "declined-newcomer".into()),
            ("version_tag", "1.0.0".into()),
        ],
        Some(make_valid_structure_zip()),
    )
    .await
    .json()
    .await
    .unwrap();
    let agent_id: uuid::Uuid = queued["data"]["agent_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let build_id = queued["data"]["build_id"].as_str().unwrap();
    wait_for_failed_upload(&server, uid, build_id).await;

    // The status is written before the agent is removed.
    for _ in 0..40 {
        if agent_state(&server, agent_id).await.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(agent_state(&server, agent_id).await, None);
    assert_eq!(
        upload_error_details(&server, uid, build_id).await,
        vec![DECLINED_MESSAGE.to_owned()]
    );
    server.cleanup().await;
}

/// Job rows queued before the prior state was carried are still in `build_jobs` when a server
/// with it starts: they must read, with nothing to put back, so a declined deploy of one ends as
/// it did before.
#[test]
fn a_queued_upload_job_from_before_the_prior_state_was_carried_still_reads() {
    let queued = json!({
        "kind": "Upload",
        "build_id": uuid::Uuid::new_v4(),
        "agent_id": uuid::Uuid::new_v4(),
        "owner_id": uuid::Uuid::new_v4(),
        "upload_id": "u-1",
        "name": "queued-before",
        "zip_path": "/nowhere/upload.zip",
        "image_tag": "nasiko/queued-before:1.0.0",
        "ports": [8000],
        "env": {},
        "writable": false,
    });

    let job: nasiko_server::agents::upload::BuildJobPayload =
        serde_json::from_value(queued).unwrap();

    assert!(matches!(
        job,
        nasiko_server::agents::upload::BuildJobPayload::Upload {
            prior_version: None,
            prior_image: None,
            prior_status: None,
            ..
        }
    ));
}
