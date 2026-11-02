//! Tests for owner-scoped image paths (`u-{owner_uuid}/{slug}`).
//!
//! With per-owner namespaces, new agent images go to `u-{uuid}/{slug}` while
//! the legacy `nasiko/{slug}` namespace remains readable for backward
//! compatibility but closed to new writes (except by superusers and when the
//! sole owner continues using their existing legacy repo).
//!
//! Requires infra (Postgres): `cargo test -p nasiko-server --test oci_owner_namespace -- --test-threads=1`

mod common;

use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_user(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@owner-namespace-test.com"))
    .fetch_one(db)
    .await
    .expect("insert test user")
}

async fn insert_superuser(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, true) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@owner-namespace-test.com"))
    .fetch_one(db)
    .await
    .expect("insert test superuser")
}

async fn insert_agent(db: &PgPool, name: &str, owner_id: Uuid, image: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', $3) RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .bind(image)
    .fetch_one(db)
    .await
    .expect("insert test agent")
}

async fn insert_manifest(db: &PgPool, repository: &str) {
    sqlx::query(
        "WITH m AS (
             INSERT INTO oci_manifests (digest, repository, media_type, content, size_bytes)
             VALUES ($1, $2, 'application/vnd.oci.image.manifest.v1+json', '{}', 2)
             RETURNING repository, digest
         )
         INSERT INTO oci_tags (repository, tag, digest)
         SELECT repository, 'latest', digest FROM m",
    )
    .bind(format!("sha256:{}", Uuid::new_v4().simple()))
    .bind(repository)
    .execute(db)
    .await
    .expect("insert test manifest");
}

// ─── Owner namespace write access ────────────────────────────────────────────

/// A session can write to their own `u-{uuid}/` namespace.
#[tokio::test]
#[serial]
async fn owner_namespace_write_own_allowed() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "owner-write-own").await;
    let repo = format!("u-{}/my-agent", owner_id.as_simple());

    // Initiate a blob upload (write operation)
    let resp = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/{}/blobs/uploads/", repo))),
        &owner_id.to_string(),
        "owner-write-own",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        202,
        "owner must be able to write to their own namespace"
    );

    server.cleanup().await;
}

/// A session cannot write to another user's `u-{uuid}/` namespace.
#[tokio::test]
#[serial]
async fn owner_namespace_write_other_rejected() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "owner-a").await;
    let stranger_id = insert_user(&server.db, "stranger-b").await;
    let repo = format!("u-{}/agent", owner_id.as_simple());

    let resp = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/{}/blobs/uploads/", repo))),
        &stranger_id.to_string(),
        "stranger-b",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "user must not be able to write to another's namespace"
    );

    server.cleanup().await;
}

// ─── Owner namespace read access ─────────────────────────────────────────────

/// A session can read from their own `u-{uuid}/` namespace.
#[tokio::test]
#[serial]
async fn owner_namespace_read_own_allowed() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "owner-read-own").await;
    let repo = format!("u-{}/my-agent", owner_id.as_simple());
    insert_manifest(&server.db, &repo).await;

    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", repo))),
        &owner_id.to_string(),
        "owner-read-own",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "owner must be able to read from their own namespace"
    );

    server.cleanup().await;
}

/// A session cannot read from another user's `u-{uuid}/` namespace.
#[tokio::test]
#[serial]
async fn owner_namespace_read_other_rejected() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "owner-c").await;
    let stranger_id = insert_user(&server.db, "stranger-d").await;
    let repo = format!("u-{}/agent", owner_id.as_simple());
    insert_manifest(&server.db, &repo).await;

    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", repo))),
        &stranger_id.to_string(),
        "stranger-d",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "user must not be able to read from another's namespace"
    );

    server.cleanup().await;
}

// ─── Legacy namespace write access ───────────────────────────────────────────

/// Non-superuser cannot write to an unclaimed legacy `nasiko/` repo.
#[tokio::test]
#[serial]
async fn legacy_write_unclaimed_rejected() {
    let server = common::TestServer::start().await;
    let user_id = insert_user(&server.db, "user-legacy-write").await;
    let repo = format!("nasiko/unclaimed-{}", Uuid::new_v4());

    let resp = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/{}/blobs/uploads/", repo))),
        &user_id.to_string(),
        "user-legacy-write",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "non-superuser must not be able to write to unclaimed legacy repo"
    );

    server.cleanup().await;
}

/// Sole owner can write to their legacy `nasiko/` repo.
#[tokio::test]
#[serial]
async fn legacy_write_sole_owner_allowed() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "sole-owner").await;
    let repo = format!("nasiko/sole-{}", Uuid::new_v4());
    let image = format!("{}:1.0", repo);
    insert_agent(&server.db, "sole-agent", owner_id, &image).await;

    let resp = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/{}/blobs/uploads/", repo))),
        &owner_id.to_string(),
        "sole-owner",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        202,
        "sole owner must be able to write to their legacy repo"
    );

    server.cleanup().await;
}

