use std::sync::Arc;

use dashmap::DashMap;
use nasiko_auth::AuthService;
use nasiko_github::{GitHubConfig, GitHubService};
use nasiko_observability::ObservabilityProvider;
use nasiko_orchestrator::{RoutingEngine, TextEmbeddingCache, VectorStore};
use nasiko_runtime::ContainerRuntime;
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::agent_lifecycle::SwappableAgentDeletionHook;
use crate::prompt_context::SwappablePromptContext;
use crate::telemetry::GenAiMetrics;
use crate::usage::UsageTracker;
use nasiko_config::Config;
use nasiko_flow::{FlowConfig, FlowEventBus, FlowGuard};

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<dyn ContainerRuntime>,
    pub db: PgPool,
    pub redis: redis::Client,
    pub oci_storage: Arc<dyn nasiko_runtime::BlobStore>,
    pub usage_tracker: UsageTracker,
    pub http_client: reqwest::Client,
    pub auth: Arc<dyn AuthService>,
    pub mcp: nasiko_mcp_gateway::McpState,
    pub flow_guard: FlowGuard,
    pub flow_events: FlowEventBus,
    pub genai_metrics: GenAiMetrics,
    pub config: Arc<Config>,
    pub routing_engine: Arc<dyn RoutingEngine>,
    /// Where both orchestrators read the operator's policy from — the chat-path
    /// delegation policy and the routing engine's bar and prompt text. OSS wires
    /// `NoOrchestratorPolicy`, which imposes nothing; the EE composition root
    /// replaces it, the same way it replaces `routing_engine`.
    pub orchestrator_policy: Arc<dyn crate::orchestrator_policy::OrchestratorPolicySource>,
    /// Supplemental per-agent prompt context (e.g. admin-authored knowledge) added before an
    /// agent runs. OSS default is a no-op; the EE composition root installs the real
    /// implementation through this cell (`SwappablePromptContext::install`, not a plain
    /// reassignment) so background tasks that already hold an earlier `AppState` clone (e.g. the
    /// HITL resume dispatcher, spawned before the EE composition root runs) see the swap too —
    /// see `prompt_context` module docs for why a plain `Arc<dyn Trait>` field can't do this.
    pub prompt_context: Arc<SwappablePromptContext>,
    /// Fired once, best-effort, after an agent is deleted — a chance for enterprise-only,
    /// agent-keyed state to clean itself up (e.g. freeing a name it reserved for reuse). OSS
    /// default is a no-op; the EE composition root installs the real implementation through this
    /// cell (`SwappableAgentDeletionHook::install`, not a plain reassignment) for the same reason
    /// `prompt_context` does — see that field's doc comment and `agent_lifecycle` module docs.
    pub agent_deletion_hook: Arc<SwappableAgentDeletionHook>,
    /// PACMS candidate/query embedding cache for the history enrichment done
    /// directly in `a2a_dispatch.rs` (shared across requests, like the one
    /// `OssRoutingEngine` holds internally for its own `fetch_pacms` call —
    /// see `TextEmbeddingCache` docs).
    pub history_embedding_cache: TextEmbeddingCache,
    /// Tempo+Loki observability provider with DB-backed model pricing.
    /// Always constructed — TEMPO_URL/LOKI_URL default to the in-cluster
    /// addresses; queries fail soft when the stack is absent.
    pub observability: Arc<dyn ObservabilityProvider>,
    /// The platform's single cost engine. Held here so every surface that needs
    /// to price something receives the same instance — and so the resolved-rate
    /// and cache-ratio caches inside it are shared rather than rebuilt per call.
    pub pricing: Arc<nasiko_pricing::PricingEngine>,
    /// Point-in-time CPU/memory/disk usage for the control plane, the agents and
    /// the supporting infra. Docker-backed in the Compose topology; the EE
    /// composition root replaces it for Kubernetes, the same way it replaces
    /// `routing_engine`.
    pub resource_stats: Arc<dyn nasiko_runtime::ResourceStatsProvider>,
    /// Shared GitHubService instance — None if GitHub OAuth is not configured.
    pub github_svc: Option<Arc<GitHubService>>,
    /// Wakes the build worker immediately when a new job is enqueued.
    pub build_tx: mpsc::Sender<()>,
    /// HITL persistence (`hitl_requests`) — detection, human-facing API, and the resume
    /// dispatcher all go through this. See `oss/hitl`.
    pub hitl_store: Arc<dyn nasiko_hitl::HitlStore>,
    /// Best-effort wake for the HITL resume dispatcher right after a `resolve()` commits — a
    /// latency optimization only; the dispatcher's own poll loop is the actual delivery
    /// guarantee.
    pub hitl_resume_tx: mpsc::Sender<()>,
    /// Replay buffer for a resumed HITL execution's real A2A/SSE events, so a frontend
    /// reconnecting through `POST /api/orchestrator/a2a` (`metadata.reconnect_after_hitl_id`)
    /// sees them without polling `/messages` or the resume dispatcher ever touching the browser
    /// connection. See `oss/server/src/hitl/continuation.rs`.
    pub continuation_events: crate::hitl::continuation::ContinuationRegistry,
    /// UI mounts for the page gate (`auth::require_page_auth`) — each frontend
    /// prefix with its own login page. Both editions serve the root mount only;
    /// the slice exists so another frontend can be mounted under its own prefix.
    pub ui_mounts: &'static [crate::auth::UiMount],
}

