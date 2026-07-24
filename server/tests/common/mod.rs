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
    std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into())
}

fn s3_endpoint() -> String {
    std::env::var("TEST_S3_ENDPOINT").unwrap_or_else(|_| "http://localhost:9000".into())
}

// ─── FakeRuntime ─────────────────────────────────────────────────────────────

/// No-op ContainerRuntime for integration tests.
///
/// Tests exercise HTTP routes, ACL, and DB state — not actual container
/// deployment. Using FakeRuntime eliminates the Docker daemon dependency from
/// CI and removes ~300ms of Docker ping overhead per test.
///
/// Tracks deployed containers in memory (`deploy`/`destroy` update it, `list`
/// reads it) so tests can exercise `list`'s per-caller filtering without a
/// real runtime.
#[derive(Default)]
struct FakeRuntime {
    containers: std::sync::Mutex<std::collections::HashMap<ContainerId, DeploymentStatus>>,
}

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
        let status = fake_status(&spec.container_id);
        self.containers
            .lock()
            .unwrap()
            .insert(spec.container_id.clone(), status.clone());
        Ok(status)
    }

    async fn destroy(&self, container_id: &ContainerId) -> RuntimeResult<()> {
        self.containers.lock().unwrap().remove(container_id);
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
        Ok(self.containers.lock().unwrap().values().cloned().collect())
    }

    async fn endpoint(&self, container_id: &ContainerId) -> RuntimeResult<String> {
        // Mirror a real runtime: a container that was never deployed can't
        // resolve. The agent proxy prefers this live lookup and only falls
        // back to the stored `agents.url` when it errors, so tests that seed
        // `agents.url` directly (no runtime deploy) rely on this failing.
        if self.containers.lock().unwrap().contains_key(container_id) {
            Ok("http://localhost:8000".into())
        } else {
            Err(nasiko_runtime::RuntimeError::ContainerNotFound(
                container_id.clone(),
            ))
        }
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
    #[allow(dead_code)]
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// Same as [`start`](Self::start), but lets the caller override fields on
    /// the generated `Config` before the server boots — e.g. setting
    /// `build_push_token` for tests exercising the build-service OCI auth
    /// path, which `test_config()` otherwise always leaves empty.
    #[allow(dead_code)]
    pub async fn start_with(configure: impl FnOnce(&mut Config)) -> Self {
        Self::start_with_runtime(configure, Arc::new(FakeRuntime::default())).await
    }

    /// Same as [`start_with`](Self::start_with), but lets the caller supply a
    /// real `ContainerRuntime` (e.g. a real `DockerRuntime`) instead of
    /// `FakeRuntime` — needed by tests that must observe a real container's
    /// lifecycle through the actual HTTP route (not by calling orchestration
    /// functions directly), such as confirming `DELETE
    /// /api/mcp/connectors/{id}` really destroys an uploaded connector's
    /// container.
    #[allow(dead_code)]
    pub async fn start_with_runtime(
        configure: impl FnOnce(&mut Config),
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Self {
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

        // The fresh per-test DB starts empty; apply migrations so auth
        // (auth_tokens revocation lookup), routing, and agent tables exist.
        // Without this every request fails closed at `require_auth` with 401
        // (or, for routes that don't hit auth first, a generic query error).
        nasiko_server::state::AppState::run_migrations(&db).await;

        let s3_ep = s3_endpoint();
        let mut config = test_config(db_url, redis_url(), s3_ep.clone());
        configure(&mut config);

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        let auth: Arc<dyn nasiko_auth::AuthService> =
            Arc::new(nasiko_auth::AuthServiceImpl::new(db.clone(), jwt_secret));

        let state = AppState::from_config_with_db(config, auth, runtime, db.clone()).await;

        let app = nasiko_server::build_app(state, fallback);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        std::env::set_var(
            "SECRETS_ENCRYPTION_KEY",
            "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=",
        );
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
        build_push_token: String::new(),
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
        observability_enabled: false,
        flow_max_depth: 5,
        flow_max_fan_out: 20,
        flow_max_tokens: 100_000,
        flow_timeout_secs: 120,
        github_client_id: None,
        github_client_secret: None,
        oidc_issuer_url: None,
        oidc_client_id: None,
        oidc_client_secret: None,
        oidc_redirect_uri: None,
        oidc_allowed_redirect_origins: vec![],
        oidc_scopes: "openid profile email".into(),
        oidc_provider_label: "microsoft_entra".into(),
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
        // Overridable so tests can point the Composio ToolProvider at a mockito
        // stub (there is no other seam to inject a fake provider — McpState builds
        // it straight from these two Config fields). Unset in the vast majority of
        // tests, which must keep seeing "Composio not configured" (COMPOSIO_API_KEY
        // unset in production === `composio_api_key: None`).
        composio_api_key: std::env::var("TEST_COMPOSIO_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        composio_base_url: std::env::var("TEST_COMPOSIO_BASE_URL")
            .unwrap_or_else(|_| "https://backend.composio.dev".into()),
        composio_webhook_secret: std::env::var("COMPOSIO_WEBHOOK_SECRET")
            .ok()
            .filter(|s| !s.is_empty()),
        // Overridable so the MCP OAuth callback round-trip test can exercise the
        // full `exchange_code` path (which needs `oauth_redirect_uri()` to be
        // `Some`). `None` by default, matching every other test's assumption
        // that the gateway's public URL is unconfigured.
        mcp_gateway_public_url: std::env::var("TEST_MCP_GATEWAY_PUBLIC_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        // None here too: falls back to mcp_gateway_public_url above, which the
        // oauth callback test already sets to an HTTPS test domain — no
        // separate override needed for that test to keep passing.
        mcp_oauth_redirect_base_url: None,
        mcp_session_ttl_seconds: 300,
        mcp_perm_cache_ttl_seconds: 30,
        mcp_manifest_ttl_seconds: 300,
        mcp_upload_max_bytes: 50 * 1024 * 1024,
        mcp_upload_default_port: 8080,
        mcp_servers_network: "nasiko-mcp-servers-net".to_string(),
        mcp_upload_max_replicas: 1,
        mcp_toolcount_ttl_seconds: 3600,
        app_base_url: "".to_string(),
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
    nasiko_auth::jwt::encode_jwt(TEST_JWT_SECRET, 3600, &identity).expect("test JWT signing failed")
}

/// Sign a short-lived agent-typed JWT (as minted by `issue_agent_token`) —
/// `encode_agent_jwt` stamps `token_type = "agent"`, which `decode_jwt`/
/// `decode_jwt_with_jti` reject outright (AUTH-3), so this must never be
/// usable to authenticate as a user via `require_auth`.
#[allow(dead_code)]
pub fn sign_agent_token(agent_id: &str) -> String {
    let identity = nasiko_auth::Identity {
        user_id: agent_id.to_owned(),
        username: format!("agent:{agent_id}"),
        is_superuser: false,
    };
    nasiko_auth::jwt::encode_agent_jwt(TEST_JWT_SECRET, 3600, &identity)
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

/// Attach HTTP Basic auth — the credential type the OCI registry's pull-only
/// path (`nasiko_oci::pull_credentials`) accepts, distinct from the bearer-JWT
/// paths above.
#[allow(dead_code)]
pub fn as_pull_credential(
    rb: reqwest::RequestBuilder,
    username: &str,
    password: &str,
) -> reqwest::RequestBuilder {
    rb.basic_auth(username, Some(password))
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
