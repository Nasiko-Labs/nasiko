//! Integration tests for `push` vs `deploy` version-activation semantics on
//! `POST /api/agents` and `PUT /api/agents/{id}`.
//!
//! `nasiko push` makes an image available in the registry without deploying
//! it, so its catalog update sends `activate_version: false` — the version
//! must be recorded in history (still protected against silent duplicate
//! overwrite) without being marked active, and without moving `agents.version`
//! /`agents.image` (which mean "what's currently deployed"). A later real
//! deploy of that same version (default `activate_version: true`) promotes it
//! to active and archives whatever was actually running.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_version_activation -- --test-threads=1

mod common;

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

async fn create_agent(server: &common::TestServer, uid: &str, body: Value) -> Value {
    common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn put_agent(
    server: &common::TestServer,
    uid: &str,
    agent_id: &str,
    body: Value,
) -> reqwest::Response {
    common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent_id}"))),
        uid,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap()
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

// ─── create() version validation ─────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn create_with_non_plain_version_succeeds_but_does_not_seed_history() {
    // The `agents.version` column stays free-form on purpose (some callers rely
    // on creating an agent with a legacy/non-semver version and getting a clear
    // error from a later explicit-version-required update, rather than being
    // blocked at creation time). What must never happen is that free-form text
    // ending up in `agent_versions` history, which is the actual bug this
    // seeding step must not reintroduce.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(
        &server,
        uid,
        json!({"name": "latest-agent", "version": "latest"}),
    )
    .await;
    assert_eq!(agent["version"], "latest", "agents.version stays free-form");
    let agent_id = agent["id"].as_str().unwrap();

    let versions = list_versions(&server, uid, agent_id).await;
    assert!(
        versions.is_empty(),
        "a non-plain version must not be seeded into agent_versions: {versions:?}"
    );

    server.cleanup().await;
}

// ─── push semantics (activate_version: false) ────────────────────────────────

#[tokio::test]
#[serial]
async fn push_style_update_records_inactive_version_without_moving_agent_columns() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(
        &server,
        uid,
        json!({"name": "push-agent", "version": "1.0.0", "image": "push-agent:1.0.0"}),
    )
    .await;
    let agent_id = agent["id"].as_str().unwrap();

    // Simulate `nasiko push`: register a new version in the registry without deploying it.
    let res = put_agent(
        &server,
        uid,
        agent_id,
        json!({
            "version": "2.0.0",
            "image": "push-agent:2.0.0",
            "activate_version": false,
        }),
    )
    .await;
    assert_eq!(res.status(), 200, "push-style update should succeed");
    let updated: Value = res.json().await.unwrap();

    // The agent's "currently deployed" columns must not move — nothing was deployed.
    assert_eq!(updated["version"], "1.0.0");
    assert_eq!(updated["image"], "push-agent:1.0.0");

    let versions = list_versions(&server, uid, agent_id).await;

    // The original active version is untouched — not archived.
    let v1 = find_version(&versions, "1.0.0");
    assert_eq!(v1["is_active"], true);
    assert_eq!(v1["status"], "active");

    // The pushed version is recorded, but inactive.
    let v2 = find_version(&versions, "2.0.0");
    assert_eq!(v2["is_active"], false);
    assert_eq!(v2["status"], "pushed");
    assert_eq!(v2["can_rollback"], false);
    assert_eq!(v2["image_tag"], "push-agent:2.0.0");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn pushed_version_still_rejects_duplicate_push() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(
        &server,
        uid,
        json!({"name": "dup-push-agent", "version": "1.0.0"}),
    )
    .await;
    let agent_id = agent["id"].as_str().unwrap();

    let first = put_agent(
        &server,
        uid,
        agent_id,
        json!({
            "version": "2.0.0",
            "image": "dup-push-agent:2.0.0",
            "activate_version": false,
        }),
    )
    .await;
    assert_eq!(first.status(), 200);

    // Pushing the exact same version again, without --overwrite, must still 409 —
    // `activate_version: false` must not bypass the duplicate-version guard.
    let second = put_agent(
        &server,
        uid,
        agent_id,
        json!({
            "version": "2.0.0",
            "image": "dup-push-agent:2.0.0",
            "activate_version": false,
        }),
    )
    .await;
    assert_eq!(second.status(), 409);
    let text = second.text().await.unwrap();
    assert!(
        text.contains("already exists"),
        "expected 'already exists' message, got: {text}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deploy_after_push_promotes_pushed_version_to_active() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(
        &server,
        uid,
        json!({"name": "promote-agent", "version": "1.0.0", "image": "promote-agent:1.0.0"}),
    )
    .await;
    let agent_id = agent["id"].as_str().unwrap();

    // `nasiko push` registers 2.0.0 without activating it.
    let pushed = put_agent(
        &server,
        uid,
        agent_id,
        json!({
            "version": "2.0.0",
            "image": "promote-agent:2.0.0",
            "activate_version": false,
        }),
    )
    .await;
    assert_eq!(pushed.status(), 200);

    // `nasiko deploy` of that same version (activate_version defaults to true)
    // must promote it to active and archive the version that was really running.
    let deployed = put_agent(
        &server,
        uid,
        agent_id,
        json!({
            "version": "2.0.0",
            "image": "promote-agent:2.0.0",
        }),
    )
    .await;
    assert_eq!(deployed.status(), 200);
    let updated: Value = deployed.json().await.unwrap();
    assert_eq!(updated["version"], "2.0.0");
    assert_eq!(updated["image"], "promote-agent:2.0.0");

    let versions = list_versions(&server, uid, agent_id).await;

    let v1 = find_version(&versions, "1.0.0");
    assert_eq!(v1["is_active"], false);
    assert_eq!(v1["status"], "archived");
    assert_eq!(
        v1["can_rollback"], true,
        "the version genuinely running before must become rollback-eligible"
    );

    let v2 = find_version(&versions, "2.0.0");
    assert_eq!(v2["is_active"], true);
    assert_eq!(v2["status"], "active");

    server.cleanup().await;
}

// ─── image fallback on version-only update ──────────────────────────────────

#[tokio::test]
#[serial]
async fn version_only_update_preserves_current_image_in_history() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(
        &server,
        uid,
        json!({"name": "image-fallback-agent", "version": "1.0.0", "image": "image-fallback-agent:1.0.0"}),
    )
    .await;
    let agent_id = agent["id"].as_str().unwrap();

    // No `image` field at all — must not write an empty image_tag into history.
    let res = put_agent(&server, uid, agent_id, json!({"version": "1.1.0"})).await;
    assert_eq!(res.status(), 200);

    let versions = list_versions(&server, uid, agent_id).await;
    let v = find_version(&versions, "1.1.0");
    assert_eq!(
        v["image_tag"], "image-fallback-agent:1.0.0",
        "version-only update must inherit the agent's current image, not an empty string"
    );

    server.cleanup().await;
}
