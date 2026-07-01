use std::sync::Arc;

use nasiko_config::Config;
use nasiko_server::{Providers, state::AppState};
use sqlx::PgPool;
use uuid::Uuid;

// ─── defaults pointing at docker compose --profile infra ────────────────────

const PG_ADMIN_URL: &str = "postgres://nasiko:nasiko@localhost:5432/nasiko_dev";
const REDIS_URL: &str = "redis://localhost:6379";
const S3_ENDPOINT: &str = "http://localhost:9000";

/// A running test server bound to a random port, backed by an isolated DB.
/// Call `cleanup().await` at the end of each test to drop the test database.
#[allow(dead_code)]
pub struct TestServer {
    pub base_url: String,
    pub client: reqwest::Client,
    pub db: PgPool,
    db_name: String,
}

impl TestServer {
    pub async fn start() -> Self {
        let db_name = format!("nasiko_test_{}", Uuid::new_v4().simple());

        // Create isolated test database
        let admin = PgPool::connect(PG_ADMIN_URL)
            .await
            .expect("connect to postgres — is `docker compose --profile infra up -d` running?");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin)
            .await
            .expect("create test database");

        let db_url = format!("postgres://nasiko:nasiko@localhost:5432/{db_name}");
        let db = PgPool::connect(&db_url).await.expect("connect to test db");
        let db_for_server = db.clone();

        let config = test_config(db_url);

        let auth: Arc<dyn nasiko_auth::AuthProvider> =
            Arc::new(nasiko_auth::SimpleJwtAuth::from_env());
        let user_auth = Arc::new(nasiko_auth::UserAuthServiceImpl::new(db.clone(), auth.clone()));
        let providers = Providers {
            auth,
            acl: Arc::new(nasiko_auth::NoopAuthorizer),
            user_auth: user_auth.clone(),
            token_svc: user_auth,
        };

        let runtime: Arc<dyn nasiko_runtime::ContainerRuntime> = Arc::new(
            nasiko_server::runtime::build_docker_runtime(&config)
                .await
                .expect("docker runtime"),
        );

        let state = AppState::from_config_with_db(config, providers, runtime, db_for_server).await;

        let app = nasiko_server::build_app(state, fallback);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give the server a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        TestServer {
            base_url: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
            db,
            db_name,
        }
    }

    pub async fn cleanup(&self) {
        let admin = PgPool::connect(PG_ADMIN_URL).await.unwrap();
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
            .execute(&admin)
            .await
            .ok();
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

fn test_config(db_url: String) -> Config {
    // JWT_SECRET must be set in env for SimpleJwtAuth::from_env()
    // SAFETY: tests run serially via #[serial], so no concurrent env mutation
    unsafe { std::env::set_var("JWT_SECRET", "test-secret-for-nasiko-tests") };

    Config {
        bind: "127.0.0.1:0".into(),
        domain: None,
        database_url: db_url,
        redis_url: REDIS_URL.into(),
        scheduler_mode: "local".into(),
        k8s_namespace: "nasiko-test".into(),
        kubeconfig: None,
        s3_endpoint: S3_ENDPOINT.into(),
        s3_bucket: "nasiko-test".into(),
        s3_access_key: "nasiko".into(),
        s3_secret_key: "nasiko123".into(),
        s3_region: "us-east-1".into(),
        secrets_encryption_key: "12345678901234567890123456789012".into(),
        oci_storage_bucket: "nasiko-test-artifacts".into(),
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
        agent_registry_cache_ttl_secs: 3600,
        router_shortlist_threshold: 15,
        router_shortlist_size: 10,
        max_router_history_messages: 20,
        ollama_url: "http://localhost:11434".into(),
        ollama_embedding_model: "nomic-embed-text".into(),
        router_agent_timeout_secs: 60,
    }
}

async fn fallback() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
}
