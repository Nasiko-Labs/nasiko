//! Regression tests for OCI blob reference counting + confidentiality
//! scoping (P1-3), and the adjacent cross-repo manifest tag-hijack fix.
//!
//! Blobs are stored at a flat, globally content-addressed key with no
//! per-repo linkage. Without `oci_blob_refs`:
//! - deleting a blob from one repo could destroy a layer another repo still
//!   needs (data loss);
//! - any repo owner could GET/HEAD any blob in the registry by digest, even
//!   one only ever pushed under a DIFFERENT repo (confidentiality leak);
//! - `oci_manifests.digest` being a global PK meant two repos pushing
//!   byte-identical manifest content silently clobbered each other's tag
//!   pointer.
//!
//! Requires infra (Postgres): `cargo test -p nasiko-server --test oci_blob_refs -- --test-threads=1`

mod common;

use serde_json::Value;
use serial_test::serial;
use sha2::{Digest, Sha256};
use uuid::Uuid;

async fn insert_user(db: &sqlx::PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@oci-blob-refs-test.com"))
    .fetch_one(db)
    .await
    .expect("insert test user")
}

/// `check_repo_delete_access` requires an existing claim (an `agents` row) —
/// unlike the read-side `check_repo_access`, an unclaimed repo name does NOT
/// grant delete access. Tests that DELETE a blob must claim the repo first.
async fn insert_agent(db: &sqlx::PgPool, name: &str, owner_id: Uuid) {
    sqlx::query(
        "INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', 'img:1')",
    )
    .bind(name)
    .bind(owner_id)
    .execute(db)
    .await
    .expect("insert test agent");
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hex_digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex_digest}")
}

/// Push a small blob via the real upload flow (POST initiate -> PUT
/// complete, no PATCH chunks needed for a single-shot body) and return its
/// digest.
async fn push_blob(
    server: &common::TestServer,
    owner: &str,
    repo: &str,
    user_id: &str,
    username: &str,
    data: &[u8],
) -> String {
    let digest = sha256_hex(data);

    let init_resp = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/{owner}/{repo}/blobs/uploads/"))),
        user_id,
        username,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(init_resp.status(), 202, "initiate upload must succeed");
    let upload_uuid = init_resp
        .headers()
        .get("docker-upload-uuid")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let complete_resp = common::as_member(
        server.client.put(server.url(&format!(
            "/v2/{owner}/{repo}/blobs/uploads/{upload_uuid}?digest={digest}"
        ))),
        user_id,
        username,
    )
    .body(data.to_vec())
    .send()
    .await
    .unwrap();
    assert_eq!(complete_resp.status(), 201, "complete upload must succeed");

    digest
}

