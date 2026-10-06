//! Key-addressed object storage for workspace files — one trait, one
//! implementation over the `object_store` crate, so the control plane runs on
//! AWS S3, DigitalOcean Spaces, MinIO/RustFS (all S3 API), Google Cloud Storage,
//! Azure Blob, or a plain filesystem for bare-metal dev. Deliberately separate
//! from the digest-keyed `BlobStore` in `nasiko-runtime`: workspace keys are
//! opaque paths chosen by the catalog, not content digests.
//!
//! An S3-compatible bucket used as the workspace store should carry an
//! abort-incomplete-multipart-upload lifecycle rule (one day): a part that
//! fails while an upload is being finished returns the error without aborting
//! the upload, and only the bucket can reap what that leaves behind.
use std::ops::Range;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

mod backend;

pub use backend::{build_from_url, build_from_url_with};

/// Object bytes in order; the stream ends at the first error.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, WorkspaceStoreError>> + Send>>;
/// Object keys under a prefix, in the backend's listing order.
pub type KeyStream = Pin<Box<dyn Stream<Item = Result<String, WorkspaceStoreError>> + Send>>;

/// What the backend knows about a stored object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    /// Stored length in bytes.
    pub size: u64,
    /// Backend-assigned entity tag; not every backend provides one.
    pub etag: Option<String>,
}

/// Object attributes applied at upload. Backends that honour them (S3, GCS,
/// Azure) serve them back on GET, which is how a direct-mode download gets a
/// real filename; the filesystem backend ignores them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PutOptions {
    /// MIME type served as `Content-Type`.
    pub content_type: Option<String>,
    /// Full header value, e.g. `attachment; filename="report.pdf"`.
    pub content_disposition: Option<String>,
}

/// Explicit S3-compatible connection settings. Applied to an `s3://` store when
/// supplied, overriding the `AWS_*` environment; other schemes ignore them.
///
/// `secret_access_key` is a secret, so `Debug` redacts it rather than deriving it
/// — a config struct that carries a credential must not leak it into a log line.
#[derive(Clone, Default)]
pub struct S3Settings {
    /// Endpoint URL, e.g. `http://localhost:9000`. `None` leaves the builder's
    /// own resolution (`AWS_ENDPOINT`, else the public AWS endpoint) in place.
    pub endpoint: Option<String>,
    /// Access key id.
    pub access_key_id: Option<String>,
    /// Secret access key. Never printed by `Debug`.
    pub secret_access_key: Option<String>,
    /// Region, e.g. `us-east-1`. S3-compatible stores still require one.
    pub region: Option<String>,
    /// `true` addresses the bucket as `endpoint/bucket/key`. MinIO and RustFS
    /// require it; AWS and DigitalOcean Spaces accept either. `None` leaves
    /// `object_store`'s own default (path-style) and its
    /// `AWS_VIRTUAL_HOSTED_STYLE_REQUEST` in charge.
    pub force_path_style: Option<bool>,
}

impl std::fmt::Debug for S3Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Settings")
            .field("endpoint", &self.endpoint)
            .field("access_key_id", &self.access_key_id)
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "<redacted>"),
            )
            .field("region", &self.region)
            .field("force_path_style", &self.force_path_style)
            .finish()
    }
}

/// Why a store operation did not complete. Variants are what a caller
/// distinguishes between: retry, fall back, reject the input, or give up.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceStoreError {
    /// No object at this key; the payload is the key as given.
    #[error("object not found: {0}")]
    NotFound(String),
    /// The backend has no way to perform the operation (a filesystem store
    /// cannot presign); the caller falls back instead of retrying.
    #[error("{0}")]
    Unsupported(&'static str),
    /// The store URL could not be parsed, names an unsupported scheme, or
    /// carries a path prefix that is not a valid key.
    #[error("invalid store url: {0}")]
    InvalidUrl(String),
    /// `key` breaks the [`WorkspaceObjectStore`] key invariant: the backend
    /// would store it under a rewritten name, so it is rejected up front.
    #[error("invalid key {key:?}: {reason}")]
    InvalidKey { key: String, reason: String },
    /// The body streamed into `put_stream` disagreed with its declared length.
    /// `received` is the count at which the mismatch was detected: the full
    /// body for a short one, the first excess chunk for an over-long one.
    #[error("body length mismatch: declared {declared} bytes, received {received}")]
    LengthMismatch { declared: u64, received: u64 },
    /// The backend failed for a reason the caller cannot act on; the message
    /// is the backend's own.
    #[error("storage backend: {0}")]
    Backend(String),
}

