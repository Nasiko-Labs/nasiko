use std::sync::Arc;

use async_trait::async_trait;
use nasiko_config::Config;
use nasiko_runtime::{
    ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, Result as RuntimeResult,
    RuntimeState,
};
use nasiko_server::state::AppState;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// ─── Infra endpoints — overridable via env for CI ────────────────────────────

fn pg_admin_url() -> String {
    std::env::var("TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into())
}

fn redis_url() -> String {
    std::env::var("TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6379".into())
}

fn s3_endpoint() -> String {
    std::env::var("TEST_S3_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:9000".into())
}

// ─── FakeRuntime ─────────────────────────────────────────────────────────────

/// No-op ContainerRuntime for integration tests.
///
/// Tests exercise HTTP routes, ACL, and DB state — not actual container
/// deployment. Using FakeRuntime eliminates the Docker daemon dependency from
/// CI and removes ~300ms of Docker ping overhead per test.
struct FakeRuntime;

fn fake_status(container_id: &ContainerId) -> DeploymentStatus {
    DeploymentStatus {
        container_id: container_id.clone(),
        state: RuntimeState::Running,
        replicas_live: 1,
        endpoint: Some("http://localhost:8000".into()),
        message: None,
        restart_count: 0,
    }
}

#[async_trait]
impl ContainerRuntime for FakeRuntime {
    async fn deploy(&self, spec: &DeploymentSpec) -> RuntimeResult<DeploymentStatus> {
        Ok(fake_status(&spec.container_id))
    }

    async fn destroy(&self, _container_id: &ContainerId) -> RuntimeResult<()> {
        Ok(())
    }

    async fn scale(&self, _container_id: &ContainerId, _replicas: u32) -> RuntimeResult<()> {
        Ok(())
    }

    async fn restart(&self, _container_id: &ContainerId) -> RuntimeResult<()> {
        Ok(())
    }

    async fn status(&self, container_id: &ContainerId) -> RuntimeResult<DeploymentStatus> {
        Ok(fake_status(container_id))
    }

    async fn list(&self) -> RuntimeResult<Vec<DeploymentStatus>> {
        Ok(vec![])
    }

    async fn endpoint(&self, _container_id: &ContainerId) -> RuntimeResult<String> {
        Ok("http://localhost:8000".into())
    }

    async fn logs(&self, _container_id: &ContainerId, _tail: u32) -> RuntimeResult<Vec<String>> {
        Ok(vec![])
    }

    async fn build(&self, _tar_context: &[u8], image_tag: &str) -> RuntimeResult<String> {
        Ok(image_tag.to_owned())
    }
}

// ─── TestServer ──────────────────────────────────────────────────────────────

/// A running test server bound to a random port, backed by an isolated DB.
/// Call `cleanup().await` at the end of each test to drop the test database.
#[allow(dead_code)]
pub struct TestServer {
    pub base_url: String,
    pub client: reqwest::Client,
    /// Direct pool access for tests that need to seed or verify DB state.
    #[allow(dead_code)]
    pub db: PgPool,
    db_name: String,
    admin_pool: PgPool,
}

/// Single-connection pool options used throughout tests.
///
/// max_connections(1) keeps shared memory usage minimal — Postgres allocates
/// DSM segments per connection for parallel workers, and a per-test admin pool
/// of default size (10) quickly exhausts the system's shared-memory budget
/// when many test DBs are created in sequence.
fn minimal_pool_opts() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .idle_timeout(std::time::Duration::from_secs(1))
        .acquire_timeout(std::time::Duration::from_secs(30))
}

impl TestServer {
    pub async fn start() -> Self {
        let pg_admin = pg_admin_url();
        let db_name = format!("nasiko_test_{}", Uuid::new_v4().simple());

        let admin = minimal_pool_opts()
            .connect(&pg_admin)
            .await
            .expect("connect to postgres — is the DB available? (set TEST_PG_URL to override)");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin)
            .await
            .expect("create test database");

        // Build db_url from pg_admin_url by replacing the database name.
        let db_url = {
            let base = pg_admin.rsplitn(2, '/').last().unwrap_or(&pg_admin);
            format!("{base}/{db_name}")
        };

        let db = minimal_pool_opts()
            .connect(&db_url)
            .await
            .expect("connect to test db");

        let s3_ep = s3_endpoint();
        let config = test_config(db_url, redis_url(), s3_ep.clone());

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        let auth: Arc<dyn nasiko_auth::AuthService> =
            Arc::new(nasiko_auth::AuthServiceImpl::new(db.clone(), jwt_secret));

        let runtime: Arc<dyn ContainerRuntime> = Arc::new(FakeRuntime);

        let state = AppState::from_config_with_db(config, auth, runtime, db.clone()).await;

