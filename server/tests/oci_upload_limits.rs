//! Regression test for the OCI chunked-blob-upload total-size cap (SEC fix
//! #8): `ops::append_chunk`'s in-memory upload buffer (`DashMap<Uuid,
//! BytesMut>`) previously grew without any bound across the lifetime of one
//! upload session — only a single chunk's size was capped (512 MiB, at the
//! HTTP body-read layer). A client could OOM the process by sending an
//! unbounded number of chunks to the same upload session.
//!
//! Rather than actually transferring gigabytes of data (impractical in CI),
//! this test seeds `oci_uploads.offset_bytes` directly to a value already
//! past any reasonable cap, then sends one small chunk — `append_chunk` reads
//! `current_offset` from that same DB column, so this exercises the exact
//! `new_offset > MAX_UPLOAD_TOTAL_BYTES` check with a single small HTTP
//! request.
//!
//! Requires infra (Postgres): `cargo test -p nasiko-server --test oci_upload_limits -- --test-threads=1`

mod common;

use uuid::Uuid;

async fn insert_user(db: &sqlx::PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@oci-upload-test.com"))
    .fetch_one(db)
    .await
    .expect("insert test user")
}

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

#[tokio::test]
#[serial_test::serial]
async fn chunked_upload_rejects_once_total_size_cap_exceeded() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-upload-owner").await;
    let agent_name = format!("oci-upload-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    // Initiate an upload session for the owner's claimed repo.
    let init = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/nasiko/{agent_name}/blobs/uploads/"))),
        &owner_id.to_string(),
        "oci-upload-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(init.status(), 202, "initiate_upload should succeed");
    let upload_uuid_str = init
        .headers()
        .get("Docker-Upload-UUID")
        .expect("Docker-Upload-UUID header present")
        .to_str()
        .unwrap()
        .to_string();
    let upload_uuid: Uuid = upload_uuid_str.parse().unwrap();

    // Seed the session's recorded offset far past any reasonable total-size
    // cap (implementation cap is 5 GiB) — avoids actually transferring
    // gigabytes of data to exercise the same code path `append_chunk` reads
    // (`current_offset` comes straight from this column).
    let six_gib: i64 = 6 * 1024 * 1024 * 1024;
    sqlx::query("UPDATE oci_uploads SET offset_bytes = $1 WHERE uuid = $2")
        .bind(six_gib)
        .bind(upload_uuid)
        .execute(&server.db)
        .await
        .unwrap();

    // One more small chunk must now be rejected outright.
    let patch = common::as_member(
        server.client.patch(server.url(&format!(
            "/v2/nasiko/{agent_name}/blobs/uploads/{upload_uuid}"
        ))),
        &owner_id.to_string(),
        "oci-upload-owner",
    )
    .body(vec![1u8; 16])
    .send()
    .await
    .unwrap();

    assert_eq!(
        patch.status(),
        400,
        "a chunk pushing the session's total past the cap must be rejected"
    );

    // The upload session must be torn down (not left dangling) on overflow.
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oci_uploads WHERE uuid = $1")
        .bind(upload_uuid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        remaining, 0,
        "overflowed upload session must be deleted, not left dangling"
    );

    server.cleanup().await;
}

/// `complete_upload`'s final chunk previously bypassed the total-size cap
/// entirely — a client could PATCH up to just under the limit, then finalize
/// with one more chunk, pushing the accumulated size past the cap
/// unchecked. Same seeding trick as the PATCH test above: seed
/// `offset_bytes` directly (the column `complete_upload` now reads for this
/// check) rather than actually transferring gigabytes.
#[tokio::test]
#[serial_test::serial]
async fn complete_upload_rejects_final_chunk_that_pushes_past_total_cap() {
    let server = common::TestServer::start().await;
    let owner_id = insert_user(&server.db, "oci-complete-owner").await;
    let agent_name = format!("oci-complete-agent-{}", Uuid::new_v4());
    insert_agent(&server.db, &agent_name, owner_id).await;

    let init = common::as_member(
        server
            .client
            .post(server.url(&format!("/v2/nasiko/{agent_name}/blobs/uploads/"))),
        &owner_id.to_string(),
        "oci-complete-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(init.status(), 202);
    let upload_uuid_str = init
        .headers()
        .get("Docker-Upload-UUID")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let upload_uuid: Uuid = upload_uuid_str.parse().unwrap();

    // Seed the session as if PATCH chunks had already accumulated just under
    // the 5 GiB cap.
    let just_under_cap: i64 = 5 * 1024 * 1024 * 1024 - 10;
    sqlx::query("UPDATE oci_uploads SET offset_bytes = $1 WHERE uuid = $2")
        .bind(just_under_cap)
        .bind(upload_uuid)
        .execute(&server.db)
        .await
        .unwrap();

    // Finalize with one more chunk that pushes the total over the cap.
    let complete = common::as_member(
        server.client.put(server.url(&format!(
            "/v2/nasiko/{agent_name}/blobs/uploads/{upload_uuid}"
        ))),
        &owner_id.to_string(),
        "oci-complete-owner",
    )
    .body(vec![1u8; 100])
    .send()
    .await
    .unwrap();

    assert_eq!(
        complete.status(),
        400,
        "a final chunk pushing the session's total past the cap must be rejected, not silently finalized"
    );

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oci_uploads WHERE uuid = $1")
        .bind(upload_uuid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        remaining, 0,
        "overflowed upload session must be deleted, not left dangling"
    );

    server.cleanup().await;
}
