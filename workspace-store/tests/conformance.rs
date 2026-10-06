//! Behavioural contract every `WorkspaceObjectStore` backend must meet. Runs
//! against the filesystem backend always, and against an S3-compatible
//! endpoint when `WORKSPACE_TEST_S3_URL` is set (e.g. `s3://nasiko-workspace-test`
//! with `AWS_ENDPOINT`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
//! `AWS_ALLOW_HTTP=true` in the env — MinIO, RustFS or real S3).
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::StreamExt;
use nasiko_workspace_store::{
    ByteStream, WorkspaceObjectStore, WorkspaceStoreError, build_from_url,
};
use tempfile::TempDir;

const S3_URL_ENV: &str = "WORKSPACE_TEST_S3_URL";

struct Fixture {
    name: String,
    store: Box<dyn WorkspaceObjectStore>,
    /// Keeps the filesystem backend's directory alive for the test's lifetime.
    _tempdir: Option<TempDir>,
}

async fn fixtures() -> Vec<Fixture> {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let file_url = format!("file://{}", tempdir.path().display());
    let mut fixtures = vec![Fixture {
        store: build_from_url(&file_url).await.expect("filesystem store"),
        name: file_url,
        _tempdir: Some(tempdir),
    }];
    if let Ok(url) = std::env::var(S3_URL_ENV) {
        fixtures.push(Fixture {
            store: build_from_url(&url).await.expect("s3 store"),
            name: url,
            _tempdir: None,
        });
    }
    fixtures
}

fn s3_url() -> Option<String> {
    std::env::var(S3_URL_ENV).ok()
}

/// Unique per process and per call, so parallel tests never share a key.
fn unique() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{nanos:x}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

fn stream_of(chunks: Vec<Bytes>) -> ByteStream {
    Box::pin(futures::stream::iter(chunks.into_iter().map(Ok)))
}

async fn collect(mut stream: ByteStream) -> Result<Vec<u8>, WorkspaceStoreError> {
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk?);
    }
    Ok(out)
}

#[tokio::test]
async fn put_head_get_delete_roundtrip() {
    for Fixture { name, store, .. } in fixtures().await {
        let key = format!("workspace/t/{}/blobs/{}", unique(), unique());
        let body = Bytes::from_static(b"hello workspace");

        let meta = store
            .put_stream(&key, stream_of(vec![body.clone()]), Some(15))
            .await
            .unwrap();
        assert_eq!(meta.size, 15, "{name}");
        assert_eq!(store.head(&key).await.unwrap().unwrap().size, 15, "{name}");

        let got = collect(store.get_stream(&key, None).await.unwrap())
            .await
            .unwrap();
        assert_eq!(got, b"hello workspace", "{name}");

        store.delete(&key).await.unwrap();
        assert!(store.head(&key).await.unwrap().is_none(), "{name}");
    }
}

#[tokio::test]
async fn multi_chunk_body_is_stored_contiguously() {
    for Fixture { name, store, .. } in fixtures().await {
        let key = format!("workspace/t/{}/blobs/{}", unique(), unique());
        let chunks = vec![
            Bytes::from_static(b"ab"),
            Bytes::from_static(b""),
            Bytes::from_static(b"cde"),
        ];

        let meta = store
            .put_stream(&key, stream_of(chunks), None)
            .await
            .unwrap();
        assert_eq!(meta.size, 5, "{name}");
        let got = collect(store.get_stream(&key, None).await.unwrap())
            .await
            .unwrap();
        assert_eq!(got, b"abcde", "{name}");

        store.delete(&key).await.unwrap();
    }
}

#[tokio::test]
async fn empty_body_stores_an_empty_object() {
    for Fixture { name, store, .. } in fixtures().await {
        let key = format!("workspace/t/{}/blobs/{}", unique(), unique());

        let meta = store
            .put_stream(&key, stream_of(vec![]), Some(0))
            .await
            .unwrap();
        assert_eq!(meta.size, 0, "{name}");
        assert_eq!(store.head(&key).await.unwrap().unwrap().size, 0, "{name}");
        let got = collect(store.get_stream(&key, None).await.unwrap())
            .await
            .unwrap();
        assert!(got.is_empty(), "{name}");

        store.delete(&key).await.unwrap();
    }
}

