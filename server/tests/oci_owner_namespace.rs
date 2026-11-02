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
    let image = format!("{}:1.0", repo);
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

// ─── R-123/R-124 regression tests ────────────────────────────────────────────

/// Catalog create with a foreign owner namespace image is refused (R-123).
#[tokio::test]
#[serial]
async fn catalog_create_foreign_owner_namespace_rejected() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "owner-a-catalog").await;
    let owner_b = insert_user(&server.db, "owner-b-catalog").await;
    let foreign_image = format!("u-{}/agent:1.0", owner_a.as_simple());

    let body = serde_json::json!({
        "name": "malicious-agent",
        "display_name": "Malicious Agent",
        "description": "Trying to use another owner's image",
        "image": foreign_image,
    });

    let resp = common::as_member(
        server.client.post(server.url("/api/agents")).json(&body),
        &owner_b.to_string(),
        "owner-b-catalog",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "catalog create must reject foreign owner namespace"
    );

    // Verify DB unchanged - no agent created
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE owner_id = $1")
        .bind(owner_b)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "no agent should be created");

    server.cleanup().await;
}

/// Catalog create with a foreign legacy repository is refused (R-123.reply-1).
#[tokio::test]
#[serial]
async fn catalog_create_foreign_legacy_rejected() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "owner-a-legacy").await;
    let owner_b = insert_user(&server.db, "owner-b-legacy").await;
    let repo = format!("nasiko/agent-{}", Uuid::new_v4());
    let legacy_image = format!("{}:1.0", repo);

    // Owner A has an agent using this legacy repo
    insert_agent(&server.db, "agent-a", owner_a, &legacy_image).await;

    // Owner B tries to register their own agent with the same legacy repo
    let body = serde_json::json!({
        "name": "agent-b",
        "display_name": "Agent B",
        "description": "Trying to use another owner's legacy repo",
        "image": legacy_image,
    });

    let resp = common::as_member(
        server.client.post(server.url("/api/agents")).json(&body),
        &owner_b.to_string(),
        "owner-b-legacy",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "catalog create must reject foreign legacy repository"
    );

    // Verify DB unchanged - no agent created
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE owner_id = $1")
        .bind(owner_b)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "no agent should be created");

    server.cleanup().await;
}

/// Admin deploy with unqualified foreign reference is refused before deployment (R-123).
#[tokio::test]
#[serial]
async fn admin_deploy_unqualified_foreign_rejected() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "owner-a-deploy").await;
    let owner_b = insert_user(&server.db, "owner-b-deploy").await;

    // Unqualified foreign reference
    let foreign_image = format!("u-{}/agent:1.0", owner_a.as_simple());

    let body = serde_json::json!({
        "name": "bad-agent",
        "image": foreign_image,
    });

    let resp = common::as_member(
        server
            .client
            .post(server.url("/api/containers"))
            .json(&body),
        &owner_b.to_string(),
        "owner-b-deploy",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "admin deploy must reject unqualified foreign reference"
    );

    // Verify DB unchanged - no agent deployed
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE owner_id = $1")
        .bind(owner_b)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "no agent should be deployed");

    server.cleanup().await;
}

/// Pull credential bound to an agent with a foreign image is denied (R-123).
#[tokio::test]
#[serial]
async fn pull_credential_foreign_image_rejected() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "owner-a-pull").await;
    let owner_b = insert_user(&server.db, "owner-b-pull").await;

    let owner_a_repo = format!("u-{}/agent-a", owner_a.as_simple());

    // Owner A's real image
    let owner_a_image = format!("{}:1.0", owner_a_repo);

    // Create agent for owner B but with owner A's image (simulating the bypass)
    let agent_b_id = insert_agent(&server.db, "agent-b", owner_b, &owner_a_image).await;
    insert_manifest(&server.db, &owner_a_repo).await;

    // Mint pull credential for owner B's agent
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_b_id)
        .await
        .unwrap()
        .expect("mint credential");

    // Try to use it to pull owner A's repository
    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", owner_a_repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "pull credential must not work for foreign namespace in bound image"
    );

    server.cleanup().await;
}

