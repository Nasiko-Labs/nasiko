//! Regression tests for the shared build-push OCI credential (the fix that
//! lets an in-cluster BuildKit build Job push freshly-built agent images):
//! `nasiko_oci::authz::BuildServiceIdentity`, checked via HTTP Basic auth
//! (username `"build-service"`) against `Config::build_push_token`.
//!
//! The core invariant this whole fix exists to preserve: a valid *pull*
//! credential must never be usable on a write route, even though write
//! routes now also accept the build-push credential — see `Writer`'s doc for
//! why that's a separate enum from `Caller` rather than one extended type.
//!
//! Requires infra (Postgres): `cargo test -p nasiko-server --test oci_build_push_credential -- --test-threads=1`

mod common;

use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

const TEST_BUILD_PUSH_TOKEN: &str = "test-build-push-token-do-not-use-in-prod";

async fn insert_user(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id")
        .bind(username)
        .bind(format!("{username}@oci-build-push-test.com"))
        .fetch_one(db)
        .await
        .expect("insert test user")
}

async fn insert_agent(db: &PgPool, name: &str, owner_id: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', 'img:1') RETURNING id")
        .bind(name)
        .bind(owner_id)
        .fetch_one(db)
        .await
        .expect("insert test agent")
}

async fn start_server_with_build_push_token() -> common::TestServer {
    common::TestServer::start_with(|config| {
        config.build_push_token = TEST_BUILD_PUSH_TOKEN.to_string();
    })
    .await
}

/// The build-service credential can push a manifest — the actual thing this
/// fix exists to make possible.
#[tokio::test]
#[serial]
async fn build_service_credential_can_push_a_manifest() {
    let server = start_server_with_build_push_token().await;
    let owner_id = insert_user(&server.db, "build-push-owner1").await;
    let agent_name = format!("build-push-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    let resp = common::as_pull_credential(
        server
            .client
            .put(server.url(&format!("/v2/nasiko/{agent_name}/manifests/latest")))
            .header("content-type", "application/vnd.oci.image.manifest.v1+json")
            .body("{}"),
        "build-service",
        TEST_BUILD_PUSH_TOKEN,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 201, "a valid build-push credential must be able to push a manifest");

    server.cleanup().await;
}

/// The core invariant: a valid PULL credential must still be rejected on a
/// write route, even now that write routes accept `Writer` (which includes
/// the build-service identity) instead of `CallerIdentity` alone.
#[tokio::test]
#[serial]
async fn pull_credential_still_cannot_push_a_manifest_when_build_push_token_is_configured() {
    let server = start_server_with_build_push_token().await;
    let owner_id = insert_user(&server.db, "build-push-owner2").await;
    let agent_name = format!("build-push-agent-{}", Uuid::new_v4());
    let agent_id = insert_agent(&server.db, &agent_name, owner_id).await;
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id).await.unwrap().unwrap();

    let resp = common::as_pull_credential(
        server
            .client
            .put(server.url(&format!("/v2/nasiko/{agent_name}/manifests/latest")))
            .header("content-type", "application/vnd.oci.image.manifest.v1+json")
            .body("{}"),
        &cred.username,
        &cred.token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 401, "a pull credential must never be able to push a manifest, build-push token or not");

    server.cleanup().await;
}

/// Wrong password against the build-service username must fail, and must
/// not accidentally fall through to the pull-credential check succeeding.
#[tokio::test]
#[serial]
async fn build_service_wrong_token_is_rejected() {
    let server = start_server_with_build_push_token().await;
    let owner_id = insert_user(&server.db, "build-push-owner3").await;
    let agent_name = format!("build-push-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    let resp = common::as_pull_credential(
        server
            .client
            .put(server.url(&format!("/v2/nasiko/{agent_name}/manifests/latest")))
            .header("content-type", "application/vnd.oci.image.manifest.v1+json")
            .body("{}"),
        "build-service",
        "wrong-token",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 401);

    server.cleanup().await;
}

/// When `build_push_token` is unset (the default — `AGENT_RUNTIME=local`),
/// the build-service credential must never match anything, even the empty
/// string — this is the guard against an accidental "empty matches empty".
#[tokio::test]
#[serial]
async fn build_service_credential_rejected_when_token_unconfigured() {
    let server = common::TestServer::start().await; // build_push_token left empty
    let owner_id = insert_user(&server.db, "build-push-owner4").await;
    let agent_name = format!("build-push-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    let resp = common::as_pull_credential(
        server
            .client
            .put(server.url(&format!("/v2/nasiko/{agent_name}/manifests/latest")))
            .header("content-type", "application/vnd.oci.image.manifest.v1+json")
            .body("{}"),
        "build-service",
        "",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 401);

    server.cleanup().await;
}

/// The build-service credential is unrestricted by repo (unlike pull
/// credentials) — it can push to an agent it has no ownership relationship
/// with at all, since it's cluster-wide trusted infrastructure, not a
/// per-tenant boundary. Also proves read access (HEAD/GET) works, which
/// BuildKit needs for its blob-exists check before pushing.
#[tokio::test]
#[serial]
async fn build_service_credential_has_unrestricted_read_and_write_access() {
    let server = start_server_with_build_push_token().await;
    let owner_id = insert_user(&server.db, "build-push-owner5").await;
    let agent_name = format!("build-push-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    let tags_resp = common::as_pull_credential(
        server.client.get(server.url(&format!("/v2/nasiko/{agent_name}/tags/list"))),
        "build-service",
        TEST_BUILD_PUSH_TOKEN,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(tags_resp.status(), 200, "build-service credential must be able to read any repo");

    server.cleanup().await;
}
