use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{Client, config::Credentials, presigning::PresigningConfig};
use std::time::Duration;

use crate::error::{OciError, Result};

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

    /// Construct from S3_* environment variables.
    pub async fn from_env(bucket: String) -> Self {
        let endpoint = std::env::var("S3_ENDPOINT").ok();
        let region = std::env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".into());
        let access_key = std::env::var("S3_ACCESS_KEY").unwrap_or_default();
        let secret_key = std::env::var("S3_SECRET_KEY").unwrap_or_default();

        Self::new(endpoint, region, access_key, secret_key, bucket, true)
            .await
            .expect("failed to create S3 client")
    }

    pub fn blob_key(digest: &str) -> String {
        format!("blobs/{}", digest.replace(':', "/"))
    }

    pub async fn put_blob(&self, digest: &str, data: bytes::Bytes) -> Result<i64> {
        let key = Self::blob_key(digest);
        let size = data.len() as i64;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(data.into())
            .send()
            .await
            .map_err(|e| OciError::Storage(e.to_string()))?;
        Ok(size)
    }

    pub async fn get_blob(&self, digest: &str) -> Result<bytes::Bytes> {
        let key = Self::blob_key(digest);
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| OciError::Storage(e.to_string()))?;
        let data = resp
            .body
            .collect()
            .await
            .map_err(|e| OciError::Storage(e.to_string()))?;
        Ok(data.into_bytes())
    }

    pub async fn presigned_get_url(&self, digest: &str, ttl_secs: u64) -> Result<String> {
        let key = Self::blob_key(digest);
        let config = PresigningConfig::expires_in(Duration::from_secs(ttl_secs))
            .map_err(|e| OciError::Storage(e.to_string()))?;
        let url = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .presigned(config)
            .await
            .map_err(|e| OciError::Storage(e.to_string()))?;
        Ok(url.uri().to_string())
    }

    pub async fn delete_blob(&self, digest: &str) -> Result<()> {
        let key = Self::blob_key(digest);
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| OciError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn blob_exists(&self, digest: &str) -> bool {
        let key = Self::blob_key(digest);
        self.client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .is_ok()
    }

    pub async fn blob_size(&self, digest: &str) -> Result<i64> {
        let key = Self::blob_key(digest);
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| OciError::NotFound(e.to_string()))?;
        Ok(resp.content_length.unwrap_or(0))
    }

    pub async fn ensure_bucket(&self, skip_create: bool) -> std::result::Result<(), anyhow::Error> {
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
