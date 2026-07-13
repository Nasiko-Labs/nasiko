use std::sync::Arc;

use nasiko_auth::AuthService;
use nasiko_github::{GitHubConfig, GitHubService};
use nasiko_orchestrator::RoutingEngine;
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
    pub routing_engine: Arc<dyn RoutingEngine>,
    /// Tempo+Loki observability provider with DB-backed model pricing.
    /// Always constructed — TEMPO_URL/LOKI_URL default to the in-cluster
    /// addresses; queries fail soft when the stack is absent.
    pub observability: Arc<dyn ObservabilityProvider>,
    /// Shared GitHubService instance — None if GitHub OAuth is not configured.
    pub github_svc: Option<Arc<GitHubService>>,
    /// Shared OIDC relying-party client (e.g. Microsoft Entra ID) — None
    /// until `OIDC_ISSUER_URL`/`OIDC_CLIENT_ID`/`OIDC_CLIENT_SECRET`/
    /// `OIDC_REDIRECT_URI` are all configured. See `docs/OIDC_SSO_SETUP.md`.
    pub oidc_svc: Option<Arc<nasiko_oidc::OidcClient>>,
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

    pub async fn run_migrations(db: &PgPool) {
        sqlx::migrate!("../migrations")
            .set_ignore_missing(true)
            .run(db)
            .await
            .expect("database migration failed");
    }

    pub async fn from_config_with_db(
        config: Config,
        auth: Arc<dyn AuthService>,
        runtime: Arc<dyn ContainerRuntime>,
        db: PgPool,
    ) -> Self {
        let redis = redis::Client::open(config.redis_url.as_str())
            .expect("invalid redis url");

        let oci_storage = nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await;
        oci_storage.ensure_bucket(false).await.ok();

        let usage_tracker = UsageTracker::new(db.clone());

        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(20)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build http client");

        let routing_engine: Arc<dyn RoutingEngine> = Arc::new(
            nasiko_orchestrator::OssRoutingEngine::from_config(&config, http_client.clone())
        );

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

        // Tempo+Loki observability backend. Model pricing resolves through
        // the model_pricing DB table with the static table as fallback; the
        // Redis resolver maps trace_id → session_id for pre-built agents.
        let observability: Arc<dyn ObservabilityProvider> = {
            use nasiko_observability::{DbPricing, TempoLokiProvider};
            use crate::observability::session_resolver::RedisSessionIdResolver;
            tracing::info!(
                tempo_url = %config.tempo_url,
                loki_url = %config.loki_url,
                "observability backend configured"
            );
            Arc::new(
                TempoLokiProvider::new(
                    config.tempo_url.clone(),
                    config.loki_url.clone(),
                    Arc::new(DbPricing::new(db.clone())),
                )
                .with_session_resolver(Arc::new(RedisSessionIdResolver::new(redis.clone()))),
            )
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

        let oidc_svc: Option<Arc<nasiko_oidc::OidcClient>> = config
            .oidc_issuer_url
            .as_ref()
            .zip(config.oidc_client_id.as_ref())
            .zip(config.oidc_client_secret.as_ref())
            .zip(config.oidc_redirect_uri.as_ref())
            .map(|(((issuer_url, client_id), client_secret), redirect_uri)| {
                let oidc_config = nasiko_oidc::OidcConfig {
                    issuer_url: issuer_url.clone(),
                    client_id: client_id.clone(),
                    client_secret: client_secret.clone(),
                    redirect_uri: redirect_uri.clone(),
                    scopes: config.oidc_scopes.clone(),
                };
                Arc::new(nasiko_oidc::OidcClient::new(oidc_config, http_client.clone()))
            });

        if let Some(svc) = oidc_svc.clone() {
            // Best-effort discovery warmup — a transient network hiccup at
            // boot must not crash the server; the first real login attempt
            // will just retry discovery lazily if this fails.
            tokio::spawn(async move {
                if let Err(e) = svc.warm().await {
                    tracing::warn!(%e, "OIDC discovery warmup failed at boot — will retry lazily on first login");
                }
            });
        }

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
            routing_engine,
            observability,
            github_svc,
            oidc_svc,
            build_tx,
        };

        // Spawn the durable build worker. It owns the receiver and exits when sender drops.
        let worker_state = state.clone();
        tokio::spawn(crate::agents::build_worker::run(worker_state, build_rx));

        state
    }

    /// Run one-time initialization: bootstrap admin user, spawn seed agents in background,
    /// and start periodic materialized view refresh.
    pub async fn init(&self) {
        if let (Ok(admin_user), Ok(admin_pass)) = (
            std::env::var("ADMIN_USERNAME"),
            std::env::var("ADMIN_PASSWORD"),
        )
            && let Err(e) = self.auth.bootstrap_admin(&admin_user, &admin_pass).await
        {
            tracing::warn!(%e, "admin bootstrap failed (may already exist)");
        }

        let state = self.clone();
        tokio::spawn(async move {
            crate::seed::seed_agents_if_configured(&state).await;
        });

        // Periodic refresh of materialized views (token_usage_daily, agent_selection_stats).
        let db = self.db.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await; // first tick fires immediately — skip it to avoid startup load
            loop {
                interval.tick().await;
                let views = [
                    "REFRESH MATERIALIZED VIEW CONCURRENTLY token_usage_daily",
                    "REFRESH MATERIALIZED VIEW CONCURRENTLY agent_selection_stats",
                ];
                for sql in views {
                    if let Err(e) = sqlx::query(sql).execute(&db).await {
                        tracing::warn!(view = sql, error = %e, "materialized view refresh failed (non-fatal)");
                    }
                }
                tracing::debug!("materialized views refreshed");
            }
        });
    }

    /// Build the full environment for an agent container: platform-level vars + agent-specific secrets.
    pub async fn agent_env(&self, agent_id: uuid::Uuid) -> std::collections::HashMap<String, String> {
        let mut env = crate::catalog::agent_secrets::resolve_agent_env(&self.db, agent_id).await;
        if let Some(ref key) = self.config.openai_api_key {
            env.entry("OPENAI_API_KEY".into()).or_insert_with(|| key.clone());
        }
        if let Some(ref url) = self.config.openai_base_url {
            env.entry("OPENAI_BASE_URL".into()).or_insert_with(|| url.clone());
        }
        env.entry("OPENAI_MODEL".into())
            .or_insert_with(|| self.config.openai_model.clone());
        env.entry("PORT".into()).or_insert_with(|| "8000".into());
        env
    }
}