/// Connector pull credential works for its own repository (R-124).
#[tokio::test]
#[serial]
async fn connector_pull_credential_own_repo_allowed() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "connector-owner").await;
    let connector_name = format!("test-connector-{}", Uuid::new_v4().simple());
    let repo = format!("u-{}/mcp-{}", owner_id.as_simple(), connector_name);
    let image = format!("{}:1.0", repo);

    // Insert connector
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build') RETURNING id",
    )
    .bind(owner_id)
    .bind(&connector_name)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Insert successful build with image_tag
    sqlx::query(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, image_tag, status) \
         VALUES ($1, $2, '1.0.0', $3, 'success')",
    )
    .bind(connector_id)
    .bind(owner_id)
    .bind(&image)
    .execute(&server.db)
    .await
    .unwrap();

    insert_manifest(&server.db, &repo).await;

    // Mint pull credential
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, connector_id)
        .await
        .unwrap()
        .expect("mint connector credential");

    // Use it to pull the connector's own repo
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
        "connector pull credential must work for its own repository"
    );

    server.cleanup().await;
}

/// Connector pull credential is denied for foreign repository (R-124).
#[tokio::test]
#[serial]
async fn connector_pull_credential_foreign_repo_rejected() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "connector-owner-a").await;
    let owner_b = insert_user(&server.db, "connector-owner-b").await;
    let connector_name = format!("connector-{}", Uuid::new_v4().simple());
    let foreign_repo = format!("u-{}/agent", owner_a.as_simple());
    let connector_repo = format!("u-{}/mcp-{}", owner_b.as_simple(), connector_name);
    let connector_image = format!("{}:1.0", connector_repo);

    // Insert connector for owner B
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build') RETURNING id",
    )
    .bind(owner_b)
    .bind(&connector_name)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Build with proper image
    sqlx::query(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, image_tag, status) \
         VALUES ($1, $2, '1.0.0', $3, 'success')",
    )
    .bind(connector_id)
    .bind(owner_b)
    .bind(&connector_image)
    .execute(&server.db)
    .await
    .unwrap();

    insert_manifest(&server.db, &foreign_repo).await;

    // Mint pull credential for connector
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, connector_id)
        .await
        .unwrap()
        .expect("mint credential");

    // Try to use it to pull owner A's repo
    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", foreign_repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "connector credential must not work for foreign repository"
    );

    server.cleanup().await;
}

/// External registry images are allowed for any user (positive control).
#[tokio::test]
#[serial]
async fn catalog_create_external_registry_allowed() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "external-user").await;
    let external_image = "docker.io/library/nginx:latest";

    let body = serde_json::json!({
        "name": "external-agent",
        "display_name": "External Agent",
        "description": "Using external registry",
        "image": external_image,
    });

    let resp = common::as_member(
        server.client.post(server.url("/api/agents")).json(&body),
        &owner_id.to_string(),
        "external-user",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        201,
        "catalog create must allow external registry images"
    );

    server.cleanup().await;
}

/// Superuser can create agents with any namespace (positive control).
#[tokio::test]
#[serial]
async fn catalog_create_superuser_unrestricted() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "owner-a-super").await;
    let admin_id = insert_superuser(&server.db, "admin-super").await;
    let any_image = format!("u-{}/agent:1.0", owner_a.as_simple());

    let body = serde_json::json!({
        "name": "super-agent",
        "display_name": "Super Agent",
        "description": "Created by superuser",
        "image": any_image,
    });

    let resp = common::as_superuser(
        server.client.post(server.url("/api/agents")).json(&body),
        &admin_id.to_string(),
        "admin-super",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        201,
        "superuser must be able to create agents with any namespace"
    );

    server.cleanup().await;
}

// ─── R-123.reply-4: External image aliases must not grant platform repository ownership ────