#[tokio::test]
async fn declared_length_mismatch_is_rejected_without_storing() {
    for Fixture { name, store, .. } in fixtures().await {
        for (declared, label) in [(20u64, "short"), (3u64, "over-long")] {
            let key = format!("workspace/t/{}/blobs/{}", unique(), unique());
            let body = stream_of(vec![
                Bytes::from_static(b"hello"),
                Bytes::from_static(b" workspace"),
            ]);

            let err = store
                .put_stream(&key, body, Some(declared))
                .await
                .unwrap_err();
            assert!(
                matches!(err, WorkspaceStoreError::LengthMismatch { declared: d, .. } if d == declared),
                "{name}/{label}: {err}"
            );
            assert!(store.head(&key).await.unwrap().is_none(), "{name}/{label}");
        }
    }
}

#[tokio::test]
async fn ranged_get_returns_only_the_requested_bytes() {
    for Fixture { name, store, .. } in fixtures().await {
        let key = format!("workspace/t/{}/blobs/{}", unique(), unique());
        store
            .put_stream(
                &key,
                stream_of(vec![Bytes::from_static(b"0123456789")]),
                Some(10),
            )
            .await
            .unwrap();

        let got = collect(store.get_stream(&key, Some(2..5)).await.unwrap())
            .await
            .unwrap();
        assert_eq!(got, b"234", "{name}");

        store.delete(&key).await.unwrap();
    }
}

#[tokio::test]
async fn head_and_get_agree_on_a_missing_key() {
    for Fixture { name, store, .. } in fixtures().await {
        let key = format!("workspace/t/{}/blobs/{}", unique(), unique());

        assert!(store.head(&key).await.unwrap().is_none(), "{name}");
        let err = store
            .get_stream(&key, None)
            .await
            .err()
            .expect("missing key must not stream");
        assert!(
            matches!(err, WorkspaceStoreError::NotFound(ref k) if *k == key),
            "{name}: {err}"
        );
    }
}

