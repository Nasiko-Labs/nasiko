use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{
    Client,
    config::Credentials,
    error::ProvideErrorMetadata,
    presigning::PresigningConfig,
    types::{CompletedMultipartUpload, CompletedPart},
};
use bytes::{Bytes, BytesMut};
use nasiko_runtime::{BlobStore, BlobStoreError, BlobStream, BlobWriter};
use std::time::Duration;

/// Renders an S3 failure with the detail needed to act on it.
///
/// `SdkError`'s own `Display` is only a category — "service error", "dispatch
/// failure" — so a plain `e.to_string()` collapses a wrong `S3_SECRET_KEY` into
/// an undiagnosable "storage error". The code that names the actual fault
/// (`SignatureDoesNotMatch`, `InvalidAccessKeyId`, `AccessDenied`,
/// `NoSuchBucket`) lives in the error metadata and the transport cause sits
/// further down the source chain. Both matter once the bucket can be a managed
/// service the operator wired up by hand, where credentials and endpoint are the
/// likeliest things to be wrong. Only the log carries this — the `/v2` response
/// body stays generic, since a registry client is not the audience.
fn s3_error<E>(err: &E) -> String
where
    E: ProvideErrorMetadata + std::error::Error,
{
    let mut out = String::new();
    if let Some(code) = err.code() {
        out.push_str(code);
        out.push_str(": ");
    }
    out.push_str(&err.to_string());
    let mut source = err.source();
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

#[derive(Clone)]
pub struct S3Storage {
    client: Client,
    bucket: String,
}

impl S3Storage {
    pub async fn new(
        endpoint: Option<String>,
        region: String,
        access_key: String,
        secret_key: String,
        bucket: String,
        force_path_style: bool,
    ) -> std::result::Result<Self, anyhow::Error> {
        let creds = Credentials::new(&access_key, &secret_key, None, None, "registry");
        let region = Region::new(region);

        let mut builder = aws_config::defaults(BehaviorVersion::latest())
            .region(region)
            .credentials_provider(creds);

        if let Some(ep) = endpoint {
            builder = builder.endpoint_url(ep);
        }

        let sdk_config = builder.load().await;
        let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
            .force_path_style(force_path_style)
            .build();

        let client = Client::from_conf(s3_config);
        Ok(Self { client, bucket })
    }

    /// Construct from S3_* environment variables (including
    /// `S3_FORCE_PATH_STYLE`, see [`force_path_style_from_env`]).
    pub async fn from_env(bucket: String) -> Self {
        let endpoint = std::env::var("S3_ENDPOINT").ok();
        let region = std::env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".into());
        let access_key = std::env::var("S3_ACCESS_KEY").unwrap_or_else(|_| "nasiko".into());
        let secret_key = std::env::var("S3_SECRET_KEY").unwrap_or_default();

        Self::new(
            endpoint,
            region,
            access_key,
            secret_key,
            bucket,
            force_path_style_from_env(),
        )
        .await
        .expect("failed to create S3 client")
    }

    pub fn blob_key(digest: &str) -> String {
        format!("blobs/{}", digest.replace(':', "/"))
    }

    async fn put_blob_at(&self, key: &str, data: Bytes) -> Result<i64, BlobStoreError> {
        let size = data.len() as i64;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(data.into())
            .send()
            .await
            .map_err(|e| BlobStoreError::Backend(s3_error(&e)))?;
        Ok(size)
    }
}

#[async_trait::async_trait]
impl BlobStore for S3Storage {
    async fn put_blob(&self, digest: &str, data: bytes::Bytes) -> Result<i64, BlobStoreError> {
        self.put_blob_at(&Self::blob_key(digest), data).await
    }

    async fn get_blob(&self, digest: &str) -> Result<bytes::Bytes, BlobStoreError> {
        let key = Self::blob_key(digest);
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            // A missing object is "not found", not a storage failure. Collapsing
            // both into `Backend` made a pull of an absent blob a 500, where the
            // Distribution Spec requires 404 — and disagreed with `blob_size`,
            // which already reports absence as `NotFound`, so HEAD and GET on the
            // same missing digest answered differently.
            .map_err(|e| {
                let msg = s3_error(&e);
                if e.into_service_error().is_no_such_key() {
                    BlobStoreError::NotFound(format!("blob {digest} not found"))
                } else {
                    BlobStoreError::Backend(msg)
                }
            })?;
        let data = resp
            .body
            .collect()
            .await
            .map_err(|e| BlobStoreError::Backend(e.to_string()))?;
        Ok(data.into_bytes())
    }

    async fn get_blob_stream(&self, digest: &str) -> Result<BlobStream, BlobStoreError> {
        let key = Self::blob_key(digest);
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                let msg = s3_error(&e);
                if e.into_service_error().is_no_such_key() {
                    BlobStoreError::NotFound(format!("blob {digest} not found"))
                } else {
                    BlobStoreError::Backend(msg)
                }
            })?;
        let pieces = futures::stream::unfold(resp.body, |mut body| async move {
            let piece = body
                .try_next()
                .await
                .map_err(|e| BlobStoreError::Backend(e.to_string()))
                .transpose()?;
            Some((piece, body))
        });
        Ok(Box::pin(pieces))
    }

    async fn blob_writer<'a>(
        &'a self,
        digest: &str,
    ) -> Result<Box<dyn BlobWriter + 'a>, BlobStoreError> {
        Ok(Box::new(S3BlobWriter {
            storage: self,
            key: Self::blob_key(digest),
            pending: BytesMut::new(),
            upload_id: None,
            parts: Vec::new(),
            size: 0,
        }))
    }

    async fn presigned_get_url(
        &self,
        digest: &str,
        ttl_secs: u64,
    ) -> Result<String, BlobStoreError> {
        let key = Self::blob_key(digest);
        let config = PresigningConfig::expires_in(Duration::from_secs(ttl_secs))
            .map_err(|e| BlobStoreError::Backend(e.to_string()))?;
        let url = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .presigned(config)
            .await
            .map_err(|e| BlobStoreError::Backend(e.to_string()))?;
        Ok(url.uri().to_string())
    }

    async fn delete_blob(&self, digest: &str) -> Result<(), BlobStoreError> {
        let key = Self::blob_key(digest);
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| BlobStoreError::Backend(s3_error(&e)))?;
        Ok(())
    }

    async fn blob_exists(&self, digest: &str) -> bool {
        let key = Self::blob_key(digest);
        self.client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .is_ok()
    }

    async fn blob_size(&self, digest: &str) -> Result<i64, BlobStoreError> {
        let key = Self::blob_key(digest);
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            // Every HEAD failure reads as absence, including a rejected
            // credential — imprecise, but preserved verbatim from before the
            // trait seam so this refactor changes no status code. The detail
            // `s3_error` carries still names the real cause in the log.
            .map_err(|e| BlobStoreError::NotFound(s3_error(&e)))?;
        Ok(resp.content_length.unwrap_or(0))
    }

    async fn ensure_bucket(&self, skip_create: bool) -> std::result::Result<(), anyhow::Error> {
        let exists = self
            .client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_ok();

        if exists {
            return Ok(());
        }

        if skip_create {
            anyhow::bail!(
                "S3 bucket '{}' not found. Create it first (skip_create=true).",
                self.bucket
            );
        }

        self.client
            .create_bucket()
            .bucket(&self.bucket)
            .send()
            .await?;
        tracing::info!("created S3 bucket: {}", self.bucket);
        Ok(())
    }
}