        let app = nasiko_server::build_app(state, fallback);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        TestServer {
            base_url: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
            db: db.clone(),
            db_name,
            admin_pool: admin,
        }
    }

    pub async fn cleanup(&self) {
        // Terminate connections to the test DB before dropping it.
        sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
            self.db_name
        ))
        .execute(&self.admin_pool)
        .await
        .ok();

        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
            .execute(&self.admin_pool)
            .await
            .ok();

        // Close both pools so Postgres can release their DSM segments immediately.
        self.db.close().await;
        self.admin_pool.close().await;
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

fn test_config(db_url: String, redis_url: String, s3_endpoint: String) -> Config {
    // SAFETY: tests run serially via #[serial], so no concurrent env mutation.
    unsafe {
        std::env::set_var("JWT_SECRET", "test-secret-for-nasiko-tests");
        std::env::set_var("S3_ENDPOINT", &s3_endpoint);
        std::env::set_var("S3_ACCESS_KEY", "nasiko");
        std::env::set_var("S3_SECRET_KEY", "nasiko123");
        std::env::set_var("S3_REGION", "us-east-1");
        // 32 bytes of 0x41 ('A'), base64-encoded — required by SecretsCrypto::load_master_key()
        std::env::set_var("SECRETS_ENCRYPTION_KEY", "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=");
    }

    Config {
        bind: "127.0.0.1:0".into(),
        domain: None,
        database_url: db_url,
        redis_url,
        agent_runtime: "local".into(),
        k8s_namespace: "nasiko-test".into(),
        kubeconfig: None,
        s3_endpoint,
        s3_bucket: "nasiko-test".into(),
        s3_access_key: "nasiko".into(),
        s3_secret_key: "nasiko123".into(),
        s3_region: "us-east-1".into(),
        secrets_encryption_key: "12345678901234567890123456789012".into(),
        oci_storage_bucket: "nasiko-test-artifacts".into(),
        agent_image_registry: String::new(),
        seed_agents: None,
        openai_api_key: None,
        openai_base_url: None,
        openai_model: "gpt-4o".into(),
        router_model: "gpt-4o".into(),
        capability_generator_model: "gpt-4o".into(),
        a2a_discovery_url: None,
        otel_endpoint: None,
        otel_protocol: "grpc".into(),
        otel_headers: None,
        otel_service_name: "nasiko-test".into(),
        otel_sample_ratio: "0.0".into(),
        otel_collector_endpoint: "http://localhost:4318".into(),
        otel_capture_content: false,
        tempo_url: "http://localhost:3200".into(),
        loki_url: "http://localhost:3100".into(),
        flow_max_depth: 5,
        flow_max_fan_out: 20,
        flow_max_tokens: 100_000,
        flow_timeout_secs: 120,
        github_client_id: None,
        github_client_secret: None,
        router_shortlist_threshold: 15,
        router_shortlist_size: 10,
        max_router_history_messages: 20,
        embedding_model: "text-embedding-3-small".into(),
        router_agent_timeout_secs: 60,
        github_callback_url: None,
        docker_agent_network: None,
        oci_registry_host: None,
        git_clone_allowed_hosts: vec![
            "github.com".to_owned(),
            "gitlab.com".to_owned(),
            "bitbucket.org".to_owned(),
        ],
        registry_import_allowed_hosts: vec![],
        cors_allowed_origins: vec![],
        admin_username: "admin".into(),
        admin_password: "test-admin-password".into(),
    }
}

async fn fallback() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
}

// ─── JWT test helpers ─────────────────────────────────────────────────────────

/// JWT secret used in all OSS integration tests — must match test_config().
pub const TEST_JWT_SECRET: &str = "test-secret-for-nasiko-tests";

/// Sign a short-lived JWT for a test identity using the known test secret.
#[allow(dead_code)]
pub fn sign_token(user_id: &str, username: &str, is_superuser: bool, _role: &str) -> String {
    // _role is accepted for call-site compatibility but no longer part of Identity —
    // role is an internal EE detail resolved from the DB, never carried in the token.
    let identity = nasiko_auth::Identity {
        user_id: user_id.to_owned(),
        username: username.to_owned(),
        is_superuser,
    };
    nasiko_auth::jwt::encode_jwt(TEST_JWT_SECRET, 3600, &identity)
        .expect("test JWT signing failed")
}

/// Attach a superuser (admin role) JWT to a request builder.
#[allow(dead_code)]
pub fn as_superuser(
    rb: reqwest::RequestBuilder,
    user_id: &str,
    username: &str,
) -> reqwest::RequestBuilder {
    rb.bearer_auth(sign_token(user_id, username, true, "admin"))
}

/// Attach a member JWT to a request builder.
#[allow(dead_code)]
pub fn as_member(
    rb: reqwest::RequestBuilder,
    user_id: &str,
    username: &str,
) -> reqwest::RequestBuilder {
    rb.bearer_auth(sign_token(user_id, username, false, "member"))
}

/// Build an in-memory zip archive from `(path, bytes)` pairs.
#[allow(dead_code)]
pub fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }
    cursor.into_inner()
}
