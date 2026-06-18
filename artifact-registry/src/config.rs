use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub s3_endpoint: Option<String>,
    pub s3_region: String,
    pub s3_bucket: String,
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    /// Force S3 path-style URLs (required for MinIO; leave false for AWS S3).
    pub s3_force_path_style: bool,
    /// Skip automatic bucket creation on startup (set true for AWS S3 — create the bucket manually).
    pub s3_skip_bucket_create: bool,
    pub admin_username: String,
    pub admin_password: String,
    pub port: u16,
    pub public_base_url: String,
    pub openai_api_key: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            s3_endpoint: env::var("S3_ENDPOINT").ok(),
            s3_region: env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
            s3_bucket: env::var("S3_BUCKET").unwrap_or_else(|_| "artifacts".into()),
            aws_access_key_id: required("AWS_ACCESS_KEY_ID")?,
            aws_secret_access_key: required("AWS_SECRET_ACCESS_KEY")?,
            s3_force_path_style: env::var("S3_FORCE_PATH_STYLE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            s3_skip_bucket_create: env::var("S3_SKIP_BUCKET_CREATE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            admin_username: env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".into()),
            admin_password: required("ADMIN_PASSWORD")?,
            port: env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()
                .unwrap_or(3000),
            public_base_url: env::var("PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3000".into()),
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
        })
    }
}

fn required(key: &str) -> anyhow::Result<String> {
    env::var(key).map_err(|_| anyhow::anyhow!("missing required env var: {}", key))
}
