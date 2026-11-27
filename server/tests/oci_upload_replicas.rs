//! Chunked blob uploads across server replicas. A non-sticky load balancer
//! sends the POST, each PATCH and the closing PUT of one upload to whichever
//! replica it likes, so nothing about a session may live in one process. Each
//! test builds two independent `OciState`s over the same database and bucket,
//! standing in for two replicas, and splits one upload between them.
//!
//! Requires infra (Postgres + S3): `cargo test -p nasiko-server --test oci_upload_replicas -- --test-threads=1`

mod common;

use std::sync::Arc;

use bytes::Bytes;
use nasiko_oci::OciState;
use nasiko_oci::error::{OciCode, OciError};
use nasiko_oci::ops;
use uuid::Uuid;

/// One replica's registry: its own `OciState` over the shared pool and bucket.
async fn replica(server: &common::TestServer) -> OciState {
    let storage = nasiko_oci::storage::S3Storage::from_env("nasiko-test-artifacts".into()).await;
    OciState::new(server.db.clone(), Arc::new(storage), String::new())
}

fn repository() -> String {
    format!("nasiko/oci-replicas-{}", Uuid::new_v4())
}

async fn part_ids(server: &common::TestServer, upload_id: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT part_ids FROM oci_uploads WHERE uuid = $1")
        .bind(upload_id)
        .fetch_one(&server.db)
        .await
        .expect("read part ids")
}

/// The store key a staged chunk is written under (`ops::blobs::upload_part_key`).
fn part_key(upload_id: Uuid, part_id: Uuid) -> String {
    format!("uploads:{upload_id}:{part_id}")
}

