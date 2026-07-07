//! Regression tests for OCI manifest pull-by-digest integrity verification
//! (SEC fix #7): `get_manifest` must recompute the SHA-256 of the stored
//! `content` and verify it matches the requested digest whenever the caller
//! addressed the manifest BY DIGEST (not by a mutable tag) — defending
//! against a DB row whose `digest` column doesn't actually match its own
//! `content` (row-level corruption, or a future code path that updates one
//! without the other).
//!
//! Requires infra (Postgres): `cargo test -p nasiko-server --test oci_manifest_digest -- --test-threads=1`

mod common;

use uuid::Uuid;

async fn insert_user(db: &sqlx::PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@oci-digest-test.com"))
    .fetch_one(db)
    .await
    .expect("insert test user")
}

async fn insert_agent(db: &sqlx::PgPool, name: &str, owner_id: Uuid) {
    sqlx::query("INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', 'img:1')")
        .bind(name)
        .bind(owner_id)
        .execute(db)
        .await
        .expect("insert test agent");
}

/// Push-by-digest verification: PUTting a manifest under a digest reference
/// that doesn't actually match the pushed body's own hash must be rejected
/// up front (400), not stored and left to 500 on a later pull.
#[tokio::test]
#[serial_test::serial]
async fn put_manifest_by_digest_rejects_mismatched_reference() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-push-digest-owner").await;
    let agent_name = format!("oci-push-digest-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    let wrong_digest = format!("sha256:{}", "1".repeat(64));
    let body = serde_json::json!({"schemaVersion": 2});

    let resp = common::as_member(
        server.client.put(server.url(&format!("/v2/nasiko/{agent_name}/manifests/{wrong_digest}"))),
        &owner_id.to_string(), "oci-push-digest-owner",
    )
    .header("content-type", "application/vnd.oci.image.manifest.v1+json")
    .json(&body)
    .send().await.unwrap();

    assert_eq!(resp.status(), 400, "a digest reference that doesn't match the pushed body must be rejected");

    server.cleanup().await;
}

/// Sanity: pushing by the CORRECT digest must still succeed (no false
/// positive), and pushing by a mutable tag is never subject to this check.
#[tokio::test]
#[serial_test::serial]
async fn put_manifest_by_matching_digest_succeeds() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-push-digest-owner-ok").await;
    let agent_name = format!("oci-push-digest-agent-ok-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    let body = serde_json::json!({"schemaVersion": 2});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, &body_bytes);
    let hex_digest: String = sha2::Digest::finalize(hasher).iter().map(|b| format!("{b:02x}")).collect();
    let real_digest = format!("sha256:{hex_digest}");

    let resp = common::as_member(
        server.client.put(server.url(&format!("/v2/nasiko/{agent_name}/manifests/{real_digest}"))),
        &owner_id.to_string(), "oci-push-digest-owner-ok",
    )
    .header("content-type", "application/vnd.oci.image.manifest.v1+json")
    .body(body_bytes)
    .send().await.unwrap();

    assert_eq!(resp.status(), 201, "a digest reference matching the pushed body must succeed");

    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn get_manifest_by_digest_rejects_content_digest_mismatch() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-digest-owner").await;
    let agent_name = format!("oci-digest-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    let repo = format!("nasiko/{agent_name}");

    // A manifest row whose `digest` column does NOT match its own `content` —
    // simulating corruption / a bug that updates one without the other.
    let bogus_digest = format!("sha256:{}", "0".repeat(64));
    sqlx::query(
        "INSERT INTO oci_manifests (digest, repository, reference, media_type, content, size_bytes)
         VALUES ($1, $2, $1, 'application/vnd.oci.image.manifest.v1+json', '{\"real\":true}', 2)",
    )
    .bind(&bogus_digest)
    .bind(&repo)
    .execute(&server.db)
    .await
    .unwrap();

    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{agent_name}/manifests/{bogus_digest}"))),
        &owner_id.to_string(),
        "oci-digest-owner",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        500,
        "a manifest whose content doesn't hash to the requested digest must fail closed, not return mismatched content"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn get_manifest_by_matching_digest_succeeds() {
    // Sanity: a correctly-consistent row (digest really is sha256(content))
    // must still be servable — the fix must not false-positive on the
    // common case.
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-digest-owner-ok").await;
    let agent_name = format!("oci-digest-agent-ok-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    let repo = format!("nasiko/{agent_name}");

    let content = r#"{"schemaVersion":2}"#;
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, content.as_bytes());
    let digest_bytes = sha2::Digest::finalize(hasher);
    let hex_digest: String = digest_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let real_digest = format!("sha256:{hex_digest}");

    sqlx::query(
        "INSERT INTO oci_manifests (digest, repository, reference, media_type, content, size_bytes)
         VALUES ($1, $2, $1, 'application/vnd.oci.image.manifest.v1+json', $3, $4)",
    )
    .bind(&real_digest)
    .bind(&repo)
    .bind(content)
    .bind(content.len() as i64)
    .execute(&server.db)
    .await
    .unwrap();

    let resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{agent_name}/manifests/{real_digest}"))),
        &owner_id.to_string(),
        "oci-digest-owner-ok",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "a self-consistent manifest row must still be servable by digest");

    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn get_manifest_by_tag_is_not_digest_verified() {
    // Tag references (mutable pointers) must NOT be subject to the
    // digest-equality check — only reference values that ARE themselves a
    // content digest are.
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-digest-owner-tag").await;
    let agent_name = format!("oci-digest-agent-tag-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;
    let repo = format!("nasiko/{agent_name}");

    let real_digest = format!("sha256:{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO oci_manifests (digest, repository, reference, media_type, content, size_bytes)
         VALUES ($1, $2, 'latest', 'application/vnd.oci.image.manifest.v1+json', '{}', 2)",
    )
    .bind(&real_digest)
    .bind(&repo)
    .execute(&server.db)
    .await
    .unwrap();

    let resp = common::as_member(
        server.client.get(server.url(&format!("/v2/nasiko/{agent_name}/manifests/latest"))),
        &owner_id.to_string(),
        "oci-digest-owner-tag",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "fetching by tag must succeed regardless of whether content hashes to any particular digest"
    );

    server.cleanup().await;
}
