//! Integration tests confirming the 3 fixes from the second Copilot review
//! pass on PR #290 (agent-version immutability):
//!
//! 1. A pre-build clone rejection restores an existing agent's prior state
//!    instead of deleting it (`execute_clone_and_deploy`'s failure path).
//! 2. `PUT /api/agents/{id}` rejects a same-version-different-image request
//!    instead of silently swapping the image under an unchanged version.
//! 3. `reserve_version` atomically claims a version before any build runs,
//!    and `finalize_reserved_version_with_retry`/`release_reserved_version`
//!    correctly resolve it afterward.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test version_immutability_fixes -- --test-threads=1

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

use nasiko_server::agents::upload::execute_clone_and_deploy;
use nasiko_server::agents::versions::{
    VersionChangeError, finalize_reserved_version_with_retry, release_reserved_version,
    reserve_version,
};

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

async fn seed_agent(
    server: &common::TestServer,
    agent_id: Uuid,
    owner_id: Uuid,
    name: &str,
    version: &str,
    image: &str,
    status: &str,
) {
    sqlx::query(
        "INSERT INTO agents (id, name, owner_id, version, image, status) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(agent_id)
    .bind(name)
    .bind(owner_id)
    .bind(version)
    .bind(image)
    .bind(status)
    .execute(&server.db)
    .await
    .expect("seed agent");
}

/// `agent_versions.build_id` foreign-keys to `agent_builds(id)` — any build_id
/// passed to `reserve_version` must reference a real row here first.
async fn seed_build(server: &common::TestServer, build_id: Uuid, agent_id: Uuid, version: &str) {
    sqlx::query(
        "INSERT INTO agent_builds (id, agent_id, version_tag, image_reference) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(build_id)
    .bind(agent_id)
    .bind(version)
    .bind(format!("nasiko/build:{version}"))
    .execute(&server.db)
    .await
    .expect("seed build");
}

/// A minimal tar.gz with no AgentCard.json/pyproject.toml/Cargo.toml, so
/// `detect_version_from_dir` returns `None` and the clone pipeline fails at
/// "no valid version found" before ever reaching a Docker build.
fn make_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.finish().unwrap();
    }
    let mut gz = Vec::new();
    {
        let mut encoder = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();
    }
    gz
}

// ─── Fix #1: pre-build rejection restores instead of deleting ───────────────