#[tokio::test]
#[serial_test::serial]
async fn upload_split_across_replicas_assembles_the_whole_blob() {
    let server = common::TestServer::start().await;
    let (a, b) = (replica(&server).await, replica(&server).await);
    let repo = repository();

    let first = Bytes::from(vec![7u8; 1024]);
    let second = Bytes::from(vec![9u8; 512]);
    let last = Bytes::from_static(b"tail");
    let whole = [first.as_ref(), second.as_ref(), last.as_ref()].concat();
    let digest = ops::sha256_of(&whole);

    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();
    let r1 = ops::append_chunk(&a, &repo, upload_id, first, Some(0))
        .await
        .unwrap();
    assert_eq!(r1.new_offset, 1024);
    let r2 = ops::append_chunk(&b, &repo, upload_id, second, Some(1024))
        .await
        .unwrap();
    assert_eq!(r2.new_offset, 1536);
    assert_eq!(
        ops::upload_offset(&a, &repo, upload_id).await.unwrap(),
        1536
    );

    let parts = part_ids(&server, upload_id).await;
    assert_eq!(parts.len(), 2);

    let done = ops::complete_upload(&a, &repo, upload_id, last, Some(&digest))
        .await
        .expect("the replica that took the PUT assembles chunks another replica staged");
    assert_eq!(done.digest, digest);

    let stored = ops::get_blob_bytes(&b, &repo, &digest).await.unwrap();
    assert_eq!(stored.as_ref(), whole.as_slice());

    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oci_uploads WHERE uuid = $1")
        .bind(upload_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "a finished session is removed");
    for part in parts {
        assert!(
            !a.storage.blob_exists(&part_key(upload_id, part)).await,
            "staged chunks are removed once the blob is stored"
        );
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn single_chunk_then_empty_put_on_another_replica() {
    let server = common::TestServer::start().await;
    let (a, b) = (replica(&server).await, replica(&server).await);
    let repo = repository();

    let layer = Bytes::from(vec![3u8; 4096]);
    let digest = ops::sha256_of(&layer);

    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();
    ops::append_chunk(&a, &repo, upload_id, layer.clone(), None)
        .await
        .unwrap();
    let done = ops::complete_upload(&b, &repo, upload_id, Bytes::new(), Some(&digest))
        .await
        .unwrap();
    assert_eq!(done.digest, digest);
    assert_eq!(
        ops::get_blob_bytes(&a, &repo, &digest).await.unwrap(),
        layer
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn stale_chunk_from_another_replica_is_refused_and_not_kept() {
    let server = common::TestServer::start().await;
    let (a, b) = (replica(&server).await, replica(&server).await);
    let repo = repository();

    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();
    ops::append_chunk(&a, &repo, upload_id, Bytes::from_static(b"abcd"), Some(0))
        .await
        .unwrap();

    // A retry of the same first chunk, landing on the other replica.
    let stale = ops::append_chunk(&b, &repo, upload_id, Bytes::from_static(b"abcd"), Some(0))
        .await
        .err()
        .expect("a chunk that does not continue the session is refused");
    assert!(
        matches!(stale, OciError::Oci(OciCode::RangeNotSatisfiable, _)),
        "refused with the spec's 416, got {stale:?}"
    );
    assert_eq!(ops::upload_offset(&b, &repo, upload_id).await.unwrap(), 4);
    assert_eq!(part_ids(&server, upload_id).await.len(), 1);

    let digest = ops::sha256_of(b"abcdef");
    let done = ops::complete_upload(&b, &repo, upload_id, Bytes::from_static(b"ef"), None)
        .await
        .unwrap();
    assert_eq!(done.digest, digest);

    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn cancel_on_another_replica_removes_staged_chunks() {
    let server = common::TestServer::start().await;
    let (a, b) = (replica(&server).await, replica(&server).await);
    let repo = repository();

    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();
    ops::append_chunk(&a, &repo, upload_id, Bytes::from_static(b"chunk"), None)
        .await
        .unwrap();
    let parts = part_ids(&server, upload_id).await;
    assert!(a.storage.blob_exists(&part_key(upload_id, parts[0])).await);

    ops::cancel_upload(&b, &repo, upload_id).await.unwrap();

    assert!(!a.storage.blob_exists(&part_key(upload_id, parts[0])).await);
    let gone = ops::upload_offset(&a, &repo, upload_id)
        .await
        .err()
        .unwrap();
    assert!(matches!(gone, OciError::Oci(OciCode::BlobUploadUnknown, _)));

    server.cleanup().await;
}

/// A replica that died between writing a chunk and committing it leaves the chunk staged on the
/// session row and its object in the store; removing the session must find and delete it, since
/// the store cannot be listed.
#[tokio::test]
#[serial_test::serial]
async fn a_chunk_staged_by_a_replica_that_died_goes_with_its_session() {
    let server = common::TestServer::start().await;
    let a = replica(&server).await;
    let repo = repository();
    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();

    // What `append_chunk` does before its commit: stage the id, then write the object.
    let orphan = Uuid::new_v4();
    sqlx::query(
        "UPDATE oci_uploads SET staged_part_ids = array_append(staged_part_ids, $1) \
         WHERE uuid = $2",
    )
    .bind(orphan)
    .bind(upload_id)
    .execute(&server.db)
    .await
    .unwrap();
    a.storage
        .put_blob(&part_key(upload_id, orphan), Bytes::from_static(b"lost"))
        .await
        .unwrap();
    // A committed chunk from a later, healthy PATCH beside it.
    ops::append_chunk(&a, &repo, upload_id, Bytes::from_static(b"kept"), None)
        .await
        .unwrap();
    let committed = part_ids(&server, upload_id).await;
    assert_eq!(committed.len(), 1, "the orphan was never committed");

    ops::cancel_upload(&a, &repo, upload_id).await.unwrap();

    assert!(!a.storage.blob_exists(&part_key(upload_id, orphan)).await);
    assert!(
        !a.storage
            .blob_exists(&part_key(upload_id, committed[0]))
            .await
    );
    server.cleanup().await;
}

/// A committed chunk leaves the staged list, so only a chunk that never committed is in it.
#[tokio::test]
#[serial_test::serial]
async fn a_committed_chunk_is_no_longer_staged() {
    let server = common::TestServer::start().await;
    let a = replica(&server).await;
    let repo = repository();
    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();

    ops::append_chunk(&a, &repo, upload_id, Bytes::from_static(b"chunk"), None)
        .await
        .unwrap();

    let staged: Vec<Uuid> =
        sqlx::query_scalar("SELECT staged_part_ids FROM oci_uploads WHERE uuid = $1")
            .bind(upload_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(staged.is_empty(), "{staged:?}");
    assert_eq!(part_ids(&server, upload_id).await.len(), 1);
    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn startup_sweep_removes_abandoned_sessions_and_their_chunks() {
    let server = common::TestServer::start().await;
    let a = replica(&server).await;
    let repo = repository();

    let abandoned = ops::initiate_upload(&a, &repo).await.unwrap();
    ops::append_chunk(&a, &repo, abandoned, Bytes::from_static(b"old"), None)
        .await
        .unwrap();
    sqlx::query("UPDATE oci_uploads SET created_at = now() - interval '2 days' WHERE uuid = $1")
        .bind(abandoned)
        .execute(&server.db)
        .await
        .unwrap();
    let live = ops::initiate_upload(&a, &repo).await.unwrap();
    let abandoned_parts = part_ids(&server, abandoned).await;

    ops::sweep_abandoned_uploads(&a).await;

    assert!(ops::upload_offset(&a, &repo, abandoned).await.is_err());
    assert!(
        !a.storage
            .blob_exists(&part_key(abandoned, abandoned_parts[0]))
            .await
    );
    assert_eq!(ops::upload_offset(&a, &repo, live).await.unwrap(), 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
async fn a_manifest_cannot_claim_a_staged_chunk() {
    let server = common::TestServer::start().await;
    let a = replica(&server).await;
    let repo = repository();

    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();
    ops::append_chunk(&a, &repo, upload_id, Bytes::from_static(b"secret"), None)
        .await
        .unwrap();
    let key = part_key(upload_id, part_ids(&server, upload_id).await[0]);

    let config = Bytes::from_static(b"{}");
    let config_digest = ops::sha256_of(&config);
    ops::upload_blob_monolithic(&a, &repo, config, &config_digest)
        .await
        .unwrap();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config_digest, "size": 2},
        "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": key, "size": 6}],
    });
    let refused = ops::put_manifest(
        &a,
        &repo,
        "latest",
        "application/vnd.oci.image.manifest.v1+json",
        manifest.to_string().as_bytes(),
    )
    .await
    .err()
    .expect("a manifest naming a staged chunk is refused");
    assert!(
        matches!(refused, OciError::Oci(OciCode::ManifestBlobUnknown, _)),
        "got {refused:?}"
    );
    assert!(ops::get_blob_bytes(&a, &repo, &key).await.is_err());

    server.cleanup().await;
}

/// A blob larger than one multipart part is written in parts, from chunks two
/// replicas staged, and reads back whole.
#[tokio::test]
#[serial_test::serial]
async fn a_blob_larger_than_one_part_is_streamed_into_place() {
    let server = common::TestServer::start().await;
    let (a, b) = (replica(&server).await, replica(&server).await);
    let repo = repository();

    // 12 MiB + 12 MiB + 1 MiB: crosses the 16 MiB part size, and the last part is short.
    let first = Bytes::from((0..12 * 1024 * 1024).map(|i| i as u8).collect::<Vec<_>>());
    let second = Bytes::from(vec![5u8; 12 * 1024 * 1024]);
    let last = Bytes::from(vec![6u8; 1024 * 1024]);
    let whole = [first.as_ref(), second.as_ref(), last.as_ref()].concat();
    let digest = ops::sha256_of(&whole);

    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();
    ops::append_chunk(&a, &repo, upload_id, first, None)
        .await
        .unwrap();
    ops::append_chunk(&b, &repo, upload_id, second, None)
        .await
        .unwrap();
    let done = ops::complete_upload(&a, &repo, upload_id, last, Some(&digest))
        .await
        .unwrap();
    assert_eq!(done.digest, digest);

    let stored = ops::get_blob_bytes(&b, &repo, &digest).await.unwrap();
    assert_eq!(stored.len(), whole.len());
    assert!(stored.as_ref() == whole.as_slice());

    server.cleanup().await;
}

/// An upload whose bytes do not hash to the digest the PUT names is refused,
/// and leaves what is stored under that digest as it was.
#[tokio::test]
#[serial_test::serial]
async fn a_wrong_digest_never_replaces_the_blob_it_names() {
    let server = common::TestServer::start().await;
    let a = replica(&server).await;
    let repo = repository();

    let genuine = Bytes::from(vec![1u8; 20 * 1024 * 1024]);
    let digest = ops::sha256_of(&genuine);
    ops::upload_blob_monolithic(&a, &repo, genuine.clone(), &digest)
        .await
        .unwrap();

    let upload_id = ops::initiate_upload(&a, &repo).await.unwrap();
    ops::append_chunk(
        &a,
        &repo,
        upload_id,
        Bytes::from(vec![2u8; 20 * 1024 * 1024]),
        None,
    )
    .await
    .unwrap();
    let refused = ops::complete_upload(&a, &repo, upload_id, Bytes::new(), Some(&digest))
        .await
        .err()
        .expect("bytes that do not match the digest are refused");
    assert!(
        matches!(refused, OciError::Oci(OciCode::DigestInvalid, _)),
        "got {refused:?}"
    );

    let stored = ops::get_blob_bytes(&a, &repo, &digest).await.unwrap();
    assert!(stored == genuine, "the stored blob is untouched");
    assert_eq!(
        ops::upload_offset(&a, &repo, upload_id).await.unwrap(),
        20 * 1024 * 1024,
        "a refused PUT leaves the session for the client to retry or cancel"
    );

    server.cleanup().await;
}