/// Size of each part of a multipart write: above S3's 5 MiB minimum for every
/// part but the last, and few enough parts for the registry's largest blob
/// (5 GiB is 320 of them, against S3's 10,000).
const MULTIPART_PART_BYTES: usize = 16 * 1024 * 1024;

/// A blob written to S3 in parts. It holds at most one part in memory. An object
/// smaller than one part is a single `PutObject` at `finish`; a larger one is a
/// multipart upload, started with the first full part, that S3 shows under the
/// key only once it is completed.
struct S3BlobWriter<'a> {
    storage: &'a S3Storage,
    key: String,
    pending: BytesMut,
    upload_id: Option<String>,
    parts: Vec<CompletedPart>,
    size: i64,
}

impl S3BlobWriter<'_> {
    async fn upload_id(&mut self) -> Result<String, BlobStoreError> {
        if let Some(id) = &self.upload_id {
            return Ok(id.clone());
        }
        let created = self
            .storage
            .client
            .create_multipart_upload()
            .bucket(&self.storage.bucket)
            .key(&self.key)
            .send()
            .await
            .map_err(|e| BlobStoreError::Backend(s3_error(&e)))?;
        let id = created.upload_id.ok_or_else(|| {
            BlobStoreError::Backend(format!("no upload id for multipart write of {}", self.key))
        })?;
        self.upload_id = Some(id.clone());
        Ok(id)
    }

    async fn upload_part(&mut self, data: Bytes) -> Result<(), BlobStoreError> {
        let upload_id = self.upload_id().await?;
        let part_number = self.parts.len() as i32 + 1;
        let uploaded = self
            .storage
            .client
            .upload_part()
            .bucket(&self.storage.bucket)
            .key(&self.key)
            .upload_id(upload_id)
            .part_number(part_number)
            .body(data.into())
            .send()
            .await
            .map_err(|e| BlobStoreError::Backend(s3_error(&e)))?;
        self.parts.push(
            CompletedPart::builder()
                .set_e_tag(uploaded.e_tag)
                .part_number(part_number)
                .build(),
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl BlobWriter for S3BlobWriter<'_> {
    async fn write(&mut self, data: Bytes) -> Result<(), BlobStoreError> {
        self.size += data.len() as i64;
        self.pending.extend_from_slice(&data);
        while self.pending.len() >= MULTIPART_PART_BYTES {
            let part = self.pending.split_to(MULTIPART_PART_BYTES).freeze();
            self.upload_part(part).await?;
        }
        Ok(())
    }

    async fn finish(mut self: Box<Self>) -> Result<i64, BlobStoreError> {
        let rest = std::mem::take(&mut self.pending).freeze();
        let Some(upload_id) = self.upload_id.clone() else {
            return self.storage.put_blob_at(&self.key, rest).await;
        };
        if !rest.is_empty() {
            self.upload_part(rest).await?;
        }
        self.storage
            .client
            .complete_multipart_upload()
            .bucket(&self.storage.bucket)
            .key(&self.key)
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(std::mem::take(&mut self.parts)))
                    .build(),
            )
            .send()
            .await
            .map_err(|e| BlobStoreError::Backend(s3_error(&e)))?;
        // Completed: nothing left for `Drop` to abort.
        self.upload_id = None;
        Ok(self.size)
    }

    async fn abort(mut self: Box<Self>) {
        if let Some(upload_id) = self.upload_id.take() {
            abort_multipart(
                self.storage.client.clone(),
                self.storage.bucket.clone(),
                self.key.clone(),
                upload_id,
            )
            .await;
        }
    }
}