impl AppState {
    /// The embedding client the chat handlers hand to
    /// `context_selection::fetch_for_user`. Built per call (it is a thin
    /// handle over the shared `history_embedding_cache`, not a connection),
    /// so the two call sites don't each re-derive the provider settings.
    pub fn history_vector_store(&self) -> VectorStore {
        VectorStore::for_embedding(
            self.config.openai_api_key.clone().unwrap_or_default(),
            self.config
                .openai_base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com".into()),
            self.config.embedding_model.clone(),
            self.history_embedding_cache.clone(),
        )
    }

    pub async fn from_config(
        config: Config,
        auth: Arc<dyn AuthService>,
        runtime: Arc<dyn ContainerRuntime>,
        oci_storage: Arc<dyn nasiko_runtime::BlobStore>,
    ) -> Self {
        let db = PgPool::connect(&config.database_url)
            .await
            .unwrap_or_else(|e| panic!("{}", pg_connect_error_message(&config.database_url, &e)));
        Self::from_config_with_db(config, auth, runtime, oci_storage, db).await
    }

    pub async fn run_migrations(db: &PgPool) {
        ensure_pg_extensions(db).await;
        sqlx::migrate!("../migrations")
            .set_ignore_missing(true)
            .run(db)
            .await
            .expect("database migration failed");
        // Offline pricing baseline: gap-filling upsert, so operator-set prices
        // and pricing-sync history always win. Code (not a migration) so price
        // updates ship with the binary.
        nasiko_observability::pricing::seed_model_pricing(db).await;
    }