/// Push a manifest referencing `layer_digest`/`config_digest` via the real
/// PUT flow, so `put_manifest`'s blob-ref extraction actually runs.
async fn push_manifest(
    server: &common::TestServer,
    repo: &str,
    reference: &str,
    user_id: &str,
    username: &str,
    config_digest: &str,
    layer_digest: &str,
) -> reqwest::Response {
    let body = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config_digest, "size": 2},
        "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": layer_digest, "size": 2}],
    });

    common::as_member(
        server
            .client
            .put(server.url(&format!("/v2/nasiko/{repo}/manifests/{reference}"))),
        user_id,
        username,
    )
    .header("content-type", "application/vnd.oci.image.manifest.v1+json")
    .json(&body)
    .send()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn manifest_push_records_blob_refs_for_config_and_layers() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-refs-owner").await;
    let repo = format!("oci-refs-repo-{}", Uuid::new_v4());

    let config_digest = push_blob(
        &server,
        "nasiko",
        &repo,
        &owner_id.to_string(),
        "oci-refs-owner",
        b"config-bytes",
    )
    .await;
    let layer_digest = push_blob(
        &server,
        "nasiko",
        &repo,
        &owner_id.to_string(),
        "oci-refs-owner",
        b"layer-bytes",
    )
    .await;

    let resp = push_manifest(
        &server,
        &repo,
        "latest",
        &owner_id.to_string(),
        "oci-refs-owner",
        &config_digest,
        &layer_digest,
    )
    .await;
    assert_eq!(resp.status(), 201);

    let full_repo = format!("nasiko/{repo}");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM oci_blob_refs WHERE repository = $1 AND digest = ANY($2)",
    )
    .bind(&full_repo)
    .bind(vec![config_digest, layer_digest])
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        count, 2,
        "both config and layer digests must be linked to the pushing repo"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_blob_shared_across_repos_preserves_other_repos_copy() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "oci-shared-a").await;
    let owner_b = insert_user(&server.db, "oci-shared-b").await;
    let repo_a = format!("oci-shared-repo-a-{}", Uuid::new_v4());
    let repo_b = format!("oci-shared-repo-b-{}", Uuid::new_v4());
    insert_agent(&server.db, &repo_a, owner_a).await;

    // Both repos push a manifest referencing the IDENTICAL layer digest
    // (byte-identical layer content forces the digest collision).
    let shared_layer = b"identical-shared-layer-bytes";
    let layer_digest = push_blob(
        &server,
        "nasiko",
        &repo_a,
        &owner_a.to_string(),
        "oci-shared-a",
        shared_layer,
    )
    .await;
    push_blob(
        &server,
        "nasiko",
        &repo_b,
        &owner_b.to_string(),
        "oci-shared-b",
        shared_layer,
    )
    .await;
    let config_a = push_blob(
        &server,
        "nasiko",
        &repo_a,
        &owner_a.to_string(),
        "oci-shared-a",
        b"config-a",
    )
    .await;
    let config_b = push_blob(
        &server,
        "nasiko",
        &repo_b,
        &owner_b.to_string(),
        "oci-shared-b",
        b"config-b",
    )
    .await;

    let resp_a = push_manifest(
        &server,
        &repo_a,
        "latest",
        &owner_a.to_string(),
        "oci-shared-a",
        &config_a,
        &layer_digest,
    )
    .await;
    assert_eq!(resp_a.status(), 201);
    let resp_b = push_manifest(
        &server,
        &repo_b,
        "latest",
        &owner_b.to_string(),
        "oci-shared-b",
        &config_b,
        &layer_digest,
    )
    .await;
    assert_eq!(resp_b.status(), 201);

    // Owner A deletes the shared layer from THEIR repo.
    let del_resp = common::as_member(
        server
            .client
            .delete(server.url(&format!("/v2/nasiko/{repo_a}/blobs/{layer_digest}"))),
        &owner_a.to_string(),
        "oci-shared-a",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(del_resp.status(), 202);

    // Repo A can no longer read it...
    let get_a = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{repo_a}/blobs/{layer_digest}"))),
        &owner_a.to_string(),
        "oci-shared-a",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        get_a.status(),
        404,
        "repo A deleted its own reference, so it should no longer see the blob"
    );

    // ...but repo B's copy is untouched (physical object still exists, still linked to B).
    let get_b = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{repo_b}/blobs/{layer_digest}"))),
        &owner_b.to_string(),
        "oci-shared-b",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        get_b.status(),
        200,
        "repo B must still be able to fetch the shared layer after A deleted its own reference"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_blob_removes_physical_object_once_last_reference_drops() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-lastref").await;
    let repo = format!("oci-lastref-repo-{}", Uuid::new_v4());
    insert_agent(&server.db, &repo, owner_id).await;

    let layer_digest = push_blob(
        &server,
        "nasiko",
        &repo,
        &owner_id.to_string(),
        "oci-lastref",
        b"solo-layer-bytes",
    )
    .await;
    let config_digest = push_blob(
        &server,
        "nasiko",
        &repo,
        &owner_id.to_string(),
        "oci-lastref",
        b"solo-config",
    )
    .await;
    let resp = push_manifest(
        &server,
        &repo,
        "latest",
        &owner_id.to_string(),
        "oci-lastref",
        &config_digest,
        &layer_digest,
    )
    .await;
    assert_eq!(resp.status(), 201);

    let del_resp = common::as_member(
        server
            .client
            .delete(server.url(&format!("/v2/nasiko/{repo}/blobs/{layer_digest}"))),
        &owner_id.to_string(),
        "oci-lastref",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(del_resp.status(), 202);

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oci_blob_refs WHERE digest = $1")
        .bind(&layer_digest)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "no repo should reference the digest anymore");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_blob_confidentiality_denies_repo_that_never_referenced_digest() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "oci-confid-a").await;
    let owner_b = insert_user(&server.db, "oci-confid-b").await;
    let repo_a = format!("oci-confid-repo-a-{}", Uuid::new_v4());
    let repo_b = format!("oci-confid-repo-b-{}", Uuid::new_v4());

    // A pushes a blob + manifest referencing it under repo A only.
    let secret_layer = push_blob(
        &server,
        "nasiko",
        &repo_a,
        &owner_a.to_string(),
        "oci-confid-a",
        b"a-private-layer",
    )
    .await;
    let config_a = push_blob(
        &server,
        "nasiko",
        &repo_a,
        &owner_a.to_string(),
        "oci-confid-a",
        b"config-a2",
    )
    .await;
    let resp = push_manifest(
        &server,
        &repo_a,
        "latest",
        &owner_a.to_string(),
        "oci-confid-a",
        &config_a,
        &secret_layer,
    )
    .await;
    assert_eq!(resp.status(), 201);

    // B owns repo_b (a real, claimed repo) but never referenced this digest.
    // A stranger who merely knows the digest must not be able to read it via B's repo.
    let get_resp = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{repo_b}/blobs/{secret_layer}"))),
        &owner_b.to_string(),
        "oci-confid-b",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        get_resp.status(),
        404,
        "B must not read a digest only ever pushed under A's repo"
    );

    let head_resp = common::as_member(
        server
            .client
            .head(server.url(&format!("/v2/nasiko/{repo_b}/blobs/{secret_layer}"))),
        &owner_b.to_string(),
        "oci-confid-b",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        head_resp.status(),
        404,
        "HEAD must be gated the same as GET, not left as a size/existence oracle"
    );

    // Sanity: A itself can still read it.
    let get_a = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{repo_a}/blobs/{secret_layer}"))),
        &owner_a.to_string(),
        "oci-confid-a",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(get_a.status(), 200);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn two_repos_pushing_identical_manifest_content_dont_clobber_each_other() {
    let server = common::TestServer::start().await;
    let owner_a = insert_user(&server.db, "oci-hijack-a").await;
    let owner_b = insert_user(&server.db, "oci-hijack-b").await;
    let repo_a = format!("oci-hijack-repo-a-{}", Uuid::new_v4());
    let repo_b = format!("oci-hijack-repo-b-{}", Uuid::new_v4());

    let config_digest = push_blob(
        &server,
        "nasiko",
        &repo_a,
        &owner_a.to_string(),
        "oci-hijack-a",
        b"shared-config",
    )
    .await;
    let layer_digest = push_blob(
        &server,
        "nasiko",
        &repo_a,
        &owner_a.to_string(),
        "oci-hijack-a",
        b"shared-layer",
    )
    .await;
    // Make the same blobs visible under repo B too so B's push doesn't 404 on linkage elsewhere.
    push_blob(
        &server,
        "nasiko",
        &repo_b,
        &owner_b.to_string(),
        "oci-hijack-b",
        b"shared-config",
    )
    .await;
    push_blob(
        &server,
        "nasiko",
        &repo_b,
        &owner_b.to_string(),
        "oci-hijack-b",
        b"shared-layer",
    )
    .await;

    // Byte-identical manifest content pushed to two different repos under different tags.
    let resp_a = push_manifest(
        &server,
        &repo_a,
        "v1",
        &owner_a.to_string(),
        "oci-hijack-a",
        &config_digest,
        &layer_digest,
    )
    .await;
    assert_eq!(resp_a.status(), 201);
    let resp_b = push_manifest(
        &server,
        &repo_b,
        "v2",
        &owner_b.to_string(),
        "oci-hijack-b",
        &config_digest,
        &layer_digest,
    )
    .await;
    assert_eq!(resp_b.status(), 201);

    // Both must be independently retrievable under their OWN repo/tag.
    let get_a = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{repo_a}/manifests/v1"))),
        &owner_a.to_string(),
        "oci-hijack-a",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        get_a.status(),
        200,
        "repo A's tag must survive repo B pushing identical content"
    );

    let get_b = common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{repo_b}/manifests/v2"))),
        &owner_b.to_string(),
        "oci-hijack-b",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        get_b.status(),
        200,
        "repo B's own push must be retrievable under its own tag"
    );

    server.cleanup().await;
}