/// Owner B can register an external image that aliases A's private legacy repository name,
/// but this must not grant B access to A's platform repository (R-123.reply-4).
#[tokio::test]
#[serial]
async fn external_alias_does_not_grant_platform_repository_ownership() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "owner-a-external-alias").await;
    let owner_b = insert_user(&server.db, "owner-b-external-alias").await;

    // Owner A has a private legacy repository
    let legacy_repo = format!("nasiko/private-agent-{}", Uuid::new_v4().simple());
    let platform_image = format!("{}:1.0", legacy_repo);
    insert_agent(&server.db, "agent-a", owner_a, &platform_image).await;
    insert_manifest(&server.db, &legacy_repo).await;

    // Owner B registers an external image that happens to have the same repository path
    let external_alias = format!("external.example/{}:1.0", legacy_repo);
    let body = serde_json::json!({
        "name": "agent-b-external",
        "display_name": "Agent B External",
        "description": "Uses external registry",
        "image": external_alias,
    });

    let resp = common::as_member(
        server.client.post(server.url("/api/agents")).json(&body),
        &owner_b.to_string(),
        "owner-b-external-alias",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        201,
        "external image registration must be allowed"
    );

    // Owner B's session read of the PLATFORM repository must still be refused
    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", legacy_repo))),
        &owner_b.to_string(),
        "owner-b-external-alias",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "external alias must not grant read access to platform repository"
    );

    // Owner A's read must still be allowed
    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/{}/tags/list", legacy_repo))),
        &owner_a.to_string(),
        "owner-a-external-alias",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "original owner must still have read access"
    );

    // Owner A's update must not be refused as "shared"
    let body = serde_json::json!({
        "name": "agent-a-updated",
        "display_name": "Agent A Updated",
        "description": "Updating legacy repository",
        "image": platform_image,
    });

    let resp = common::as_member(
        server.client.post(server.url("/api/agents")).json(&body),
        &owner_a.to_string(),
        "owner-a-external-alias",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        201,
        "owner A must still be able to use their legacy repository"
    );

    server.cleanup().await;
}

/// A pull credential bound to an agent with an external image must not work for
/// the platform repository with the same path (R-123.reply-4).
#[tokio::test]
#[serial]
async fn pull_credential_external_image_refused_for_platform_repo() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "owner-a-pull-ext").await;
    let owner_b = insert_user(&server.db, "owner-b-pull-ext").await;

    // Owner A has a platform repository
    let platform_repo = format!("nasiko/agent-{}", Uuid::new_v4().simple());
    let platform_image = format!("{}:1.0", platform_repo);
    insert_agent(&server.db, "agent-a-platform", owner_a, &platform_image).await;
    insert_manifest(&server.db, &platform_repo).await;

    // Owner B has an agent with an external image that aliases the same repository path
    let external_image = format!("external.example/{}:1.0", platform_repo);
    let agent_b_id = insert_agent(&server.db, "agent-b-ext", owner_b, &external_image).await;

    // Mint pull credential for owner B's agent
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, agent_b_id)
        .await
        .unwrap()
        .expect("mint credential");

    // Try to use it to pull the platform repository
    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", platform_repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "pull credential for external image must not work for platform repository"
    );

    server.cleanup().await;
}

// ─── R-124.reply-3: Connector first-pull authorization ────────────────────────

/// A connector's first pull (before the build is marked successful) must be allowed
/// if it pulls its own repository (R-124.reply-3).
#[tokio::test]
#[serial]
async fn connector_first_pull_own_repo_allowed() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "connector-first-pull").await;
    let connector_name = format!("test-connector-{}", Uuid::new_v4().simple());
    let repo = format!("u-{}/mcp-{}", owner_id.as_simple(), connector_name);
    let image = format!("{}:1.0", repo);

    // Insert connector
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build') RETURNING id",
    )
    .bind(owner_id)
    .bind(&connector_name)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Insert a pending build with image_tag (as queue_upload now does)
    sqlx::query(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, image_tag, status) \
         VALUES ($1, $2, '1.0.0', $3, 'pending')",
    )
    .bind(connector_id)
    .bind(owner_id)
    .bind(&image)
    .execute(&server.db)
    .await
    .unwrap();

    insert_manifest(&server.db, &repo).await;

    // Mint pull credential
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, connector_id)
        .await
        .unwrap()
        .expect("mint connector credential");

    // The first pull (while still pending) must be allowed
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
        "connector first pull of its own repository must be allowed"
    );

    server.cleanup().await;
}

