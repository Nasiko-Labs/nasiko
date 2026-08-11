use std::sync::Arc;

use nasiko_auth::AuthService;
use nasiko_github::{GitHubConfig, GitHubService};
use nasiko_observability::ObservabilityProvider;
use nasiko_orchestrator::RoutingEngine;
use nasiko_runtime::ContainerRuntime;
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::telemetry::GenAiMetrics;
use crate::usage::UsageTracker;
use nasiko_config::Config;
use nasiko_flow::{FlowConfig, FlowEventBus, FlowGuard};

/// (config fingerprint, client) pair for the DB-configured OIDC client — see
/// `AppState::resolve_oidc_client`.
type OidcClientCache = Arc<tokio::sync::RwLock<Option<(String, Arc<nasiko_oidc::OidcClient>)>>>;

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<dyn ContainerRuntime>,
    pub db: PgPool,
    pub redis: redis::Client,
    pub oci_storage: nasiko_oci::storage::S3Storage,
    pub usage_tracker: UsageTracker,
    pub http_client: reqwest::Client,
    pub auth: Arc<dyn AuthService>,
    pub mcp: nasiko_mcp_gateway::McpState,
    pub flow_guard: FlowGuard,
    pub flow_events: FlowEventBus,
    pub genai_metrics: GenAiMetrics,
    pub config: Arc<Config>,
    pub routing_engine: Arc<dyn RoutingEngine>,
    /// Tempo+Loki observability provider with DB-backed model pricing.
    /// Always constructed — TEMPO_URL/LOKI_URL default to the in-cluster
    /// addresses; queries fail soft when the stack is absent.
    pub observability: Arc<dyn ObservabilityProvider>,
    /// Point-in-time CPU/memory/disk usage for the control plane, the agents and
    /// the supporting infra. Docker-backed in the Compose topology; the EE
    /// composition root replaces it for Kubernetes, the same way it replaces
    /// `routing_engine`.
    pub resource_stats: Arc<dyn nasiko_runtime::ResourceStatsProvider>,
    /// Shared GitHubService instance — None if GitHub OAuth is not configured.
    pub github_svc: Option<Arc<GitHubService>>,
    /// Env-configured OIDC relying-party client (e.g. Microsoft Entra ID) —
    /// None until `OIDC_ISSUER_URL`/`OIDC_CLIENT_ID`/`OIDC_CLIENT_SECRET`/
    /// `OIDC_REDIRECT_URI` are all set. This is the fallback; prefer
    /// `resolve_oidc_client()`, which lets DB-stored settings (configurable
    /// by an admin via `PUT /api/settings`, see `oss/server/src/settings.rs`)
    /// take precedence. See the enterprise OIDC SSO guide.
    pub oidc_svc: Option<Arc<nasiko_oidc::OidcClient>>,
    /// Cache for the DB-configured OIDC client: (config fingerprint, client).
    /// Rebuilt only when the stored config actually changes, so a config
    /// change takes effect on the next login without forcing a fresh
    /// discovery/JWKS fetch on every single request. See `resolve_oidc_client`.
    oidc_dynamic_cache: OidcClientCache,
    /// Wakes the build worker immediately when a new job is enqueued.
    pub build_tx: mpsc::Sender<()>,
    /// UI mounts for the page gate (`auth::require_page_auth`) — each frontend
    /// prefix with its own login page. OSS serves the root mount only; the EE
    /// composition root adds the Flutter app mount at `/app/`.
    pub ui_mounts: &'static [crate::auth::UiMount],
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
        let redis = redis::Client::open(config.redis_url.as_str()).expect("invalid redis url");

        let oci_storage =
            nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await;
        oci_storage.ensure_bucket(false).await.ok();

        let usage_tracker = UsageTracker::new(db.clone());

        let resource_stats = crate::observability::resources::build_provider(&config, db.clone());

        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(20)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build http client");

        let routing_engine: Arc<dyn RoutingEngine> = Arc::new(
            nasiko_orchestrator::OssRoutingEngine::from_config(&config, http_client.clone()),
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
        // session_traces resolver maps session ↔ trace both ways for agents
        // that never set session.id on their spans.
        let observability: Arc<dyn ObservabilityProvider> = {
            use crate::observability::session_resolver::PgSessionIdResolver;
            use nasiko_observability::{DbPricing, TempoLokiProvider};
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
                .with_session_resolver(Arc::new(PgSessionIdResolver::new(db.clone()))),
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
                    central_callback_url: config.github_central_callback_url.clone(),
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
                Arc::new(nasiko_oidc::OidcClient::new(
                    oidc_config,
                    http_client.clone(),
                ))
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

        // MCP gateway state: reuses the same pool, redis client, and pooled
        // HTTP client — no duplicated infrastructure.
        let mut mcp = nasiko_mcp_gateway::McpState::new(
            db.clone(),
            redis.clone(),
            http_client.clone(),
            &config,
        );
        // Swap in the real, ContainerRuntime-backed endpoint refresher (Step
        // 13) — the gateway crate's own default is a no-op, since it has no
        // ContainerRuntime dependency by design.
        mcp.endpoint_refresher = Arc::new(crate::mcp::build::RuntimeEndpointRefresher::new(
            runtime.clone(),
            db.clone(),
        ));

        let state = Self {
            runtime,
            db,
            redis,
            oci_storage,
            usage_tracker,
            resource_stats,
            http_client,
            auth,
            mcp,
            flow_guard,
            flow_events,
            genai_metrics,
            config: Arc::new(config),
            routing_engine,
            observability,
            github_svc,
            oidc_svc,
            oidc_dynamic_cache: Arc::new(tokio::sync::RwLock::new(None)),
            build_tx,
            ui_mounts: &[crate::auth::UiMount::ROOT],
        };

        // Spawn the durable build worker. It owns the receiver and exits when sender drops.
        let worker_state = state.clone();
        tokio::spawn(crate::agents::build_worker::run(worker_state, build_rx));

        // Container-hours meter: records per-instance run sessions for billing
        // (see agents/hours_meter.rs). 0 disables — used by tests that drive
        // reconcile_once directly.
        if state.config.container_hours_poll_secs > 0 {
            tokio::spawn(crate::agents::hours_meter::run(
                state.db.clone(),
                state.runtime.clone(),
                state.config.agent_runtime.clone(),
                std::time::Duration::from_secs(state.config.container_hours_poll_secs),
            ));
        }

        state
    }

    /// Run one-time initialization: bootstrap admin user, spawn seed agents in background,
    /// and start periodic materialized view refresh.
    pub async fn init(&self) {
        if let (Ok(admin_user), Ok(admin_pass)) = (
            std::env::var("ADMIN_USERNAME"),
            std::env::var("ADMIN_PASSWORD"),
        ) && let Err(e) = self.auth.bootstrap_admin(&admin_user, &admin_pass).await
        {
            tracing::warn!(%e, "admin bootstrap failed (may already exist)");
        }

        let state = self.clone();
        tokio::spawn(async move {
            crate::seed::seed_agents_if_configured(&state).await;
            crate::seed::seed_toolkits_if_configured(&state).await;
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

    /// Resolves the OIDC client — and the `user_identities.provider` label
    /// to file new logins under — to actually use for a login/callback: a
    /// DB-stored config (set via `PUT /api/settings`, see
    /// `oss/server/src/settings.rs`) takes precedence over the env-configured
    /// `oidc_svc`/`config.oidc_provider_label`, so an admin can configure or
    /// rotate SSO without a redeploy. Falls back to the env config when no
    /// DB config is present.
    ///
    /// The built `OidcClient` is cached (see `oidc_dynamic_cache`) keyed by a
    /// fingerprint of the config in use, so this is cheap on the common path
    /// (one indexed row read + a string compare) and only pays for a fresh
    /// `OidcClient` (and thus a fresh discovery/JWKS fetch on first use)
    /// when the stored config has actually changed since last checked.
    pub async fn resolve_oidc_client(&self) -> Option<(Arc<nasiko_oidc::OidcClient>, String)> {
        match self.fetch_db_oidc_config().await {
            Some((config, label)) => Some((self.cached_or_build_oidc_client(config).await, label)),
            None => self
                .oidc_svc
                .clone()
                .map(|svc| (svc, self.config.oidc_provider_label.clone())),
        }
    }

    /// Same resolution order as [`resolve_oidc_client`](Self::resolve_oidc_client)
    /// (DB `settings` row, falling back to env config) but returns the raw
    /// `OidcConfig` fields instead of a built `OidcClient` — for callers that
    /// need `client_id`/`client_secret`/`issuer_url` directly for a different
    /// OAuth2 flow (e.g. EE's Azure AD directory sync uses Graph API's
    /// client-credentials flow, not the login authorization-code flow
    /// `OidcClient` is built for). Critically, the returned label is the same
    /// one `resolve_oidc_client`'s caller writes to `user_identities.provider`
    /// at login — any caller minting `user_identities` rows ahead of time
    /// (like directory sync) must reuse this exact label or a later real
    /// login's `(provider, provider_id)` lookup will never match.
    pub async fn resolve_raw_oidc_config(&self) -> Option<(nasiko_oidc::OidcConfig, String)> {
        if let Some(db_config) = self.fetch_db_oidc_config().await {
            return Some(db_config);
        }
        let config = nasiko_oidc::OidcConfig {
            issuer_url: self.config.oidc_issuer_url.clone()?,
            client_id: self.config.oidc_client_id.clone()?,
            client_secret: self.config.oidc_client_secret.clone()?,
            redirect_uri: self.config.oidc_redirect_uri.clone()?,
            scopes: self.config.oidc_scopes.clone(),
        };
        Some((config, self.config.oidc_provider_label.clone()))
    }

    async fn fetch_db_oidc_config(&self) -> Option<(nasiko_oidc::OidcConfig, String)> {
        #[derive(sqlx::FromRow)]
        struct Row {
            oidc_issuer_url: Option<String>,
            oidc_client_id: Option<String>,
            oidc_client_secret_encrypted: Option<String>,
            oidc_redirect_uri: Option<String>,
            oidc_scopes: Option<String>,
            oidc_provider_label: Option<String>,
        }

        let row: Row = sqlx::query_as(
            r#"SELECT oidc_issuer_url, oidc_client_id, oidc_client_secret_encrypted,
                      oidc_redirect_uri, oidc_scopes, oidc_provider_label
               FROM settings LIMIT 1"#,
        )
        .fetch_optional(&self.db)
        .await
        .ok()??;

        let secret = nasiko_secrets::SecretsCrypto::for_platform_settings()
            .decrypt(row.oidc_client_secret_encrypted.as_deref()?)
            .ok()?;

        let config = nasiko_oidc::OidcConfig {
            issuer_url: row.oidc_issuer_url?,
            client_id: row.oidc_client_id?,
            client_secret: secret,
            redirect_uri: row.oidc_redirect_uri?,
            scopes: row
                .oidc_scopes
                .unwrap_or_else(|| "openid profile email".to_string()),
        };
        let label = row
            .oidc_provider_label
            .unwrap_or_else(|| "microsoft_entra".to_string());
        Some((config, label))
    }

    async fn cached_or_build_oidc_client(
        &self,
        config: nasiko_oidc::OidcConfig,
    ) -> Arc<nasiko_oidc::OidcClient> {
        let fingerprint = format!(
            "{}|{}|{}|{}|{}",
            config.issuer_url,
            config.client_id,
            config.client_secret,
            config.redirect_uri,
            config.scopes
        );
        {
            let cached = self.oidc_dynamic_cache.read().await;
            if let Some((cached_fp, client)) = cached.as_ref()
                && cached_fp == &fingerprint
            {
                return client.clone();
            }
        }
        let client = Arc::new(nasiko_oidc::OidcClient::new(
            config,
            self.http_client.clone(),
        ));
        *self.oidc_dynamic_cache.write().await = Some((fingerprint, client.clone()));
        client
    }

    /// Platform-level fallback env vars applied to every agent deployment
    /// when the agent has no secret of the same name. Also served to the CLI
    /// (`GET /api/agents/dev-env`, deployer+) so `nasiko run` can give local
    /// containers the same defaults a CP deployment would get.
    pub fn platform_fallback_env(&self) -> std::collections::HashMap<String, String> {
        let mut env = std::collections::HashMap::new();
        if let Some(ref key) = self.config.openai_api_key {
            env.insert("OPENAI_API_KEY".into(), key.clone());
        }
        if let Some(ref url) = self.config.openai_base_url {
            env.insert("OPENAI_BASE_URL".into(), url.clone());
        }
        env.insert("OPENAI_MODEL".into(), self.config.openai_model.clone());
        env
    }

    /// Build the full environment for an agent container: platform-level vars + agent-specific secrets.
    pub async fn agent_env(
        &self,
        agent_id: uuid::Uuid,
    ) -> std::collections::HashMap<String, String> {
        let mut env = crate::catalog::agent_secrets::resolve_agent_env(&self.db, agent_id).await;
        for (key, value) in self.platform_fallback_env() {
            env.entry(key).or_insert(value);
        }
        env.entry("PORT".into()).or_insert_with(|| "8000".into());
        env
    }
}
