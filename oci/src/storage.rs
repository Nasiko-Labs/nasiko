use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{
    Client, config::Credentials, error::ProvideErrorMetadata, presigning::PresigningConfig,
};
use nasiko_runtime::{BlobStore, BlobStoreError};
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
}

#[async_trait::async_trait]
impl BlobStore for S3Storage {
    async fn put_blob(&self, digest: &str, data: bytes::Bytes) -> Result<i64, BlobStoreError> {
        let key = Self::blob_key(digest);
        let size = data.len() as i64;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(data.into())
            .send()
            .await
            .map_err(|e| BlobStoreError::Backend(s3_error(&e)))?;
        Ok(size)
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