/// Key-addressed object storage. Keys are opaque slash-separated paths chosen
/// by the catalog; the store never interprets them.
///
/// Key invariant: a key is `/`-joined, non-empty segments of ASCII, with no
/// leading or trailing `/`, no `.` or `..` segment, no leading or trailing
/// whitespace in a segment (S3 lists such a segment trimmed, under a name the
/// object was never stored with), no segment ending in an ASCII letter
/// followed by `:` (`Url::to_file_path`, which the filesystem backend resolves
/// keys through, appends a `/` when the assembled path's last two bytes read
/// as a Windows drive specification, so the key becomes a directory — only a
/// final segment can be caught that way, and the rule covers every segment on
/// purpose, so it needs no positional reasoning), and none of the
/// characters `object_store` percent-encodes: `%`, `#`, `[`, `]`, `{`, `}`,
/// `^`, `~`, `<`, `>`, `|`, `\`, `"`, `*`, `?`, the backtick and control
/// characters — exactly the set `object_store`'s lenient `Path::from` would
/// percent-encode, which is what the code checks, not this list. Every
/// backend then stores and lists a valid key verbatim. A key outside that set
/// is rejected with [`WorkspaceStoreError::InvalidKey`], never rewritten.
///
/// A prefix is a key, optionally with a trailing `/`. The empty string is the
/// one prefix that is not a key, and the two prefix methods read it
/// differently: [`WorkspaceObjectStore::list_prefix`] takes it as the whole
/// store, while [`WorkspaceObjectStore::delete_prefix`] rejects it, so no
/// single call can wipe the store.
#[async_trait]
pub trait WorkspaceObjectStore: Send + Sync {
    /// [`Self::put_stream_with`] with no attributes.
    async fn put_stream(
        &self,
        key: &str,
        body: ByteStream,
        len: Option<u64>,
    ) -> Result<ObjectMeta, WorkspaceStoreError> {
        self.put_stream_with(key, body, len, PutOptions::default())
            .await
    }

    /// Streams `body` into `key`, replacing any existing object. `len`, when
    /// known, is enforced: a body of any other length is rejected with
    /// [`WorkspaceStoreError::LengthMismatch`] and nothing is stored, so a
    /// truncated upload never becomes a truncated object. `opts` are applied
    /// to the object where the backend supports them (see [`PutOptions`]).
    async fn put_stream_with(
        &self,
        key: &str,
        body: ByteStream,
        len: Option<u64>,
        opts: PutOptions,
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

    /// Every key under `prefix`, or every key in the store when `prefix` is
    /// empty. GC and delete-my-data only — never on a request path.
    async fn list_prefix(&self, prefix: &str) -> Result<KeyStream, WorkspaceStoreError>;

    /// Deletes every key under `prefix`; returns how many were deleted. An
    /// empty prefix is rejected with [`WorkspaceStoreError::InvalidKey`]
    /// rather than read as the whole store: every sweep names an agent or a
    /// user subtree, so no single call can wipe the store. A key
    /// that disappears between listing and deletion (two sweepers racing) is
    /// skipped, not an error, and is left uncounted when the backend reports
    /// its absence. The count is advisory: S3's bulk delete reports a missing
    /// key as deleted, and a filesystem may answer two racing unlinks with
    /// success for both (macOS does).
    async fn delete_prefix(&self, prefix: &str) -> Result<u64, WorkspaceStoreError>;

    /// A URL that downloads the object for `ttl`, or `Unsupported` when the
    /// backend cannot sign (filesystem) — the caller then serves the bytes
    /// through the relay. `download_name` is not honoured by any backend over
    /// `object_store` 0.10: `Signer::signed_url` takes no query parameters and
    /// SigV4 canonicalises the query before signing, so a name cannot be
    /// signed into the URL. The download's filename comes from the object's
    /// `Content-Disposition`, set at upload with [`Self::put_stream_with`].
    /// The parameter stays so an `object_store` bump that can sign query
    /// parameters honours it without a signature change.
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

    /// Whether this backend can issue presigned URLs at all; without them
    /// every transfer is relayed. It says nothing about whether a client can
    /// reach the signed endpoint — the store never probes it — so
    /// `WORKSPACE_TRANSFER_MODE=auto` must combine this with an explicit
    /// deployment setting in the server.
    fn supports_presigned(&self) -> bool;
}
