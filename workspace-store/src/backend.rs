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
use object_store::{
    Attribute, Attributes, GetOptions, GetRange, ObjectStore, PutMultipartOpts, WriteMultipart,
};
use reqwest::Method;
use url::Url;

use crate::{
    ByteStream, KeyStream, ObjectMeta, PutOptions, WorkspaceObjectStore, WorkspaceStoreError,
};

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
    /// Whether uploads may carry object attributes. `LocalFileSystem` rejects
    /// any with `NotImplemented` rather than ignoring them, so the backend
    /// drops them itself to keep `put_stream_with` portable.
    stores_attributes: bool,
}

/// Builds the store for `s3://bucket[/prefix]`, `gs://bucket[/prefix]`,
/// `az://container[/prefix]` or `file:///absolute/dir`. Credentials and
/// endpoints come from the provider's standard env — for S3-compatible stores
/// `AWS_ENDPOINT`, `AWS_ALLOW_HTTP`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
/// and `AWS_REGION`. The filesystem backend cannot presign, so every transfer
/// on it is relayed.
///
/// Presigning needs a credential that can sign, which is narrower than one
/// that can read and write: S3 signs with static keys or any credential
/// provider the builder resolves (env, web identity, instance metadata); GCS
/// with a service-account key, or through IAM `signBlob` when the credential
/// carries no private key (application-default or metadata-server
/// credentials); Azure with an account key, or an AAD bearer token via a user
/// delegation key. An Azure SAS token or skip-signature mode cannot sign and
/// fails at call time with [`WorkspaceStoreError::Backend`] — the server's
/// probe should treat that as "relay", not as a transient fault.
pub async fn build_from_url(
    url: &str,
) -> Result<Box<dyn WorkspaceObjectStore>, WorkspaceStoreError> {
    let parsed = Url::parse(url).map_err(|e| WorkspaceStoreError::InvalidUrl(e.to_string()))?;
    let backend = match parsed.scheme() {
        "s3" => ObjectStoreBackend::cloud(
            AmazonS3Builder::from_env()
                .with_url(url)
                .build()
                .map_err(invalid_url)?,
            &parsed,
        )?,
        "gs" => ObjectStoreBackend::cloud(
            GoogleCloudStorageBuilder::from_env()
                .with_url(url)
                .build()
                .map_err(invalid_url)?,
            &parsed,
        )?,
        "az" | "azure" | "abfs" => ObjectStoreBackend::cloud(
            MicrosoftAzureBuilder::from_env()
                .with_url(url)
                .build()
                .map_err(invalid_url)?,
            &parsed,
        )?,
        "file" => {
            let dir = parsed.to_file_path().map_err(|()| {
                WorkspaceStoreError::InvalidUrl("file:// needs an absolute path".into())
            })?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| WorkspaceStoreError::Backend(e.to_string()))?;
            let fs = LocalFileSystem::new_with_prefix(dir).map_err(invalid_url)?;
            ObjectStoreBackend::filesystem(fs)
        }
        other => {
            return Err(WorkspaceStoreError::InvalidUrl(format!(
                "unsupported scheme '{other}'"
            )));
        }
    };
    Ok(Box::new(backend))
}

/// The URL path after the bucket, as a key prefix; `None` when there is none.
/// Validated like a key, so a prefix the store would rewrite is refused up
/// front rather than scattering objects under a name nobody asked for.
fn key_prefix(url: &Url) -> Result<Option<Path>, WorkspaceStoreError> {
    let path = url.path().strip_prefix('/').unwrap_or(url.path());
    let prefix = prefix_path(path)
        .map_err(|err| WorkspaceStoreError::InvalidUrl(format!("path prefix: {err}")))?;
    Ok((!prefix.as_ref().is_empty()).then_some(prefix))
}