/// User cannot write to a legacy `nasiko/` repo shared with another owner.
#[tokio::test]
#[serial]
async fn legacy_write_shared_rejected() {
    let server = common::TestServer::start().await;
    let owner1 = insert_user(&server.db, "owner-1").await;
    let owner2 = insert_user(&server.db, "owner-2").await;
    let repo = format!("nasiko/shared-{}", Uuid::new_v4());
    let image = format!("{}:1.0", repo);
    insert_agent(&server.db, "shared-1", owner1, &image).await;
    insert_agent(&server.db, "shared-2", owner2, &image).await;

    let resp = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/{}/blobs/uploads/", repo))),
        &owner1.to_string(),
        "owner-1",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "user must not be able to write to repo shared with others"
    );

    server.cleanup().await;
}

// ─── Legacy namespace read access ────────────────────────────────────────────

/// Anyone can read an unclaimed legacy `nasiko/` repo.
#[tokio::test]
#[serial]
async fn legacy_read_unclaimed_allowed() {
    let server = common::TestServer::start().await;
    let user_id = insert_user(&server.db, "reader").await;
    let repo = format!("nasiko/unclaimed-read-{}", Uuid::new_v4());
    insert_manifest(&server.db, &repo).await;

    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", repo))),
        &user_id.to_string(),
        "reader",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "anyone should be able to read unclaimed legacy repo"
    );

    server.cleanup().await;
}

/// Owner can read their legacy `nasiko/` repo.
#[tokio::test]
#[serial]
async fn legacy_read_owner_allowed() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "legacy-owner").await;
    let repo = format!("nasiko/owner-read-{}", Uuid::new_v4());
    let image = format!("{}:1.0", repo);
    insert_agent(&server.db, "owner-agent", owner_id, &image).await;
    insert_manifest(&server.db, &repo).await;

    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", repo))),
        &owner_id.to_string(),
        "legacy-owner",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "owner should be able to read their legacy repo"
    );

    server.cleanup().await;
}

/// Stranger cannot read another owner's legacy `nasiko/` repo.
#[tokio::test]
#[serial]
async fn legacy_read_stranger_rejected() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "legacy-owner-2").await;
    let stranger_id = insert_user(&server.db, "stranger").await;
    let repo = format!("nasiko/stranger-test-{}", Uuid::new_v4());
    let image = format!("{}:1.0", repo);
    insert_agent(&server.db, "owned-agent", owner_id, &image).await;
    insert_manifest(&server.db, &repo).await;

    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", repo))),
        &stranger_id.to_string(),
        "stranger",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "stranger should not be able to read another owner's legacy repo"
    );

    server.cleanup().await;
}

// ─── Superuser unrestricted ──────────────────────────────────────────────────

/// Superuser can write to any namespace.
#[tokio::test]
#[serial]
async fn superuser_write_unrestricted() {
    let server = common::TestServer::start().await;
    let other_owner = insert_user(&server.db, "other-owner").await;
    let admin_id = insert_superuser(&server.db, "admin").await;
    let repo = format!("u-{}/agent", other_owner.as_simple());

    let resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/v2/{}/blobs/uploads/", repo))),
        &admin_id.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        202,
        "superuser must be able to write anywhere"
    );

    server.cleanup().await;
}

/// Superuser can read from any namespace.
#[tokio::test]
#[serial]
async fn superuser_read_unrestricted() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "owner-for-super").await;
    let admin_id = insert_superuser(&server.db, "admin").await;
    let repo = format!("u-{}/agent", owner_id.as_simple());
    insert_manifest(&server.db, &repo).await;

    let resp = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", repo))),
        &admin_id.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "superuser must be able to read anywhere"
    );

    server.cleanup().await;
}

// ─── Pull credentials work with both namespaces ──────────────────────────────

/// Pull credential works with owner namespace.
#[tokio::test]
#[serial]
async fn pull_credential_owner_namespace() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-owner").await;
    let repo = format!("u-{}/my-agent", owner_id.as_simple());
    let image = format!("registry.io/{}:1.0", repo);
    let agent_id = insert_agent(&server.db, "my-agent", owner_id, &image).await;
    insert_manifest(&server.db, &repo).await;

    // Mint pull credential
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .expect("first mint returns credential");

    // Use it to read
    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "pull credential must work for owner namespace"
    );

    server.cleanup().await;
}

/// Pull credential works with legacy namespace.
#[tokio::test]
#[serial]
async fn pull_credential_legacy_namespace() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "pull-legacy-owner").await;
    let repo = format!("nasiko/legacy-pull-{}", Uuid::new_v4());
    let image = format!("{}:1.0", repo);
    let agent_id = insert_agent(&server.db, "legacy-agent", owner_id, &image).await;
    insert_manifest(&server.db, &repo).await;

    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_id)
        .await
        .unwrap()
        .expect("first mint returns credential");

    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "pull credential must work for legacy namespace"
    );

    server.cleanup().await;
}