/// A writer dropped before it finished — its request cancelled when the client
/// went away, or a failed `finish` — aborts its multipart upload, since S3 keeps
/// an incomplete upload's parts (and bills them) until something aborts it.
impl Drop for S3BlobWriter<'_> {
    fn drop(&mut self) {
        let Some(upload_id) = self.upload_id.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(key = %self.key, "multipart write dropped outside a runtime; parts left");
            return;
        };
        runtime.spawn(abort_multipart(
            self.storage.client.clone(),
            self.storage.bucket.clone(),
            self.key.clone(),
            upload_id,
        ));
    }
}

async fn abort_multipart(client: Client, bucket: String, key: String, upload_id: String) {
    if let Err(e) = client
        .abort_multipart_upload()
        .bucket(bucket)
        .key(&key)
        .upload_id(upload_id)
        .send()
        .await
    {
        tracing::warn!(key = %key, error = %s3_error(&e), "abort multipart write");
    }
}

/// `S3_FORCE_PATH_STYLE`: path-style requests (`endpoint.com/bucket/key`),
/// defaulting **true** — RustFS/MinIO (every in-cluster install) require it,
/// so only an explicit `false`/`0` switches to virtual-hosted style. The
/// negative parse keeps a typo'd value from silently flipping the default
/// out from under existing stores.
pub fn force_path_style_from_env() -> bool {
    parse_force_path_style(std::env::var("S3_FORCE_PATH_STYLE").ok().as_deref())
}

fn parse_force_path_style(value: Option<&str>) -> bool {
    !matches!(value.map(str::trim), Some("false") | Some("0"))
}

#[cfg(test)]
mod tests {
    use super::parse_force_path_style;

    #[test]
    fn force_path_style_defaults_true_and_only_explicit_false_disables() {
        assert!(parse_force_path_style(None));
        assert!(parse_force_path_style(Some("true")));
        assert!(parse_force_path_style(Some("1")));
        assert!(parse_force_path_style(Some("garbage")));
        assert!(!parse_force_path_style(Some("false")));
        assert!(!parse_force_path_style(Some("0")));
    }
}

/// Against a real S3 endpoint (the local RustFS is enough):
/// `cargo test -p nasiko-oci --lib -- --ignored dropped_writer`
#[cfg(test)]
mod s3_tests {
    use super::*;

    async fn open_uploads(storage: &S3Storage, key: &str) -> usize {
        storage
            .client
            .list_multipart_uploads()
            .bucket(&storage.bucket)
            .prefix(key)
            .send()
            .await
            .expect("list multipart uploads")
            .uploads
            .unwrap_or_default()
            .len()
    }

    #[tokio::test]
    #[ignore = "needs an S3 endpoint (S3_* env)"]
    async fn a_dropped_writer_aborts_its_multipart_upload() {
        let storage = S3Storage::from_env("nasiko-test-artifacts".into()).await;
        storage.ensure_bucket(false).await.unwrap();
        let digest = format!("test:dropped-{}", std::process::id());
        let key = S3Storage::blob_key(&digest);

        let mut writer = storage.blob_writer(&digest).await.unwrap();
        writer
            .write(Bytes::from(vec![7u8; MULTIPART_PART_BYTES + 1]))
            .await
            .unwrap();
        assert_eq!(open_uploads(&storage, &key).await, 1, "a part was uploaded");

        drop(writer);
        for _ in 0..50 {
            if open_uploads(&storage, &key).await == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(open_uploads(&storage, &key).await, 0);
        assert!(!storage.blob_exists(&digest).await);
    }
}