/// Parses `key` as a store path without rewriting it. `object_store`'s lenient
/// `Path::from` percent-encodes some characters and collapses or strips
/// slashes, which stores an object under a different name than the one later
/// looked up or listed; a key it would alter is rejected instead.
fn object_path(key: &str) -> Result<Path, WorkspaceStoreError> {
    let invalid = |reason: String| WorkspaceStoreError::InvalidKey {
        key: key.to_owned(),
        reason,
    };
    if key.is_empty() {
        return Err(invalid("empty".into()));
    }
    let path = Path::parse(key).map_err(|e| invalid(e.to_string()))?;
    if path.as_ref() != key {
        return Err(invalid("leading or trailing '/'".into()));
    }
    if Path::from(key).as_ref() != key {
        return Err(invalid(
            "contains a character object_store would percent-encode".into(),
        ));
    }
    Ok(path)
}

/// A listing prefix is a key, optionally with the trailing `/` a directory
/// style prefix naturally carries; empty means the whole store.
fn prefix_path(prefix: &str) -> Result<Path, WorkspaceStoreError> {
    let prefix = prefix.strip_suffix('/').unwrap_or(prefix);
    if prefix.is_empty() {
        return Ok(Path::default());
    }
    object_path(prefix)
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

/// The attribute set `opts` asks for, in `object_store`'s terms.
fn attributes(opts: PutOptions) -> Attributes {
    let mut attributes = Attributes::new();
    if let Some(content_type) = opts.content_type {
        attributes.insert(Attribute::ContentType, content_type.into());
    }
    if let Some(disposition) = opts.content_disposition {
        attributes.insert(Attribute::ContentDisposition, disposition.into());
    }
    attributes
}

impl ObjectStoreBackend {
    /// A cloud backend: one handle both stores and signs, scoped to the URL's
    /// path prefix when it has one.
    fn cloud<S: ObjectStore + Signer>(store: S, url: &Url) -> Result<Self, WorkspaceStoreError> {
        let prefix = key_prefix(url)?;
        let store = Arc::new(store);
        let inner: Arc<dyn ObjectStore> = store.clone();
        let scoped: Arc<dyn ObjectStore> = match &prefix {
            Some(prefix) => Arc::new(PrefixStore::new(inner, prefix.clone())),
            None => inner,
        };
        Ok(Self {
            store: scoped,
            signer: Some(store),
            prefix,
            stores_attributes: true,
        })
    }

    /// The filesystem backend: cannot sign, and rejects object attributes
    /// rather than ignoring them. Its directory is already the root, so no
    /// prefix applies.
    fn filesystem(fs: LocalFileSystem) -> Self {
        Self {
            store: Arc::new(fs),
            signer: None,
            prefix: None,
            stores_attributes: false,
        }
    }

    /// The bucket-level location of `key`, for signers that bypass `PrefixStore`.
    fn signed_location(&self, key: Path) -> Path {
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
        let location = self.signed_location(object_path(key)?);
        let signer = self
            .signer
            .as_ref()
            .ok_or(WorkspaceStoreError::Unsupported(CANNOT_PRESIGN))?;
        signer
            .signed_url(method, &location, ttl)
            .await
            .map_err(|e| map_err(e, key))
    }

    /// The attributes an upload sends: none on a backend that would reject
    /// them, so `put_stream_with` behaves the same everywhere.
    fn upload_attributes(&self, opts: PutOptions) -> Attributes {
        if self.stores_attributes {
            attributes(opts)
        } else {
            Attributes::default()
        }
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
    async fn put_stream_with(
        &self,
        key: &str,
        body: ByteStream,
        len: Option<u64>,
        opts: PutOptions,
    ) -> Result<ObjectMeta, WorkspaceStoreError> {
        let path = object_path(key)?;
        let multipart_opts = PutMultipartOpts {
            attributes: self.upload_attributes(opts),
            ..PutMultipartOpts::default()
        };
        let upload = self
            .store
            .put_multipart_opts(&path, multipart_opts)
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
        let path = object_path(key)?;
        let options = GetOptions {
            range: range.map(byte_range),
            ..GetOptions::default()
        };
        let result = self
            .store
            .get_opts(&path, options)
            .await
            .map_err(|e| map_err(e, key))?;
        let key = key.to_owned();
        Ok(Box::pin(
            result.into_stream().map_err(move |e| map_err(e, &key)),
        ))
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>, WorkspaceStoreError> {
        let path = object_path(key)?;
        match self.store.head(&path).await {
            Ok(meta) => Ok(Some(ObjectMeta {
                size: meta.size as u64,
                etag: meta.e_tag,
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(err) => Err(map_err(err, key)),
        }
    }

    async fn delete(&self, key: &str) -> Result<(), WorkspaceStoreError> {
        let path = object_path(key)?;
        match self.store.delete(&path).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(err) => Err(map_err(err, key)),
        }
    }

    async fn list_prefix(&self, prefix: &str) -> Result<KeyStream, WorkspaceStoreError> {
        let path = prefix_path(prefix)?;
        // `ObjectStore::list` borrows the store for the stream's lifetime; the
        // generator owns a handle so the returned stream is self-contained.
        let store = Arc::clone(&self.store);
        let prefix = prefix.to_owned();
        Ok(Box::pin(async_stream::stream! {
            let mut entries = store.list(Some(&path));
            while let Some(entry) = entries.next().await {
                yield entry.map(|meta| meta.location.to_string()).map_err(|e| map_err(e, &prefix));
            }
        }))
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<u64, WorkspaceStoreError> {
        let path = prefix_path(prefix)?;
        let locations = self
            .store
            .list(Some(&path))
            .map_ok(|meta| meta.location)
            .boxed();
        let mut deleted = self.store.delete_stream(locations);
        let mut count = 0u64;
        while let Some(result) = deleted.next().await {
            match result {
                Ok(_) => count += 1,
                // Gone since it was listed — another sweeper got there first.
                // Absence is the goal, so this is neither an error nor a
                // deletion of ours.
                Err(object_store::Error::NotFound { .. }) => {}
                Err(err) => return Err(map_err(err, prefix)),
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn is_invalid_key(result: Result<Path, WorkspaceStoreError>) -> bool {
        matches!(result, Err(WorkspaceStoreError::InvalidKey { .. }))
    }

    #[test]
    fn valid_keys_parse_verbatim() {
        for key in ["workspace/x/y/blobs/z", "a", "a.b/c-d_e", "with space/f"] {
            let path = object_path(key).unwrap_or_else(|e| panic!("{key}: {e}"));
            assert_eq!(path.as_ref(), key);
        }
    }

    #[test]
    fn keys_object_store_would_rewrite_are_rejected() {
        for key in [
            "a%b", "a//b", "/a", "a/", "", "a/../b", "a/./b", "a#b", "a?b", "résumé",
        ] {
            assert!(is_invalid_key(object_path(key)), "{key:?} must be rejected");
        }
    }

    #[test]
    fn prefixes_allow_one_trailing_slash_and_empty_means_root() {
        assert_eq!(prefix_path("workspace/p/").unwrap().as_ref(), "workspace/p");
        assert_eq!(prefix_path("workspace/p").unwrap().as_ref(), "workspace/p");
        assert_eq!(prefix_path("").unwrap().as_ref(), "");
        assert_eq!(prefix_path("/").unwrap().as_ref(), "");
        assert!(is_invalid_key(prefix_path("workspace//p/")));
        assert!(is_invalid_key(prefix_path("workspace/p//")));
    }

    #[test]
    fn url_path_prefix_is_validated_like_a_key() {
        let prefix = |url: &str| key_prefix(&Url::parse(url).expect("url"));
        assert_eq!(prefix("s3://bucket").unwrap(), None);
        assert_eq!(prefix("s3://bucket/").unwrap(), None);
        assert_eq!(
            prefix("s3://bucket/team/a/")
                .unwrap()
                .map(|p| p.to_string()),
            Some("team/a".into())
        );
        for url in ["s3://bucket/a%20b", "s3://bucket//a", "s3://bucket/a//b"] {
            assert!(
                matches!(prefix(url), Err(WorkspaceStoreError::InvalidUrl(_))),
                "{url} must be rejected"
            );
        }
    }
}
