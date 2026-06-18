use std::sync::Arc;

use nasiko_auth::{AclChecker, AuthProvider};
use nasiko_runtime::ContainerRuntime;
use sqlx::PgPool;

use nasiko_config::Config;
use crate::flow::{FlowConfig, FlowEventBus, FlowGuard};
use crate::telemetry::GenAiMetrics;
use crate::usage::UsageTracker;

/// Pluggable providers for auth and ACL.
/// OSS uses SingleUserAuth/NoopAcl.
/// Cloud uses OAuthProvider/RbacChecker.
#[derive(Clone)]
pub struct Providers {
    pub auth: Arc<dyn AuthProvider>,
    pub acl: Arc<dyn AclChecker>,
}

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<dyn ContainerRuntime>,
    pub db: PgPool,
    pub redis: redis::Client,
    pub oci_storage: nasiko_oci::storage::S3Storage,
    pub usage_tracker: UsageTracker,
    pub http_client: reqwest::Client,
    pub providers: Providers,
    pub flow_guard: FlowGuard,
    pub flow_events: FlowEventBus,
    pub genai_metrics: GenAiMetrics,
    pub config: Arc<Config>,
}

impl AppState {
    pub async fn from_config(
        config: Config,
        providers: Providers,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Self {
        let db = PgPool::connect(&config.database_url)
            .await
            .expect("failed to connect to postgres");

        sqlx::migrate!("../migrations")
            .run(&db)
            .await
            .expect("failed to run migrations");

        let redis = redis::Client::open(config.redis_url.as_str())
            .expect("invalid redis url");

        let oci_storage = nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await;

        let usage_tracker = UsageTracker::new(db.clone());

        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(20)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build http client");

        let flow_config = FlowConfig {
            max_depth: config.flow_max_depth as u32,
            max_fan_out: config.flow_max_fan_out as u32,
            max_flow_tokens: config.flow_max_tokens as u64,
            flow_timeout_secs: config.flow_timeout_secs as u64,
            flow_state_ttl_secs: 300,
        };
        let flow_guard = FlowGuard::new(redis.clone(), flow_config);
        let flow_events = FlowEventBus::new();
        let genai_metrics = GenAiMetrics::new();

        let state = Self {
            runtime,
            db,
            redis,
            oci_storage,
            usage_tracker,
            http_client,
            providers,
            flow_guard,
            flow_events,
            genai_metrics,
            config: Arc::new(config),
        };

        crate::seed::seed_agents_if_configured(&state).await;

        state
    }
}