/// A connector's pull credential must be refused for another owner's repository (R-124).
#[tokio::test]
#[serial]
async fn connector_pull_foreign_owner_repo_rejected() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "connector-owner-a-foreign").await;
    let owner_b = insert_user(&server.db, "connector-owner-b-foreign").await;
    let connector_name = format!("connector-{}", Uuid::new_v4().simple());
    let connector_repo = format!("u-{}/mcp-{}", owner_b.as_simple(), connector_name);
    let connector_image = format!("{}:1.0", connector_repo);
    let foreign_repo = format!("u-{}/agent", owner_a.as_simple());

    // Insert connector for owner B
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build') RETURNING id",
    )
    .bind(owner_b)
    .bind(&connector_name)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Build with proper image
    sqlx::query(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, image_tag, status) \
         VALUES ($1, $2, '1.0.0', $3, 'pending')",
    )
    .bind(connector_id)
    .bind(owner_b)
    .bind(&connector_image)
    .execute(&server.db)
    .await
    .unwrap();

    insert_manifest(&server.db, &foreign_repo).await;

    // Mint pull credential for connector
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, connector_id)
        .await
        .unwrap()
        .expect("mint credential");

    // Try to use it to pull owner A's repo
    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", foreign_repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "connector credential must not work for foreign owner repository"
    );

    server.cleanup().await;
}

/// A connector's pull credential must be refused for another connector's repository (R-124).
#[tokio::test]
#[serial]
async fn connector_pull_other_connector_repo_rejected() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "connector-owner-other").await;
    let connector_a_name = format!("connector-a-{}", Uuid::new_v4().simple());
    let connector_b_name = format!("connector-b-{}", Uuid::new_v4().simple());
    let connector_a_repo = format!("u-{}/mcp-{}", owner_id.as_simple(), connector_a_name);
    let connector_a_image = format!("{}:1.0", connector_a_repo);
    let connector_b_repo = format!("u-{}/mcp-{}", owner_id.as_simple(), connector_b_name);
    let connector_b_image = format!("{}:1.0", connector_b_repo);

    // Insert connector A
    let connector_a_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build') RETURNING id",
    )
    .bind(owner_id)
    .bind(&connector_a_name)
    .fetch_one(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, image_tag, status) \
         VALUES ($1, $2, '1.0.0', $3, 'pending')",
    )
    .bind(connector_a_id)
    .bind(owner_id)
    .bind(&connector_a_image)
    .execute(&server.db)
    .await
    .unwrap();

    // Insert connector B
    let connector_b_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build') RETURNING id",
    )
    .bind(owner_id)
    .bind(&connector_b_name)
    .fetch_one(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, image_tag, status) \
         VALUES ($1, $2, '1.0.0', $3, 'pending')",
    )
    .bind(connector_b_id)
    .bind(owner_id)
    .bind(&connector_b_image)
    .execute(&server.db)
    .await
    .unwrap();

    insert_manifest(&server.db, &connector_b_repo).await;

    // Mint pull credential for connector A
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, connector_a_id)
        .await
        .unwrap()
        .expect("mint credential");

    // Try to use it to pull connector B's repo
    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", connector_b_repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "connector credential must not work for another connector's repository"
    );

    server.cleanup().await;
}

/// A connector with no build row must not be granted pull access (R-124).
#[tokio::test]
#[serial]
async fn connector_no_build_row_rejected() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "connector-no-build").await;
    let connector_name = format!("connector-{}", Uuid::new_v4().simple());
    let repo = format!("u-{}/mcp-{}", owner_id.as_simple(), connector_name);

    // Insert connector without any build row
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build') RETURNING id",
    )
    .bind(owner_id)
    .bind(&connector_name)
    .fetch_one(&server.db)
    .await
    .unwrap();

    insert_manifest(&server.db, &repo).await;

    // Mint pull credential
    let cred = nasiko_oci::pull_credentials::get_or_create(&server.db, connector_id)
        .await
        .unwrap()
        .expect("mint credential");

    // Pull must be refused
    let resp = server
        .client
        .get(server.url(&format!("/v2/{}/tags/list", repo)))
        .basic_auth(&cred.username, Some(&cred.token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        403,
        "connector without build row must be refused"
    );

    server.cleanup().await;
}
