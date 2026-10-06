use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};
use object_store::aws::AmazonS3Builder;
use object_store::azure::MicrosoftAzureBuilder;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::prefix::PrefixStore;
use object_store::signer::Signer;
use object_store::{GetOptions, GetRange, ObjectStore, WriteMultipart};
use reqwest::Method;
use url::Url;

use crate::{ByteStream, KeyStream, ObjectMeta, WorkspaceObjectStore, WorkspaceStoreError};

/// Multipart part size: above S3's 5 MiB minimum for non-final parts, and the
/// unit of memory one upload buffers.
const PART_SIZE: usize = 8 << 20;
/// Parts uploading concurrently per `put_stream`, bounding its memory to a few
/// parts while still overlapping network time with reading the body.
const MAX_IN_FLIGHT_PARTS: usize = 2;
const CANNOT_PRESIGN: &str = "this storage backend cannot presign URLs";

/// One backend for every cloud: `object_store` picks the concrete client from
/// the URL scheme and the provider's standard env (`AWS_*`, `GOOGLE_*`,
/// `AZURE_*`).
pub struct ObjectStoreBackend {
    /// Already scoped to `prefix` when one was given, so data operations pass
    /// keys through untouched.
    store: Arc<dyn ObjectStore>,
    /// `None` when the backend cannot sign URLs (filesystem).
    signer: Option<Arc<dyn Signer>>,
    /// Key prefix from the store URL's path. Signers work on the raw bucket,
    /// so signed URLs apply it explicitly.
    prefix: Option<Path>,
}

/// Builds the store for `s3://bucket[/prefix]`, `gs://bucket[/prefix]`,
/// `az://container[/prefix]` or `file:///absolute/dir`. Credentials and
/// endpoints come from the provider's standard env — for S3-compatible stores
/// `AWS_ENDPOINT`, `AWS_ALLOW_HTTP`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
/// and `AWS_REGION`. The filesystem backend cannot presign, so every transfer
/// on it is relayed.
pub async fn build_from_url(
    url: &str,
) -> Result<Box<dyn WorkspaceObjectStore>, WorkspaceStoreError> {
    let parsed = Url::parse(url).map_err(|e| WorkspaceStoreError::InvalidUrl(e.to_string()))?;
    let prefix = key_prefix(&parsed);
    let backend = match parsed.scheme() {
        "s3" => with_signer(
            AmazonS3Builder::from_env()
                .with_url(url)
                .build()
                .map_err(invalid_url)?,
            prefix,
        ),
        "gs" => with_signer(
            GoogleCloudStorageBuilder::from_env()
                .with_url(url)
                .build()
                .map_err(invalid_url)?,
            prefix,
        ),
        "az" | "azure" | "abfs" => with_signer(
            MicrosoftAzureBuilder::from_env()
                .with_url(url)
                .build()
                .map_err(invalid_url)?,
            prefix,
        ),
        "file" => {
            let dir = parsed.to_file_path().map_err(|()| {
                WorkspaceStoreError::InvalidUrl("file:// needs an absolute path".into())
            })?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| WorkspaceStoreError::Backend(e.to_string()))?;
            let fs = LocalFileSystem::new_with_prefix(dir).map_err(invalid_url)?;
            ObjectStoreBackend::new(Arc::new(fs), None, None)
        }
        other => {
            return Err(WorkspaceStoreError::InvalidUrl(format!(
                "unsupported scheme '{other}'"
            )));
        }
    };
    Ok(Box::new(backend))
}

/// Backends that both store and sign share one handle for both roles.
fn with_signer<S: ObjectStore + Signer>(store: S, prefix: Option<Path>) -> ObjectStoreBackend {
    let store = Arc::new(store);
    ObjectStoreBackend::new(store.clone(), Some(store), prefix)
}

/// The URL path after the bucket, as a key prefix; `None` when there is none.
fn key_prefix(url: &Url) -> Option<Path> {
    let path = url.path().trim_matches('/');
    (!path.is_empty()).then(|| Path::from(path))
}

fn invalid_url(err: object_store::Error) -> WorkspaceStoreError {
    WorkspaceStoreError::InvalidUrl(err.to_string())
}

fn map_err(err: object_store::Error, key: &str) -> WorkspaceStoreError {
    match err {
        object_store::Error::NotFound { .. } => WorkspaceStoreError::NotFound(key.to_owned()),
        other => WorkspaceStoreError::Backend(other.to_string()),
    }
}

impl ObjectStoreBackend {
    fn new(
        inner: Arc<dyn ObjectStore>,
        signer: Option<Arc<dyn Signer>>,
        prefix: Option<Path>,
    ) -> Self {
        let store: Arc<dyn ObjectStore> = match &prefix {
            Some(prefix) => Arc::new(PrefixStore::new(inner, prefix.clone())),
            None => inner,
        };
        Self {
            store,
            signer,
            prefix,
        }
    }

    /// The bucket-level location of `key`, for signers that bypass `PrefixStore`.
    fn signed_location(&self, key: &str) -> Path {
        let key = Path::from(key);
        match &self.prefix {
            Some(prefix) => prefix.parts().chain(key.parts()).collect(),
            None => key,
        }
    }