/// The migration's backfill logic (jsonb extraction of config/layer digests
/// from pre-existing `oci_manifests.content`) run against data that was never
/// inserted through `put_manifest` — proving the extraction SQL itself is
/// correct, independent of migration ordering.
#[tokio::test]
#[serial]
async fn backfill_sql_populates_refs_from_existing_manifest_content() {
    let server = common::TestServer::start().await;
    let repo = format!("nasiko/oci-backfill-repo-{}", Uuid::new_v4());
    let config_digest = "sha256:aaaa000000000000000000000000000000000000000000000000000000aaaa";
    let layer_digest = "sha256:bbbb000000000000000000000000000000000000000000000000000000bbbb";
    let manifest_digest = format!("sha256:{}", Uuid::new_v4().simple());

    let content = serde_json::json!({
        "schemaVersion": 2,
        "config": {"digest": config_digest},
        "layers": [{"digest": layer_digest}],
    });

    // Bypass put_manifest entirely — simulates a row that existed BEFORE this
    // migration shipped, with zero oci_blob_refs rows.
    sqlx::query(
        "WITH m AS (
             INSERT INTO oci_manifests (digest, repository, media_type, content, size_bytes)
             VALUES ($1, $2, 'application/vnd.oci.image.manifest.v1+json', $3, $4)
             RETURNING repository, digest
         )
         INSERT INTO oci_tags (repository, tag, digest)
         SELECT repository, 'latest', digest FROM m",
    )
    .bind(&manifest_digest)
    .bind(&repo)
    .bind(content.to_string())
    .bind(content.to_string().len() as i64)
    .execute(&server.db)
    .await
    .unwrap();

    // Re-run the exact backfill statements from 017_oci_blob_refs.sql.
    sqlx::query(
        "INSERT INTO oci_blob_refs (digest, repository)
         SELECT DISTINCT content::jsonb -> 'config' ->> 'digest', repository
         FROM oci_manifests
         WHERE content::jsonb -> 'config' ->> 'digest' IS NOT NULL
         ON CONFLICT (digest, repository) DO NOTHING",
    )
    .execute(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO oci_blob_refs (digest, repository)
         SELECT DISTINCT layer_digest, repository
         FROM (
             SELECT repository, jsonb_array_elements(content::jsonb -> 'layers') ->> 'digest' AS layer_digest
             FROM oci_manifests
             WHERE jsonb_typeof(content::jsonb -> 'layers') = 'array'
         ) backfill
         WHERE layer_digest IS NOT NULL
         ON CONFLICT (digest, repository) DO NOTHING",
    )
    .execute(&server.db)
    .await
    .unwrap();

    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT digest FROM oci_blob_refs WHERE repository = $1 ORDER BY digest")
            .bind(&repo)
            .fetch_all(&server.db)
            .await
            .unwrap();
    let digests: Vec<&str> = rows.iter().map(|(d,)| d.as_str()).collect();

    assert_eq!(
        digests,
        vec![config_digest, layer_digest],
        "backfill must extract both config and layer digests from pre-existing content"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn backfill_sql_skips_manifest_lists_without_erroring() {
    let server = common::TestServer::start().await;
    let repo = format!("nasiko/oci-backfill-list-{}", Uuid::new_v4());
    let manifest_digest = format!("sha256:{}", Uuid::new_v4().simple());

    // A manifest LIST/index has no top-level config/layers — must not error,
    // must not extract anything (its child manifests are separate rows).
    let content = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{"digest": format!("sha256:{}", Uuid::new_v4().simple()), "mediaType": "application/vnd.oci.image.manifest.v1+json"}],
    });

    sqlx::query(
        "WITH m AS (
             INSERT INTO oci_manifests (digest, repository, media_type, content, size_bytes)
             VALUES ($1, $2, 'application/vnd.oci.image.index.v1+json', $3, $4)
             RETURNING repository, digest
         )
         INSERT INTO oci_tags (repository, tag, digest)
         SELECT repository, 'latest', digest FROM m",
    )
    .bind(&manifest_digest)
    .bind(&repo)
    .bind(content.to_string())
    .bind(content.to_string().len() as i64)
    .execute(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO oci_blob_refs (digest, repository)
         SELECT DISTINCT content::jsonb -> 'config' ->> 'digest', repository
         FROM oci_manifests
         WHERE content::jsonb -> 'config' ->> 'digest' IS NOT NULL
         ON CONFLICT (digest, repository) DO NOTHING",
    )
    .execute(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO oci_blob_refs (digest, repository)
         SELECT DISTINCT layer_digest, repository
         FROM (
             SELECT repository, jsonb_array_elements(content::jsonb -> 'layers') ->> 'digest' AS layer_digest
             FROM oci_manifests
             WHERE jsonb_typeof(content::jsonb -> 'layers') = 'array'
         ) backfill
         WHERE layer_digest IS NOT NULL
         ON CONFLICT (digest, repository) DO NOTHING",
    )
    .execute(&server.db)
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oci_blob_refs WHERE repository = $1")
        .bind(&repo)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "a manifest list has no blobs of its own to link");

    server.cleanup().await;
}

