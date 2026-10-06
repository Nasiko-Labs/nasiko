//! Key-addressed object storage for workspace files — one trait, one
//! implementation over the `object_store` crate, so the control plane runs on
//! AWS S3, DigitalOcean Spaces, MinIO/RustFS (all S3 API), Google Cloud Storage,
//! Azure Blob, or a plain filesystem for bare-metal dev. Deliberately separate
//! from the digest-keyed `BlobStore` in `nasiko-runtime`: workspace keys are
//! opaque paths chosen by the catalog, not content digests.
use std::ops::Range;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

mod backend;

pub use backend::build_from_url;

/// Object bytes in order; the stream ends at the first error.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, WorkspaceStoreError>> + Send>>;
/// Object keys under a prefix, in the backend's listing order.
pub type KeyStream = Pin<Box<dyn Stream<Item = Result<String, WorkspaceStoreError>> + Send>>;

/// What the backend knows about a stored object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    pub size: u64,
    /// Backend-assigned entity tag; not every backend provides one.
    pub etag: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceStoreError {
    #[error("object not found: {0}")]
    NotFound(String),
    /// The backend has no way to perform the operation (a filesystem store
    /// cannot presign); the caller falls back instead of retrying.
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("invalid store url: {0}")]
    InvalidUrl(String),
    /// The body streamed into `put_stream` disagreed with its declared length.
    /// `received` is the count at which the mismatch was detected: the full
    /// body for a short one, the first excess chunk for an over-long one.
    #[error("body length mismatch: declared {declared} bytes, received {received}")]
    LengthMismatch { declared: u64, received: u64 },
    #[error("storage backend: {0}")]
    Backend(String),
}

/// Key-addressed object storage. Keys are opaque slash-separated paths chosen
/// by the catalog; the store never interprets them.
#[async_trait]
pub trait WorkspaceObjectStore: Send + Sync {
    /// Streams `body` into `key`, replacing any existing object. `len`, when
    /// known, is enforced: a body of any other length is rejected with
    /// [`WorkspaceStoreError::LengthMismatch`] and nothing is stored, so a
    /// truncated upload never becomes a truncated object.
    async fn put_stream(
        &self,
        key: &str,
        body: ByteStream,
        len: Option<u64>,
    ) -> Result<ObjectMeta, WorkspaceStoreError>;

    /// Streams the object, or the byte `range` of it when given.
    async fn get_stream(
        &self,
        key: &str,
        range: Option<Range<u64>>,
    ) -> Result<ByteStream, WorkspaceStoreError>;

    /// `Ok(None)` for absence — `head` and `get_stream` must agree on which
    /// keys exist.
    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>, WorkspaceStoreError>;

    /// Idempotent: deleting a missing key succeeds.
    async fn delete(&self, key: &str) -> Result<(), WorkspaceStoreError>;

    /// Every key under `prefix`. GC and delete-my-data only — never on a
    /// request path.
    async fn list_prefix(&self, prefix: &str) -> Result<KeyStream, WorkspaceStoreError>;

    /// Deletes every key under `prefix`; returns how many were deleted.
    async fn delete_prefix(&self, prefix: &str) -> Result<u64, WorkspaceStoreError>;

    /// A URL that downloads the object for `ttl`, or `Unsupported` when the
    /// backend cannot sign (filesystem) — the caller then serves the bytes
    /// through the relay. `download_name` is the filename the download should
    /// carry; a backend applies it only when it can sign it into the URL, so
    /// callers that need it on every backend set the object's
    /// `Content-Disposition` at upload instead.
    async fn presigned_get_url(
        &self,
        key: &str,
        ttl: Duration,
        download_name: Option<&str>,
    ) -> Result<url::Url, WorkspaceStoreError>;

    /// A URL that accepts a PUT of the object for `ttl`, or `Unsupported` as
    /// for [`Self::presigned_get_url`]. `content_length` is advisory; the
    /// caller learns the stored size from [`Self::head`] once the upload lands.
    async fn presigned_put_url(
        &self,
        key: &str,
        ttl: Duration,
        content_length: Option<u64>,
    ) -> Result<url::Url, WorkspaceStoreError>;

    /// Whether this backend can issue presigned URLs at all. Drives
    /// `WORKSPACE_TRANSFER_MODE=auto`: without them every transfer is relayed.
    fn supports_presigned(&self) -> bool;
}