#[tokio::test]
#[serial]
async fn clone_pre_build_rejection_restores_existing_agent_instead_of_deleting() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    // Seed an existing agent the way a CLI `nasiko push`/`deploy` leaves it —
    // a real `agents` row, but zero `agent_builds` history (CLI deploys never
    // go through the server's build worker). This is exactly the agent shape
    // that made `delete_agent_or_mark_failed` hard-delete instead of restore.
    let agent_id = Uuid::new_v4();
    seed_agent(
        &server,
        agent_id,
        uid,
        "existing-cli-agent",
        "1.0.0",
        "nasiko/existing:1.0.0",
        "running",
    )
    .await;

    // Simulate the queueing handler's optimistic overwrite (github.rs's
    // github_clone) before the failure we're about to trigger.
    sqlx::query(
        "UPDATE agents SET version = 'latest', image = 'nasiko/existing:latest', \
         status = 'deploying' WHERE id = $1",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let tar_gz = make_tar_gz(&[("README.md", b"no AgentCard.json here")]);
    let tar_path = std::env::temp_dir().join(format!("test-clone-{}.tar.gz", Uuid::new_v4()));
    std::fs::write(&tar_path, &tar_gz).unwrap();

    let build_id = Uuid::new_v4();
    execute_clone_and_deploy(
        server.runtime.clone() as Arc<dyn nasiko_runtime::ContainerRuntime>,
        server.db.clone(),
        reqwest::Client::new(),
        build_id,
        agent_id,
        uid,
        "upload-1".to_string(),
        "existing-cli-agent".to_string(),
        tar_path,
        vec![8000],
        HashMap::new(),
        None,
        None,
        "docker".to_string(),
        String::new(),
        1,
        "512Mi".to_string(),
        None,
        Some("1.0.0".to_string()),
        Some("nasiko/existing:1.0.0".to_string()),
        Some("running".to_string()),
    )
    .await;

    // The agent must still exist, restored to exactly what it was before the
    // optimistic overwrite — not deleted, and not left on the placeholder.
    let row: (String, Option<String>, String) =
        sqlx::query_as("SELECT version, image, status FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .expect("agent should still exist after a pre-build rejection, not be deleted");
    assert_eq!(row.0, "1.0.0");
    assert_eq!(row.1.as_deref(), Some("nasiko/existing:1.0.0"));
    assert_eq!(row.2, "running");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn clone_genuine_deploy_failure_on_existing_agent_restores_instead_of_deleting() {
    // The agent has no `agent_builds` history at all (a CLI-deployed agent),
    // so the old "was this ever built by our own build worker?" check would
    // have wrongly treated it as brand-new and deleted it. Whether *this*
    // import created the agent is the right question, and `prior_version`
    // already answers it — this must restore, not delete, even though a real
    // build/deploy was genuinely attempted and genuinely failed.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let agent_id = Uuid::new_v4();
    seed_agent(
        &server,
        agent_id,
        uid,
        "existing-cli-agent-deploy-fail",
        "1.0.0",
        "nasiko/existing:1.0.0",
        "running",
    )
    .await;
    sqlx::query(
        "UPDATE agents SET version = 'latest', image = 'nasiko/existing:latest', \
         status = 'deploying' WHERE id = $1",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    server.runtime.set_fail_deploy(true);

    let tar_gz = make_tar_gz(&[
        ("AgentCard.json", br#"{"version": "2.0.0"}"#),
        ("Dockerfile", b"FROM scratch"),
    ]);
    let tar_path = std::env::temp_dir().join(format!("test-clone-{}.tar.gz", Uuid::new_v4()));
    std::fs::write(&tar_path, &tar_gz).unwrap();

    let build_id = Uuid::new_v4();
    seed_build(&server, build_id, agent_id, "2.0.0").await;
    execute_clone_and_deploy(
        server.runtime.clone() as Arc<dyn nasiko_runtime::ContainerRuntime>,
        server.db.clone(),
        reqwest::Client::new(),
        build_id,
        agent_id,
        uid,
        "upload-3".to_string(),
        "existing-cli-agent-deploy-fail".to_string(),
        tar_path,
        vec![8000],
        HashMap::new(),
        None,
        None,
        "docker".to_string(),
        String::new(),
        1,
        "512Mi".to_string(),
        None,
        Some("1.0.0".to_string()),
        Some("nasiko/existing:1.0.0".to_string()),
        Some("running".to_string()),
    )
    .await;

    let row: (String, Option<String>, String) =
        sqlx::query_as("SELECT version, image, status FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .expect(
                "agent should still exist after a genuine deploy failure, not be deleted \
                 just because it lacks build-worker history",
            );
    assert_eq!(row.0, "1.0.0");
    assert_eq!(row.1.as_deref(), Some("nasiko/existing:1.0.0"));
    assert_eq!(row.2, "running");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn clone_pre_build_rejection_on_brand_new_agent_still_cleans_up() {
    // A genuinely new agent (no prior state to restore to) must still be
    // cleaned up like any other failed first deploy — restoring "nothing" is
    // not an option, so this exercises the other arm of the same branch.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let agent_id = Uuid::new_v4();
    seed_agent(
        &server,
        agent_id,
        uid,
        "brand-new-agent",
        "latest",
        "nasiko/brand-new:latest",
        "deploying",
    )
    .await;

    let tar_gz = make_tar_gz(&[("README.md", b"no AgentCard.json here")]);
    let tar_path = std::env::temp_dir().join(format!("test-clone-{}.tar.gz", Uuid::new_v4()));
    std::fs::write(&tar_path, &tar_gz).unwrap();

    let build_id = Uuid::new_v4();
    execute_clone_and_deploy(
        server.runtime.clone() as Arc<dyn nasiko_runtime::ContainerRuntime>,
        server.db.clone(),
        reqwest::Client::new(),
        build_id,
        agent_id,
        uid,
        "upload-2".to_string(),
        "brand-new-agent".to_string(),
        tar_path,
        vec![8000],
        HashMap::new(),
        None,
        None,
        "docker".to_string(),
        String::new(),
        1,
        "512Mi".to_string(),
        None,
        None, // no prior state — this is a first-ever deploy
        None,
        None,
    )
    .await;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1)")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(
        !exists,
        "a brand-new agent with nothing to restore should still be cleaned up"
    );

    server.cleanup().await;
}

// ─── Fix #2: same version, different image must be rejected ────────────────

#[tokio::test]
#[serial]
async fn update_rejects_image_change_under_the_same_version() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
        .json(&json!({
            "name": "img-swap-agent",
            "version": "1.0.0",
            "image": "nasiko/img-swap:1.0.0",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let agent: Value = res.json().await.unwrap();
    let agent_id = agent["id"].as_str().unwrap();

    // Same version, different image — must be rejected outright.
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent_id}"))),
        uid,
        "admin",
    )
    .json(&json!({"version": "1.0.0", "image": "nasiko/img-swap:evil"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 409);

    let stored_image: Option<String> = sqlx::query_scalar("SELECT image FROM agents WHERE id = $1")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        stored_image.as_deref(),
        Some("nasiko/img-swap:1.0.0"),
        "image must not have changed after the rejected request"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_allows_metadata_edit_with_same_version_and_image() {
    // Sanity check: a harmless metadata-only edit (same version, same image)
    // must still go through — the fix only closes the image-swap gap.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
        .json(&json!({
            "name": "metadata-edit-agent",
            "version": "1.0.0",
            "image": "nasiko/metadata-edit:1.0.0",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let agent: Value = res.json().await.unwrap();
    let agent_id = agent["id"].as_str().unwrap();

    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent_id}"))),
        uid,
        "admin",
    )
    .json(&json!({
        "version": "1.0.0",
        "image": "nasiko/metadata-edit:1.0.0",
        "description": "updated description",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        200,
        "same version + same image must still be allowed"
    );

    server.cleanup().await;
}

// ─── Fix #3: version reservation is atomic, finalize/release behave ────────

#[tokio::test]
#[serial]
async fn reserve_version_is_atomic_second_reservation_fails() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let agent_id = Uuid::new_v4();
    seed_agent(
        &server,
        agent_id,
        uid,
        "reserve-race-agent",
        "0.1.0",
        "nasiko/reserve-race:0.1.0",
        "running",
    )
    .await;

    let build_a = Uuid::new_v4();
    let build_b = Uuid::new_v4();
    seed_build(&server, build_a, agent_id, "2.0.0").await;
    seed_build(&server, build_b, agent_id, "2.0.0-other").await;

    let first = reserve_version(
        &server.db,
        agent_id,
        build_a,
        "2.0.0",
        "nasiko/reserve:2.0.0",
    )
    .await;
    assert!(first.is_ok(), "first reservation should win: {first:?}");

    let second = reserve_version(
        &server.db,
        agent_id,
        build_b,
        "2.0.0",
        "nasiko/reserve:2.0.0-other",
    )
    .await;
    assert!(
        matches!(second, Err(VersionChangeError::VersionAlreadyExists(_))),
        "a second concurrent reservation for the same version must lose: {second:?}"
    );

    let status: String = sqlx::query_scalar(
        "SELECT status FROM agent_versions WHERE agent_id = $1 AND version = '2.0.0'",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(status, "building");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn finalize_reserved_version_activates_and_archives_previous() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let agent_id = Uuid::new_v4();
    seed_agent(
        &server,
        agent_id,
        uid,
        "finalize-agent",
        "1.0.0",
        "nasiko/finalize:1.0.0",
        "running",
    )
    .await;
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, status) \
         VALUES ($1, '1.0.0', 'nasiko/finalize:1.0.0', true, 'active')",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let build_id = Uuid::new_v4();
    seed_build(&server, build_id, agent_id, "2.0.0").await;
    reserve_version(
        &server.db,
        agent_id,
        build_id,
        "2.0.0",
        "nasiko/finalize:2.0.0",
    )
    .await
    .unwrap();
    finalize_reserved_version_with_retry(&server.db, agent_id, "2.0.0").await;

    let new_row: (bool, String) = sqlx::query_as(
        "SELECT is_active, status FROM agent_versions WHERE agent_id = $1 AND version = '2.0.0'",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(new_row, (true, "active".to_string()));

    let old_row: (bool, String, bool) = sqlx::query_as(
        "SELECT is_active, status, can_rollback FROM agent_versions \
         WHERE agent_id = $1 AND version = '1.0.0'",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(old_row, (false, "archived".to_string(), true));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn release_reserved_version_frees_it_for_retry() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let agent_id = Uuid::new_v4();
    seed_agent(
        &server,
        agent_id,
        uid,
        "release-agent",
        "0.1.0",
        "nasiko/release:0.1.0",
        "running",
    )
    .await;

    let build_id = Uuid::new_v4();
    seed_build(&server, build_id, agent_id, "3.0.0").await;
    reserve_version(
        &server.db,
        agent_id,
        build_id,
        "3.0.0",
        "nasiko/release:3.0.0",
    )
    .await
    .unwrap();
    release_reserved_version(&server.db, agent_id, "3.0.0").await;

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agent_versions WHERE agent_id = $1 AND version = '3.0.0')",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(!exists, "a released reservation should be gone");

    // And the same version can be reserved again by a fresh retry attempt.
    let retry_build = Uuid::new_v4();
    seed_build(&server, retry_build, agent_id, "3.0.0-retry").await;
    let retry = reserve_version(
        &server.db,
        agent_id,
        retry_build,
        "3.0.0",
        "nasiko/release:3.0.0-retry",
    )
    .await;
    assert!(
        retry.is_ok(),
        "version should be retryable after release: {retry:?}"
    );

    server.cleanup().await;
}