#[tokio::test]
async fn delete_of_a_missing_key_succeeds() {
    for Fixture { name, store, .. } in fixtures().await {
        let key = format!("workspace/t/{}/blobs/{}", unique(), unique());
        store
            .delete(&key)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[tokio::test]
async fn list_and_delete_prefix() {
    for Fixture { name, store, .. } in fixtures().await {
        let prefix = format!("workspace/p/{}/", unique());
        let sibling = format!("workspace/p/{}/blobs/outside", unique());
        for i in 0..3u8 {
            let body = Bytes::from(vec![b'x'; usize::from(i) + 1]);
            store
                .put_stream(&format!("{prefix}blobs/{i}"), stream_of(vec![body]), None)
                .await
                .unwrap();
        }
        store
            .put_stream(&sibling, stream_of(vec![Bytes::from_static(b"s")]), None)
            .await
            .unwrap();

        let mut keys = Vec::new();
        let mut listing = store.list_prefix(&prefix).await.unwrap();
        while let Some(key) = listing.next().await {
            keys.push(key.unwrap());
        }
        keys.sort();
        let expected: Vec<String> = (0..3).map(|i| format!("{prefix}blobs/{i}")).collect();
        assert_eq!(keys, expected, "{name}");

        assert_eq!(store.delete_prefix(&prefix).await.unwrap(), 3, "{name}");
        assert!(
            store
                .head(&format!("{prefix}blobs/0"))
                .await
                .unwrap()
                .is_none(),
            "{name}"
        );
        assert!(
            store.head(&sibling).await.unwrap().is_some(),
            "{name}: sibling must survive"
        );
        store.delete(&sibling).await.unwrap();
    }
}

#[tokio::test]
async fn presigned_urls_only_where_the_backend_can_sign() {
    for Fixture { name, store, .. } in fixtures().await {
        let get = store
            .presigned_get_url("workspace/x/blobs/y", Duration::from_secs(60), None)
            .await;
        let put = store
            .presigned_put_url("workspace/x/blobs/y", Duration::from_secs(60), None)
            .await;
        if name.starts_with("file://") {
            assert!(!store.supports_presigned(), "{name}");
            assert!(
                matches!(get, Err(WorkspaceStoreError::Unsupported(_))),
                "{name}"
            );
            assert!(
                matches!(put, Err(WorkspaceStoreError::Unsupported(_))),
                "{name}"
            );
        } else {
            assert!(store.supports_presigned(), "{name}");
            assert!(get.unwrap().as_str().starts_with("http"), "{name}");
            assert!(put.unwrap().as_str().starts_with("http"), "{name}");
        }
    }
}

/// S3 only: the signed URLs must actually move bytes, not merely look right.
#[tokio::test]
async fn presigned_urls_transfer_bytes_on_s3() {
    let Some(url) = s3_url() else { return };
    let store = build_from_url(&url).await.expect("s3 store");
    let http = reqwest::Client::new();
    let ttl = Duration::from_secs(60);

    let upload_key = format!("workspace/t/{}/blobs/{}", unique(), unique());
    let put_url = store
        .presigned_put_url(&upload_key, ttl, Some(6))
        .await
        .unwrap();
    let put = http.put(put_url).body("direct").send().await.unwrap();
    assert!(put.status().is_success(), "presigned PUT: {}", put.status());
    assert_eq!(store.head(&upload_key).await.unwrap().unwrap().size, 6);

    let get_url = store
        .presigned_get_url(&upload_key, ttl, Some("report.txt"))
        .await
        .unwrap();
    let get = http.get(get_url).send().await.unwrap();
    assert!(get.status().is_success(), "presigned GET: {}", get.status());
    assert_eq!(get.bytes().await.unwrap().as_ref(), b"direct");

    store.delete(&upload_key).await.unwrap();
}

/// S3 only: a path on the store URL scopes every key, including signed ones.
#[tokio::test]
async fn url_path_prefix_scopes_keys_on_s3() {
    let Some(url) = s3_url() else { return };
    let prefix = format!("conformance-prefix-{}", unique());
    let scoped = build_from_url(&format!("{}/{prefix}", url.trim_end_matches('/')))
        .await
        .expect("scoped store");
    let root = build_from_url(&url).await.expect("root store");

    let key = "workspace/t/blobs/scoped";
    scoped
        .put_stream(key, stream_of(vec![Bytes::from_static(b"p")]), Some(1))
        .await
        .unwrap();
    assert!(
        root.head(&format!("{prefix}/{key}"))
            .await
            .unwrap()
            .is_some(),
        "key must land under the prefix"
    );
    assert!(
        root.head(key).await.unwrap().is_none(),
        "key must not land at the bucket root"
    );

    let signed = scoped
        .presigned_get_url(key, Duration::from_secs(60), None)
        .await
        .unwrap();
    assert!(
        signed.path().contains(&prefix),
        "signed URL must carry the prefix: {signed}"
    );
    let got = reqwest::get(signed).await.unwrap();
    assert!(
        got.status().is_success(),
        "signed GET under prefix: {}",
        got.status()
    );

    let mut listed = Vec::new();
    let mut listing = scoped.list_prefix("workspace/t/").await.unwrap();
    while let Some(entry) = listing.next().await {
        listed.push(entry.unwrap());
    }
    assert_eq!(
        listed,
        vec![key.to_string()],
        "listing must be relative to the prefix"
    );

    assert_eq!(scoped.delete_prefix("workspace/").await.unwrap(), 1);
    assert!(
        root.head(&format!("{prefix}/{key}"))
            .await
            .unwrap()
            .is_none()
    );
}
