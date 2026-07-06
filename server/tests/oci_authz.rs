//! Regression tests for the OCI registry cross-tenant access-control gap
//! (Phase A3): `/v2/{owner}/{repo}/...` handlers previously discarded the
//! path's owner/repo segments entirely and let any authenticated caller
//! read/delete any blob or manifest regardless of which agent it belongs to.
//!
//! These tests exercise `GET /v2/{owner}/{repo}/tags/list`, since it only
//! touches `oci_manifests` (no S3/MinIO needed) while still going through
//! the exact same `check_repo_access` gate as the blob/manifest handlers.
//!
//! Requires infra (Postgres): `cargo test -p nasiko-server --test oci_authz -- --test-threads=1`

mod common;

use serde_json::Value;
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_user(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@oci-authz-test.com"))
    .fetch_one(db)
    .await
    .expect("insert test user")
}

async fn insert_agent(db: &PgPool, name: &str, owner_id: Uuid) {
    sqlx::query("INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', 'img:1')")
        .bind(name)
        .bind(owner_id)
        .execute(db)
        .await
        .expect("insert test agent");
}

async fn insert_manifest(db: &PgPool, repository: &str) {
    sqlx::query(
        "INSERT INTO oci_manifests (digest, repository, reference, media_type, content, size_bytes)
         VALUES ($1, $2, 'latest', 'application/vnd.oci.image.manifest.v1+json', '{}', 2)",
    )
    .bind(format!("sha256:{}", Uuid::new_v4().simple()))
    .bind(repository)
    .execute(db)
    .await
    .expect("insert test manifest");
}

/// A non-owner, non-superuser caller must be denied `tags/list` for a repo
/// whose agent-name segment belongs to someone else.
#[tokio::test]
#[serial]
async fn tags_list_denies_non_owner_of_claimed_repo() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-owner").await;
    let stranger_id = insert_user(&server.db, "oci-stranger").await;
    let agent_name = format!("oci-authz-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;

    let resp = common::as_member(
        server.client.get(server.url(&format!("/v2/nasiko/{agent_name}/tags/list"))),
        &stranger_id.to_string(),
        "oci-stranger",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 403, "non-owner must be forbidden from a claimed repo");

    server.cleanup().await;
}

/// The agent's actual owner must be allowed through.
#[tokio::test]
#[serial]
async fn tags_list_allows_owner_of_claimed_repo() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-owner2").await;
    let agent_name = format!("oci-authz-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;

    let resp = common::as_member(
        server.client.get(server.url(&format!("/v2/nasiko/{agent_name}/tags/list"))),
        &owner_id.to_string(),
        "oci-owner2",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "the owner must be able to list their own repo's tags");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["tags"], serde_json::json!(["latest"]));

    server.cleanup().await;
}

/// A repo whose name doesn't (yet) match any registered agent is unclaimed —
/// any authenticated caller may proceed. This is the CLI's "push image before
/// registering the agent" ordering (oss/cli/src/commands/push.rs); a
/// brand-new repo must not be blocked before an owner exists to check against.
#[tokio::test]
#[serial]
async fn tags_list_allows_anyone_on_unclaimed_repo() {
    let server = common::TestServer::start().await;
    let stranger_id = insert_user(&server.db, "oci-stranger2").await;
    let unclaimed_name = format!("oci-authz-unclaimed-{}", Uuid::new_v4());
    insert_manifest(&server.db, &format!("nasiko/{unclaimed_name}")).await;

    let resp = common::as_member(
        server.client.get(server.url(&format!("/v2/nasiko/{unclaimed_name}/tags/list"))),
        &stranger_id.to_string(),
        "oci-stranger2",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "an unclaimed (not-yet-registered) repo must not block access");

    server.cleanup().await;
}

/// A superuser must bypass the ownership check.
#[tokio::test]
#[serial]
async fn tags_list_allows_superuser_on_any_repo() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-owner3").await;
    let agent_name = format!("oci-authz-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;

    let resp = common::as_superuser(
        server.client.get(server.url(&format!("/v2/nasiko/{agent_name}/tags/list"))),
        &Uuid::new_v4().to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "a superuser must be able to access any repo");

    server.cleanup().await;
}