    /// `oci_storage` is received, not constructed, for the same reason `auth`
    /// and `runtime` are: which backend is in play is an edition decision. OSS
    /// ships the S3-compatible one; the enterprise composition root selects
    /// among more. Constructing it here would drag every backend — and their
    /// dependencies — into the public edition that cannot configure them.
    pub async fn from_config_with_db(
        config: Config,
        auth: Arc<dyn AuthService>,
        runtime: Arc<dyn ContainerRuntime>,
        oci_storage: Arc<dyn nasiko_runtime::BlobStore>,
        db: PgPool,
    ) -> Self {
        let redis = redis::Client::open(config.redis_url.as_str()).expect("invalid redis url");

        // Fail fast, exactly as the Postgres connect above does. This was
        // `.ok()` — which discarded the error without even logging it, so a
        // control plane whose object store was unreachable, misconfigured, or
        // missing its bucket booted green and reported healthy, then failed
        // every image push and agent deploy afterwards with no startup signal
        // pointing at the cause. An unusable artifact store is not a degraded
        // mode, it is a broken one. The startup-ordering race this used to
        // paper over (the store not ready yet when the control plane boots)
        // is handled the same way it already is for Postgres: the process
        // exits and the orchestrator restarts it.
        if let Err(e) = oci_storage.ensure_bucket(false).await {
            panic!("object storage is not usable: {e}");
        }

        let usage_tracker = UsageTracker::new(db.clone());

        let resource_stats = crate::observability::resources::build_provider(&config, db.clone());

        // Shared client for short, bounded calls: embeddings, registry probes,
        // OAuth, provider requests. Agent hops are *not* short — every call site
        // that forwards to an agent container overrides this per-request with
        // `config.agent_call_timeout_secs` rather than raising the default here
        // and handing every other caller a ten-minute hang.
        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(20)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build http client");

        let routing_engine: Arc<dyn RoutingEngine> = Arc::new(
            nasiko_orchestrator::OssRoutingEngine::from_config(&config, http_client.clone()),
        );
        let orchestrator_policy: Arc<dyn crate::orchestrator_policy::OrchestratorPolicySource> =
            Arc::new(crate::orchestrator_policy::NoOrchestratorPolicy);
        let prompt_context = Arc::new(SwappablePromptContext::new(Arc::new(
            crate::prompt_context::NoopPromptContextProvider,
        )));
        let agent_deletion_hook =
            Arc::new(crate::agent_lifecycle::SwappableAgentDeletionHook::new(
                Arc::new(crate::agent_lifecycle::NoopAgentDeletionHook),
            ));
        let history_embedding_cache: TextEmbeddingCache = Arc::new(DashMap::new());

        let flow_config = FlowConfig {
            max_depth: config.flow_max_depth as u32,
            max_fan_out: config.flow_max_fan_out as u32,
            max_flow_tokens: config.flow_max_tokens as u64,
            flow_timeout_secs: config.flow_timeout_secs as u64,
            // Derived, never a literal: the guard reads `started_at` out of the
            // flow's Redis key, so a TTL shorter than the timeout would expire
            // the state the timeout check depends on — the check would pass
            // silently and the depth/fan-out counters would reset mid-flow.
            flow_state_ttl_secs: nasiko_flow::state_ttl_for(config.flow_timeout_secs as u64),
        };
        let flow_guard = FlowGuard::new(redis.clone(), flow_config);
        let flow_events = FlowEventBus::new();
        let genai_metrics = GenAiMetrics::new();

        // Tempo+Loki observability backend. Model pricing resolves through
        // the model_pricing DB table with the static table as fallback; the
        // session_traces resolver maps session ↔ trace both ways for agents
        // that never set session.id on their spans.
        let pricing = Arc::new(nasiko_pricing::PricingEngine::new(db.clone()));

        let observability: Arc<dyn ObservabilityProvider> = {
            use crate::observability::session_resolver::PgSessionIdResolver;
            use nasiko_observability::TempoLokiProvider;
            tracing::info!(
                tempo_url = %config.tempo_url,
                loki_url = %config.loki_url,
                "observability backend configured"
            );
            Arc::new(
                TempoLokiProvider::new(
                    config.tempo_url.clone(),
                    config.loki_url.clone(),
                    pricing.clone(),
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

        let (build_tx, build_rx) = mpsc::channel(64);
        let (hitl_resume_tx, hitl_resume_rx) = mpsc::channel(64);
        let continuation_events = crate::hitl::continuation::ContinuationRegistry::new();
        let hitl_store: Arc<dyn nasiko_hitl::HitlStore> = Arc::new(
            nasiko_hitl::PgHitlStore::with_ttl_days(db.clone(), config.hitl_request_ttl_days),
        );

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
        // Build the initial search index from whatever tools are already in the DB.
        // The seed task (spawned later) will rebuild again after syncing new tools.
        if let Err(e) = mcp.search_index.rebuild(&db).await {
            tracing::warn!(%e, "initial search index build failed — search will return empty until next sync");
        }

        let state = Self {
            pricing,
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
            orchestrator_policy,
            prompt_context,
            agent_deletion_hook,
            history_embedding_cache,
            observability,
            github_svc,
            build_tx,
            hitl_store,
            hitl_resume_tx,
            continuation_events,
            ui_mounts: &[crate::auth::UiMount::ROOT],
        };

        // Spawn the durable build worker. It owns the receiver and exits when sender drops.
        let worker_state = state.clone();
        tokio::spawn(crate::agents::build_worker::run(worker_state, build_rx));

        // Spawn the HITL resume dispatcher — same shape as the build worker above.
        // Handles `direct_chat`/`agent_proxy`/`maf`/`orchestrator`-origin rows (real A2A task
        // resume, or an XADD continuation job for MAF); its own `claim_for_resume` is scoped to
        // just those four origins. `mcp_tool` rows have their own separate dispatcher
        // (`nasiko_hitl::dispatcher`, wired up elsewhere in this function).
        let hitl_state = state.clone();
        tokio::spawn(crate::hitl::run(hitl_state, hitl_resume_rx));

        // Periodic eviction for finished continuation buffers — same shape as every other
        // periodic sweep in this file.
        tokio::spawn(crate::hitl::continuation::sweep_loop(
            state.continuation_events.clone(),
        ));

        // The `mcp_tool`-origin resume dispatcher (AuthRequired's auto-resume nudge, and any
        // ToolApproval push) — lives inside `nasiko-hitl` since it isn't A2A-task-shaped (an MCP
        // tool call has no `task_id` to resume; `RuntimeResumeNotifier` sends a standalone nudge
        // message instead). Its own `claim_for_resume` is scoped to `origin = 'mcp_tool'`, so it
        // can run alongside the dispatcher above without racing it for the same rows.
        let resume_notifier: Arc<dyn nasiko_hitl::ResumeNotifier> =
            Arc::new(nasiko_hitl::RuntimeResumeNotifier::new(
                state.db.clone(),
                state.runtime.clone(),
                state.http_client.clone(),
                // Same bound `gateway.rs`'s `flow_user` enforces on the agent's retry, so the
                // nudge never registers a window the gateway will reject.
                i64::from(state.config.flow_timeout_secs),
            ));
        tokio::spawn(nasiko_hitl::dispatcher::run(
            state.db.clone(),
            resume_notifier,
            nasiko_hitl::DispatcherConfig {
                poll_interval: std::time::Duration::from_secs(
                    state.config.hitl_resume_poll_interval_secs,
                ),
                recovery_interval: std::time::Duration::from_secs(
                    state.config.hitl_resume_recovery_interval_secs,
                ),
                lease_minutes: state.config.hitl_resume_lease_minutes,
                max_attempts: state.config.hitl_resume_max_attempts,
                retry_delay: std::time::Duration::from_secs(
                    state.config.hitl_resume_retry_delay_secs,
                ),
            },
        ));

        if let Some(endpoint) = state.config.coding_agent_otlp_endpoint.clone() {
            tokio::spawn(crate::coding_agent_otlp::run(
                state.db.clone(),
                state.http_client.clone(),
                endpoint,
            ));
        }

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

        // Trace-usage materializer: reads recent traces from Tempo, extracts
        // FinOps metrics, and upserts into trace_usage so dashboard queries
        // hit Postgres instead of Tempo. 0 disables.
        if state.config.observability_enabled && state.config.trace_usage_sync_secs > 0 {
            let session_resolver: std::sync::Arc<dyn nasiko_observability::SessionIdResolver> =
                std::sync::Arc::new(
                    crate::observability::session_resolver::PgSessionIdResolver::new(
                        state.db.clone(),
                    ),
                );
            tokio::spawn(crate::observability::trace_materializer::run(
                state.db.clone(),
                state.observability.clone(),
                session_resolver,
                std::time::Duration::from_secs(state.config.trace_usage_sync_secs),
                std::time::Duration::from_secs(state.config.trace_usage_overlap_secs),
                state.config.trace_usage_batch_size,
            ));
        }

        // Retires the seeded brevity factor once the holdout has enough samples. Cheap (one
        // grouped scan) and idempotent, so it rides the same process as the other workers rather
        // than needing a scheduler.
        tokio::spawn(crate::observability::savings_factors::run(
            state.db.clone(),
            std::time::Duration::from_secs(state.config.savings_factor_refresh_secs),
            state.config.savings_factor_min_samples,
            state.config.savings_factor_window_days,
        ));

        state
    }

    /// Run one-time initialization: bootstrap admin user, spawn seed agents in background,
    /// reconcile any `running` agent with no live runtime resource, and start periodic
    /// materialized view refresh.
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

        // Covers e.g. a tenant cluster restore, which recreates the database
        // but not the individual agent Deployments/Services — see
        // `agents::reconcile`'s module doc.
        let state = self.clone();
        tokio::spawn(async move {
            crate::agents::reconcile::reconcile_agents_on_startup(&state).await;
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

    /// Build the full environment for an agent container: platform-level vars + agent-specific secrets
    /// + feature flags from metadata.
    pub async fn agent_env(
        &self,
        agent_id: uuid::Uuid,
    ) -> std::collections::HashMap<String, String> {
        let mut env = crate::catalog::agent_secrets::resolve_agent_env(&self.db, agent_id).await;
        for (key, value) in self.platform_fallback_env() {
            env.entry(key).or_insert(value);
        }
        env.entry("PORT".into()).or_insert_with(|| "8000".into());

        // Inject feature flags from agents.metadata.features as `NASIKO_<KEY>` env vars.
        // `metadata` is owner-writable through `PUT /api/agents/{id}`, so keys are filtered
        // to identifier characters: anything else cannot form a valid env var name. Flags use
        // `or_insert`, so an agent secret of the same name still wins.
        if let Ok(metadata) = sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT metadata FROM agents WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(agent_id)
        .fetch_one(&self.db)
        .await
            && let Some(features) = metadata.get("features").and_then(|f| f.as_object())
        {
            for (key, value) in features {
                let Some(val) = value.as_str() else { continue };
                if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    tracing::warn!(%agent_id, %key, "agent_env: skipping feature flag with non-identifier key");
                    continue;
                }
                env.entry(format!("NASIKO_{}", key.to_uppercase()))
                    .or_insert_with(|| val.to_string());
            }
        }

        // A plain `agents` column, not a secret (see migration 0032) — the
        // control plane also reads it at A2A dispatch time to inject the
        // minimal-code ladder into the outgoing message (a2a_dispatch.rs), but
        // the agent still needs its own copy at boot purely to gate
        // self-review (wants_self_review requires minimal_code as one of its
        // three conditions — see nasiko-coding-policy). That gate still needs
        // a restart to pick up a change; the ladder injection itself does not.
        let minimal_code_enabled: Option<bool> =
            sqlx::query_scalar("SELECT minimal_code_enabled FROM agents WHERE id = $1")
                .bind(agent_id)
                .fetch_optional(&self.db)
                .await
                .ok()
                .flatten();
        // Always set explicitly (not just when true) — a pre-migration agent
        // may still carry a stale CODING_AGENT_MINIMAL_CODE secret from
        // before this column existed, and that must not leak through once
        // the column says otherwise.
        let minimal_code_enabled = minimal_code_enabled.unwrap_or(false);
        tracing::info!(
            %agent_id,
            minimal_code_enabled,
            "agent_env: injecting CODING_AGENT_MINIMAL_CODE"
        );
        env.insert(
            "CODING_AGENT_MINIMAL_CODE".into(),
            minimal_code_enabled.to_string(),
        );
        env
    }
}

/// Postgres extensions the migrations require (`0001_schema.sql` runs
/// `CREATE EXTENSION IF NOT EXISTS` for each). Invisible on the in-cluster
/// `pgvector/pgvector` image, which ships all three preinstalled.
const REQUIRED_PG_EXTENSIONS: [&str; 3] = ["pgcrypto", "pg_trgm", "vector"];

/// Creates the required extensions before the migration runner touches them,
/// so a managed Postgres that hasn't installed or allowlisted one (Azure
/// Flexible Server, RDS, Cloud SQL all gate `CREATE EXTENSION`) fails fast
/// with an actionable message instead of a raw mid-migration SQL error.
async fn ensure_pg_extensions(db: &PgPool) {
    for ext in REQUIRED_PG_EXTENSIONS {
        if let Err(err) = sqlx::query(&format!("CREATE EXTENSION IF NOT EXISTS \"{ext}\""))
            .execute(db)
            .await
        {
            panic!("{}", pg_extension_error_message(ext, &err.to_string()));
        }
    }
}

/// Explains a startup connect failure by naming the address it failed against.
///
/// sqlx reports a filtered or blackholed host as a bare `PoolTimedOut` after the
/// acquire timeout elapses, with nothing logged in the meantime — so the most
/// likely managed-Postgres misconfiguration (a firewall rule or egress
/// NetworkPolicy that never admits the control plane) reads as "the platform
/// hung" rather than "nothing answered at this address". The credentials the DSN
/// also carries are never included: the options are parsed rather than the
/// string printed, so there is no path by which the password reaches a log.
pub fn pg_connect_error_message(database_url: &str, err: &sqlx::Error) -> String {
    use std::str::FromStr;
    let target = sqlx::postgres::PgConnectOptions::from_str(database_url)
        .map(|o| {
            format!(
                "{}:{}/{}",
                o.get_host(),
                o.get_port(),
                o.get_database().unwrap_or("<no database>")
            )
        })
        .unwrap_or_else(|e| format!("<unparseable DATABASE_URL: {e}>"));
    format!(
        "failed to connect to Postgres at {target}: {err}\n\
         A timeout here means nothing answered, not that the credentials are \
         wrong — check that the host and port are reachable from the control \
         plane (managed Postgres: firewall rule, private endpoint, or the \
         egress NetworkPolicy derived from `postgres.external.egress_cidr`), \
         and that `sslmode` matches what the server requires."
    )
}

fn pg_extension_error_message(ext: &str, err: &str) -> String {
    format!(
        "required Postgres extension \"{ext}\" is unavailable: {err}\n\
         The migrations need pgcrypto, pg_trgm, and vector. On a managed \
         Postgres, install/allowlist them on the server first — e.g. Azure \
         Flexible Server: `az postgres flexible-server parameter set \
         --name azure.extensions --value VECTOR,PG_TRGM,PGCRYPTO` — then \
         restart the control plane."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_error_names_the_address_but_never_the_password() {
        let err = sqlx::Error::PoolTimedOut;
        let msg = pg_connect_error_message(
            "postgres://nasiko_admin:sup3rs3cret@pg.internal:5432/nasiko_prod?sslmode=require",
            &err,
        );
        assert!(msg.contains("pg.internal:5432/nasiko_prod"));
        assert!(msg.contains("egress_cidr"));
        assert!(!msg.contains("sup3rs3cret"));
    }

    #[test]
    fn extension_error_names_the_extension_and_the_remedy() {
        let msg = pg_extension_error_message("vector", "permission denied");
        assert!(msg.contains("\"vector\""));
        assert!(msg.contains("permission denied"));
        assert!(msg.contains("azure.extensions"));
    }
}