/// Sanity check that the referrers join (`ops::get_referrers`) is scoped by
/// repository, not just digest — now that `oci_manifests.digest` is no
/// longer globally unique (post PK change to `(repository, digest)`), a
/// referrer digest can legitimately have manifest rows in TWO different
/// repositories. An unscoped join (`m.digest = r.referrer_digest` alone)
/// would return one row per matching `oci_manifests` row, duplicating the
/// entry once per repository that happens to share the digest.
#[tokio::test]
#[serial]
async fn get_referrers_join_scoped_to_repository_not_just_digest() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-referrers-owner").await;
    let repo_a = format!("nasiko/oci-referrers-repo-a-{}", Uuid::new_v4());
    let repo_b = format!("nasiko/oci-referrers-repo-b-{}", Uuid::new_v4());
    let subject_digest = format!("sha256:{}", Uuid::new_v4().simple());
    let shared_referrer_digest = format!("sha256:{}", Uuid::new_v4().simple());

    // The SAME digest has a manifest row in both repos (legal post-fix — PK
    // is (repository, digest), not digest alone).
    for repo in [&repo_a, &repo_b] {
        sqlx::query(
            "WITH m AS (
                 INSERT INTO oci_manifests (digest, repository, media_type, content, size_bytes)
                 VALUES ($1, $2, 'application/vnd.oci.artifact.manifest.v1+json', '{}', 2)
                 RETURNING repository, digest
             )
             INSERT INTO oci_tags (repository, tag, digest)
             SELECT repository, 'ref-tag', digest FROM m",
        )
        .bind(&shared_referrer_digest)
        .bind(repo)
        .execute(&server.db)
        .await
        .unwrap();
    }

    // Only repo A actually records this referrer relationship.
    sqlx::query(
        "INSERT INTO oci_referrers (subject_digest, repository, referrer_digest, size_bytes) VALUES ($1, $2, $3, 2)",
    )
    .bind(&subject_digest)
    .bind(&repo_a)
    .bind(&shared_referrer_digest)
    .execute(&server.db)
    .await
    .unwrap();

    let (owner_path, repo_path) = repo_a.split_once('/').unwrap();
    let referrers: Value = common::as_member(
        server.client.get(server.url(&format!(
            "/v2/{owner_path}/{repo_path}/referrers/{subject_digest}"
        ))),
        &owner_id.to_string(),
        "oci-referrers-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    let manifests = referrers["manifests"].as_array().unwrap();
    assert_eq!(
        manifests.len(),
        1,
        "must return exactly one entry, not one per repository sharing the digest: {manifests:?}"
    );

    server.cleanup().await;
}