    async fn sign(
        &self,
        method: Method,
        key: &str,
        ttl: Duration,
    ) -> Result<Url, WorkspaceStoreError> {
        let signer = self
            .signer
            .as_ref()
            .ok_or(WorkspaceStoreError::Unsupported(CANNOT_PRESIGN))?;
        signer
            .signed_url(method, &self.signed_location(key), ttl)
            .await
            .map_err(|e| map_err(e, key))
    }
}

/// Feeds `body` into `writer` in `PART_SIZE` parts and returns the byte count.
/// A body that overruns `declared` is cut off at the first excess chunk; one
/// that falls short is rejected once it ends. Either way the caller aborts the
/// upload, so a wrong-length body never completes as an object.
async fn feed_parts(
    writer: &mut WriteMultipart,
    mut body: ByteStream,
    declared: Option<u64>,
    key: &str,
) -> Result<u64, WorkspaceStoreError> {
    let mut received = 0u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        received += chunk.len() as u64;
        if let Some(declared) = declared
            && received > declared
        {
            return Err(WorkspaceStoreError::LengthMismatch { declared, received });
        }
        writer
            .wait_for_capacity(MAX_IN_FLIGHT_PARTS)
            .await
            .map_err(|e| map_err(e, key))?;
        writer.put(chunk);
    }
    if let Some(declared) = declared
        && received != declared
    {
        return Err(WorkspaceStoreError::LengthMismatch { declared, received });
    }
    Ok(received)
}

fn byte_range(range: Range<u64>) -> GetRange {
    GetRange::Bounded(range.start as usize..range.end as usize)
}

#[async_trait]
impl WorkspaceObjectStore for ObjectStoreBackend {
    async fn put_stream(
        &self,
        key: &str,
        body: ByteStream,
        len: Option<u64>,
    ) -> Result<ObjectMeta, WorkspaceStoreError> {
        let upload = self
            .store
            .put_multipart(&Path::from(key))
            .await
            .map_err(|e| map_err(e, key))?;
        let mut writer = WriteMultipart::new_with_chunk_size(upload, PART_SIZE);
        let size = match feed_parts(&mut writer, body, len, key).await {
            Ok(size) => size,
            Err(err) => {
                // The body's error is the one to report; a failed abort only
                // leaves orphan parts for the bucket's lifecycle rule to reap.
                let _ = writer.abort().await;
                return Err(err);
            }
        };
        let result = writer.finish().await.map_err(|e| map_err(e, key))?;
        Ok(ObjectMeta {
            size,
            etag: result.e_tag,
        })
    }

    async fn get_stream(
        &self,
        key: &str,
        range: Option<Range<u64>>,
    ) -> Result<ByteStream, WorkspaceStoreError> {
        let options = GetOptions {
            range: range.map(byte_range),
            ..GetOptions::default()
        };
        let result = self
            .store
            .get_opts(&Path::from(key), options)
            .await
            .map_err(|e| map_err(e, key))?;
        let key = key.to_owned();
        Ok(Box::pin(
            result.into_stream().map_err(move |e| map_err(e, &key)),
        ))
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>, WorkspaceStoreError> {
        match self.store.head(&Path::from(key)).await {
            Ok(meta) => Ok(Some(ObjectMeta {
                size: meta.size as u64,
                etag: meta.e_tag,
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(err) => Err(map_err(err, key)),
        }
    }

    async fn delete(&self, key: &str) -> Result<(), WorkspaceStoreError> {
        match self.store.delete(&Path::from(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(err) => Err(map_err(err, key)),
        }
    }

    async fn list_prefix(&self, prefix: &str) -> Result<KeyStream, WorkspaceStoreError> {
        // `ObjectStore::list` borrows the store for the stream's lifetime; the
        // generator owns a handle so the returned stream is self-contained.
        let store = Arc::clone(&self.store);
        let prefix = prefix.to_owned();
        Ok(Box::pin(async_stream::stream! {
            let path = Path::from(prefix.as_str());
            let mut entries = store.list(Some(&path));
            while let Some(entry) = entries.next().await {
                yield entry.map(|meta| meta.location.to_string()).map_err(|e| map_err(e, &prefix));
            }
        }))
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<u64, WorkspaceStoreError> {
        let path = Path::from(prefix);
        let locations = self
            .store
            .list(Some(&path))
            .map_ok(|meta| meta.location)
            .boxed();
        let mut deleted = self.store.delete_stream(locations);
        let mut count = 0u64;
        while let Some(result) = deleted.next().await {
            result.map_err(|e| map_err(e, prefix))?;
            count += 1;
        }
        Ok(count)
    }

    async fn presigned_get_url(
        &self,
        key: &str,
        ttl: Duration,
        _download_name: Option<&str>,
    ) -> Result<Url, WorkspaceStoreError> {
        // No `object_store` 0.10 signer can add a response-content-disposition
        // parameter: `Signer` takes none, its query signing is crate-private,
        // and appending one after signing invalidates the signature. The name
        // therefore rides on the object's Content-Disposition, set at upload.
        self.sign(Method::GET, key, ttl).await
    }

    async fn presigned_put_url(
        &self,
        key: &str,
        ttl: Duration,
        _content_length: Option<u64>,
    ) -> Result<Url, WorkspaceStoreError> {
        self.sign(Method::PUT, key, ttl).await
    }

    fn supports_presigned(&self) -> bool {
        self.signer.is_some()
    }
}
