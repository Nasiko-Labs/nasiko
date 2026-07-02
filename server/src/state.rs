use std::sync::Arc;

use nasiko_auth::AuthService;
use nasiko_github::{GitHubConfig, GitHubService};
use nasiko_observability::ObservabilityProvider;
use nasiko_runtime::ContainerRuntime;
use sqlx::PgPool;
use tokio::sync::mpsc;

use nasiko_config::Config;
use nasiko_flow::{FlowConfig, FlowEventBus, FlowGuard};
use crate::telemetry::GenAiMetrics;
use crate::usage::UsageTracker;

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<dyn ContainerRuntime>,
    pub db: PgPool,
    pub redis: redis::Client,
    pub oci_storage: nasiko_oci::storage::S3Storage,
    pub usage_tracker: UsageTracker,
    pub http_client: reqwest::Client,
    pub auth: Arc<dyn AuthService>,
    pub flow_guard: FlowGuard,
    pub flow_events: FlowEventBus,
    pub genai_metrics: GenAiMetrics,
    pub config: Arc<Config>,
    /// Optional Tempo+Loki provider — present when TEMPO_URL + LOKI_URL are configured.
    /// Falls back to DB-only queries when None.
    pub observability: Option<Arc<dyn ObservabilityProvider>>,
    /// Shared GitHubService instance — None if GitHub OAuth is not configured.
    pub github_svc: Option<Arc<GitHubService>>,
    /// Wakes the build worker immediately when a new job is enqueued.
    pub build_tx: mpsc::Sender<()>,
}

impl AppState {
    pub async fn from_config(
        config: Config,
        auth: Arc<dyn AuthService>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Self {
        let db = PgPool::connect(&config.database_url)
            .await
            .expect("failed to connect to postgres");
        Self::from_config_with_db(config, auth, runtime, db).await
    }

    pub async fn from_config_with_db(
        config: Config,
        auth: Arc<dyn AuthService>,
        runtime: Arc<dyn ContainerRuntime>,
        db: PgPool,
    ) -> Self {
        // ignore_missing: EE migrations (v10+) already applied to the DB must not
        // cause the OSS migrator to panic; strict on everything else.
        sqlx::migrate!("../migrations")
            .set_ignore_missing(true)
            .run(&db)
            .await
            .expect("database migration failed");

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

        // Optional Tempo+Loki observability backend.
        // Both TEMPO_URL and LOKI_URL must be set together — a partial config
        // is logged as a warning so operators can spot misconfiguration early.
        let observability: Option<Arc<dyn ObservabilityProvider>> = {
            let tempo_url = std::env::var("TEMPO_URL").ok();
            let loki_url  = std::env::var("LOKI_URL").ok();
            match (tempo_url, loki_url) {
                (Some(t), Some(l)) => {
                    use nasiko_observability::TempoLokiProvider;
                    tracing::info!(tempo_url = %t, loki_url = %l, "observability backend enabled");
                    Some(Arc::new(TempoLokiProvider::new(t, l)))
                }
                (Some(_), None) => {
                    tracing::warn!("TEMPO_URL is set but LOKI_URL is missing — observability backend disabled. Set both to enable.");
                    None
                }
                (None, Some(_)) => {
                    tracing::warn!("LOKI_URL is set but TEMPO_URL is missing — observability backend disabled. Set both to enable.");
                    None
                }
                (None, None) => None,
            }
        };

        let github_svc = config.github_client_id.as_ref()
            .zip(config.github_client_secret.as_ref())
            .and_then(|(id, sec)| {
                let signing = std::env::var("OAUTH_STATE_SIGNING_KEY")
                    .unwrap_or_else(|_| sec.clone());
                let cfg = GitHubConfig {
                    client_id: id.clone(),
                    client_secret: sec.clone(),
                    oauth_state_secret: signing,
                    callback_url: config.github_callback_url.clone()
                        .expect("GITHUB_CALLBACK_URL must be set when GITHUB_CLIENT_ID and GITHUB_CLIENT_SECRET are configured"),
                    central_callback_url: None,
                    clone_timeout_secs: 300,
                    clone_max_size_bytes: 500 * 1024 * 1024,
                };
                GitHubService::new(cfg).ok().map(Arc::new)
            });

        let (build_tx, build_rx) = mpsc::channel(64);

        let state = Self {
            runtime,
            db,
            redis,
            oci_storage,
            usage_tracker,
            http_client,
            auth,
            flow_guard,
            flow_events,
            genai_metrics,
            config: Arc::new(config),
            observability,
            github_svc,
            build_tx,
        };

        // Spawn the durable build worker. It owns the receiver and exits when sender drops.
        let worker_state = state.clone();
        tokio::spawn(crate::agents::build_worker::run(worker_state, build_rx));

        state
    }

    /// Run one-time initialization: bootstrap admin user, seed agents.
    /// Call after the server is constructed but before serving requests.
    pub async fn init(&self) {
        if let (Ok(admin_user), Ok(admin_pass)) = (
            std::env::var("ADMIN_USERNAME"),
            std::env::var("ADMIN_PASSWORD"),
        ) {
            if let Err(e) = self.auth.bootstrap_admin(&admin_user, &admin_pass).await {
                tracing::warn!(%e, "admin bootstrap failed (may already exist)");
            }
        }

        crate::seed::seed_agents_if_configured(self).await;
    }
}