// ─── soundness under per-owner name collisions ──────────────────────────────
//
// Agent names are unique only per-owner (migration 015), and the OCI repo
// path carries no real owner segment — so two different owners can both have
// an agent named e.g. "shared-name", producing the SAME `oci_manifests.repository`
// string for both. A single-row `SELECT owner_id ... LIMIT 1` lookup would pick
// an arbitrary one of the two rows, nondeterministically denying whichever
// owner didn't get picked. `check_repo_access` must ask "do I own a match" and
// "does anyone" as two independent existence checks instead.

#[tokio::test]
#[serial]
async fn tags_list_sound_when_two_owners_share_an_agent_name() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "oci-collision-a").await;
    let owner_b = insert_user(&server.db, "oci-collision-b").await;
    let shared_name = format!("oci-collision-{}", Uuid::new_v4());
    insert_agent(&server.db, &shared_name, owner_a).await;
    insert_agent(&server.db, &shared_name, owner_b).await;
    insert_manifest(&server.db, &format!("nasiko/{shared_name}")).await;

    for (owner_id, username) in [(owner_a, "oci-collision-a"), (owner_b, "oci-collision-b")] {
        let resp = common::as_member(
            server.client.get(server.url(&format!("/v2/nasiko/{shared_name}/tags/list"))),
            &owner_id.to_string(),
            username,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(
            resp.status(), 200,
            "owner {username} must not be denied due to the other owner's row sharing this name"
        );
    }

    server.cleanup().await;
}

// ─── destructive ops must not accept the "unclaimed repo" pass-through ──────
//
// `check_repo_access`'s "no agent row yet → allow" policy exists so the CLI's
// push-before-register ordering works. But blob storage is globally
// content-addressed and deduplicated by digest — so applying that same
// pass-through to DELETE would let a stranger push to a throwaway, never-to-
// be-registered repo name purely to reach a delete endpoint and destroy
// content that's actually shared with someone else's claimed, registered
// agent. Destructive ops must require an existing claim (or superuser).

async fn delete_manifest(server: &common::TestServer, repo: &str, user_id: &str, username: &str) -> reqwest::Response {
    common::as_member(
        server.client.delete(server.url(&format!("/v2/nasiko/{repo}/manifests/latest"))),
        user_id,
        username,
    )
    .send()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn manifest_delete_denies_unclaimed_repo() {
    let server = common::TestServer::start().await;
    let stranger_id = insert_user(&server.db, "oci-del-stranger").await;
    let unclaimed_name = format!("oci-del-unclaimed-{}", Uuid::new_v4());
    insert_manifest(&server.db, &format!("nasiko/{unclaimed_name}")).await;

    let resp = delete_manifest(&server, &unclaimed_name, &stranger_id.to_string(), "oci-del-stranger").await;

    assert_eq!(
        resp.status(), 403,
        "delete on an unclaimed repo must be denied — read access being open to an unclaimed repo must not extend to destroying its content"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn manifest_delete_allows_owner() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-del-owner").await;
    let agent_name = format!("oci-del-owned-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;

    let resp = delete_manifest(&server, &agent_name, &owner_id.to_string(), "oci-del-owner").await;

    assert_eq!(resp.status(), 202, "the owner must be able to delete their own repo's manifest");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn manifest_delete_denies_non_owner_of_claimed_repo() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-del-owner2").await;
    let stranger_id = insert_user(&server.db, "oci-del-stranger2").await;
    let agent_name = format!("oci-del-claimed-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    insert_manifest(&server.db, &format!("nasiko/{agent_name}")).await;

    let resp = delete_manifest(&server, &agent_name, &stranger_id.to_string(), "oci-del-stranger2").await;

    assert_eq!(resp.status(), 403, "a non-owner must not be able to delete another owner's manifest");

    server.cleanup().await;
}
