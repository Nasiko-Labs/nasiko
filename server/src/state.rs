use std::sync::Arc;

use dashmap::DashMap;
use nasiko_auth::AuthService;
use nasiko_github::{GitHubConfig, GitHubService};
use nasiko_observability::ObservabilityProvider;
use nasiko_orchestrator::{RoutingEngine, TextEmbeddingCache, VectorStore};
use nasiko_runtime::{ContainerRuntime, SandboxProvider};
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::agent_lifecycle::SwappableAgentDeletionHook;
use crate::agent_scaling::{AgentScaling, SingleReplica};
use crate::chat::attachment_store::{ChatUploadStore, SwappableAttachmentStore};
use crate::chat::turn_artifacts::{NoTurnArtifacts, SwappableTurnArtifactSource};
use crate::prompt_context::SwappablePromptContext;
use crate::telemetry::GenAiMetrics;
use crate::usage::UsageTracker;
use nasiko_config::Config;
use nasiko_flow::{FlowConfig, FlowEventBus, FlowGuard};
use nasiko_leader::LockKey;

/// Boot seeding (`SEED_AGENTS`, workflows, harness agents, toolkits), one replica at a time.
const BOOT_SEED_LOCK: LockKey = LockKey::named("server.boot_seed");
/// The startup agent reconcile, one replica at a time.
const STARTUP_RECONCILE_LOCK: LockKey = LockKey::named("server.startup_reconcile");
/// The hourly materialized-view refresh, on one replica per period.
const MATVIEW_REFRESH_LOCK: LockKey = LockKey::named("server.matview_refresh");
/// Held while a replica migrates (both editions' migrations, and the extensions before them).
///
/// Taken by polling, never by a blocking `pg_advisory_lock`: a session waiting inside a statement
/// holds a snapshot, and a `CREATE INDEX CONCURRENTLY` in the holder's migration waits for every
/// older snapshot to end, so a blocked waiter and the holder would wait on each other. sqlx's own
/// migration lock is that blocking kind; under this one no other replica ever waits on it.
pub const MIGRATIONS_LOCK: LockKey = LockKey::named("server.migrations");
pub const MIGRATIONS_LOCK_POLL: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<dyn ContainerRuntime>,
    /// Session-shaped sandboxes (harness sessions), beside the deployment-shaped `runtime`.
    pub sandbox: Arc<dyn SandboxProvider>,
    pub db: PgPool,
    pub redis: nasiko_redis::SharedRedis,
    /// Tells other replicas what this one changed, and runs this one's handlers for theirs
    /// (`invalidation`).
    pub invalidations: crate::invalidation::Invalidations,
    pub oci_storage: Arc<dyn nasiko_runtime::BlobStore>,
    pub usage_tracker: UsageTracker,
    pub http_client: reqwest::Client,
    /// The client every server-to-agent call goes through (see `agent_client`): no idle
    /// connections, so requests spread across an agent's replicas.
    pub agent_client: reqwest::Client,
    pub auth: Arc<dyn AuthService>,
    pub mcp: nasiko_mcp_gateway::McpState,
    pub flow_guard: FlowGuard,
    pub flow_events: FlowEventBus,
    pub genai_metrics: GenAiMetrics,
    /// Rate limit, concurrency cap and counters of the coding-agent ingest route.
    pub coding_agent_ingest: crate::coding_agent_ingest_guard::IngestGuard,
    /// How many dashboard reads this server serves at once (`dashboard_limit`).
    pub dashboard_limit: crate::dashboard_limit::DashboardLimit,
    pub config: Arc<Config>,
    pub routing_engine: Arc<dyn RoutingEngine>,
    /// In-process LLM router for the server's own platform-key calls (titles, planner,
    /// capabilities, proactive agent embeddings). The router writes their usage.
    pub llm_backend: nasiko_orchestrator::llm_backend::SharedLlmBackend,
    /// Where both orchestrators read the operator's policy from — the chat-path
    /// delegation policy and the routing engine's bar and prompt text. OSS wires
    /// `NoOrchestratorPolicy`, which imposes nothing; the EE composition root
    /// replaces it, the same way it replaces `routing_engine`.
    pub orchestrator_policy: Arc<dyn crate::orchestrator_policy::OrchestratorPolicySource>,
    /// Asked before each of the chat orchestrator's own model calls. OSS wires
    /// `NoSpendGate`, which never refuses; the EE composition root replaces it
    /// with its budget check.
    pub spend_gate: Arc<dyn crate::spend_gate::SpendGate>,
    /// Who pays for a model call another user makes through an agent. OSS wires `CallerPays`; the
    /// EE composition root may replace it.
    pub shared_call_payer: Arc<dyn nasiko_llm_router::payer::SharedCallPayer>,
    /// Asked before a harness session is opened and before each of its turns starts. OSS wires
    /// `AdmitEverySession`; an edition whose sessions can add an agent installs its limit check
    /// through this cell (`SwappableSessionAdmission::install`), once it has what that needs.
    pub session_admission: Arc<crate::sessions::admission::SwappableSessionAdmission>,
    /// Asked inside the transaction that inserts a new agent row. OSS wires `AdmitEveryAgent`; an
    /// edition with an agent limit replaces it at its composition root, before the routers take
    /// their `AppState` clones.
    pub agent_admission: Arc<dyn crate::agents::admission::AgentAdmission>,
    /// Where a downstream reader of hot `trace_usage` rows has read up to, so
    /// tiering never moves rows it has not seen. OSS wires `NoTieringHold` (no such
    /// reader); the EE composition root replaces it before `init`.
    pub tiering_hold: Arc<dyn nasiko_event_store::TieringHold>,
    /// Called just before resolving and forwarding a request to an agent. The default
    /// (`ParkedAgentWake`) scales a parked agent's workload back up; an edition that prepares
    /// agents differently installs its own through this cell (`SwappableAgentWake::install`, not
    /// a plain reassignment) for the same reason `prompt_context` does - the HITL resume
    /// dispatcher forwards to agents too, and holds an `AppState` clone taken before that root
    /// runs. See that field's doc comment and the `agent_wake` module docs.
    pub agent_wake: Arc<crate::agent_wake::SwappableAgentWake>,
    /// What the MAF worker runs each queued workflow through. OSS wires
    /// `LinearMafExecutor` (steps in array order); the EE composition root
    /// replaces it before the app is built, the same way it replaces
    /// `routing_engine`, so the worker started by the app sees the swap.
    pub maf_executor: Arc<dyn nasiko_orchestrator::maf::MafExecutor>,
    /// What the MAF workflow routes consult beyond their own checks when a
    /// workflow is saved or run. OSS wires `NoMafWorkflowPolicy`, which changes
    /// nothing; the EE composition root replaces it.
    pub maf_policy: Arc<dyn crate::maf_policy::MafWorkflowPolicy>,
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
    /// Decides the replica bounds of every agent deploy and whether an autoscaler owns the
    /// count. A constructor argument (`from_config_with_scaling`), not a swappable cell: the
    /// build worker and HITL dispatcher copy `AppState` while it is built. OSS wires
    /// `SingleReplica`.
    pub agent_scaling: Arc<dyn AgentScaling>,
    /// Where a completed chat turn's files may also come from — files a turn
    /// produced outside this crate's own chat store. OSS default is a no-op
    /// ([`NoTurnArtifacts`]): no chips are added and no download scheme beyond
    /// the ones the chat routes own is claimed. The EE composition root installs
    /// the real implementation through this cell
    /// (`SwappableTurnArtifactSource::install`, not a plain reassignment) for the
    /// same reason `prompt_context` does — see that field's doc comment and the
    /// `chat::turn_artifacts` module docs.
    pub turn_artifacts: Arc<SwappableTurnArtifactSource>,
    pub attachment_store: Arc<SwappableAttachmentStore>,
    pub marker_signer: Option<Arc<nasiko_llm_router::attachments::MarkerSigner>>,
    pub llm_config_cache: Arc<nasiko_llm_router::ConfigCache>,
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
    /// Set when this replica is asked to stop: `/ready` answers 503 and workers stop claiming
    /// work. See `shutdown.rs`.
    pub shutdown: crate::shutdown::ShutdownSignal,
}

impl AppState {
    /// After any `llm_configs` write: drop every cached resolution here and on every replica.
    pub fn llm_configs_changed(&self) {
        self.llm_config_cache.clear();
        self.invalidations
            .publish(crate::invalidation::topic::LLM_CONFIG);
    }

    /// After an agent's own LLM config binding changes: drop its cached resolution here and on
    /// every replica.
    pub fn agent_llm_config_changed(&self, agent_id: uuid::Uuid) {
        self.llm_config_cache.invalidate(agent_id);
        self.invalidations
            .publish_key(crate::invalidation::topic::LLM_CONFIG, agent_id.to_string());
    }

    /// A build job was queued: wake the build worker here and on every replica. Latency only;
    /// each worker's poll is the delivery guarantee.
    pub async fn wake_build_worker(&self) {
        let _ = self.build_tx.send(()).await;
        self.invalidations
            .publish(crate::invalidation::topic::BUILD_QUEUE);
    }

    /// A HITL request was resolved: wake the resume dispatcher here and on every replica.
    /// Latency only; each dispatcher's poll is the delivery guarantee.
    pub fn wake_hitl_resume(&self) {
        let _ = self.hitl_resume_tx.try_send(());
        self.invalidations
            .publish(crate::invalidation::topic::HITL_RESUME);
    }

    /// What this replica does when another one publishes an OSS invalidation (the tool-search
    /// handler is installed with its index, `mcp::search_sync`).
    fn register_invalidation_handlers(&self) {
        use crate::invalidation::{Invalidated, topic};
        let cache = self.llm_config_cache.clone();
        self.invalidations
            .register(topic::LLM_CONFIG, move |what| match what {
                Invalidated::Key(key) => match uuid::Uuid::parse_str(key) {
                    Ok(agent_id) => cache.invalidate(agent_id),
                    Err(_) => cache.clear(),
                },
                Invalidated::All => cache.clear(),
            });
        let build_tx = self.build_tx.clone();
        self.invalidations.register(topic::BUILD_QUEUE, move |_| {
            let _ = build_tx.try_send(());
        });
        let hitl_resume_tx = self.hitl_resume_tx.clone();
        self.invalidations.register(topic::HITL_RESUME, move |_| {
            let _ = hitl_resume_tx.try_send(());
        });
    }

    /// Whether agent messages carry the current date and time (`AGENT_CONTEXT_CURRENT_TIME`).
    pub fn agent_clock(&self) -> nasiko_types::agent_clock::AgentClock {
        nasiko_types::agent_clock::AgentClock::new(self.config.agent_current_time)
    }

    /// Handle over hot Postgres rows and the cold Lance tier (docs/LAKEHOUSE_TRD.md),
    /// tiering behind this state's `tiering_hold`. Cheap: a pool clone plus config;
    /// readers merging both tiers build one per call.
    pub fn event_store(&self) -> nasiko_event_store::EventStore {
        event_store(&self.config, &self.db).with_hold(self.tiering_hold.clone())
    }

    /// The embedding client the chat handlers hand to
    /// `context_selection::fetch_for_user`. Built per call (it is a thin
    /// handle over the shared `history_embedding_cache`, not a connection),
    /// so the call sites don't each re-derive the provider settings. Embeds with the key `user_id`
    /// saved from the UI when there is one, else the server's.
    pub async fn history_vector_store(&self, user_id: uuid::Uuid) -> VectorStore {
        let Some(saved_key) =
            nasiko_orchestrator::user_key::saved_openai_key(&self.db, user_id).await
        else {
            // No saved user key: platform embeddings go through the in-process router.
            return VectorStore::for_embedding_routed(
                nasiko_orchestrator::vector_store::RoutedEmbedder {
                    backend: Arc::clone(&self.llm_backend),
                    call: nasiko_orchestrator::llm_backend::LlmCallContext {
                        user_id,
                        component:
                            nasiko_orchestrator::llm_backend::PlatformComponent::HistoryEmbedding
                                .to_string(),
                        session_id: None,
                    },
                },
                self.config.embedding_model.clone(),
                self.history_embedding_cache.clone(),
            );
        };
        let base_url: String = self
            .config
            .openai_base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com".into());
        let model: String = nasiko_orchestrator::providers::direct_embedding_model(
            &self.config.embedding_model,
            &base_url,
        );
        VectorStore::for_embedding(
            saved_key,
            base_url,
            model,
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

    /// Runs under [`MIGRATIONS_LOCK`], so replicas starting together migrate one at a time.
    pub async fn run_migrations(db: &PgPool) {
        let held = nasiko_leader::lock(db, MIGRATIONS_LOCK, MIGRATIONS_LOCK_POLL)
            .await
            .expect("could not take the migrations lock");
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
        held.release().await;
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
        let agent_scaling = Arc::new(SingleReplica::new(runtime.clone(), db.clone()));
        Self::from_config_with_scaling(config, auth, runtime, oci_storage, db, agent_scaling).await
    }

    /// [`from_config_with_db`](Self::from_config_with_db) with the edition's replica policy
    /// instead of the single-replica default.
    ///
    /// Without a sandbox provider the state gets one that refuses every call, naming the
    /// configured backend; a binary that runs harness sessions uses
    /// [`from_config_with_sandbox`](Self::from_config_with_sandbox).
    pub async fn from_config_with_scaling(
        config: Config,
        auth: Arc<dyn AuthService>,
        runtime: Arc<dyn ContainerRuntime>,
        oci_storage: Arc<dyn nasiko_runtime::BlobStore>,
        db: PgPool,
        agent_scaling: Arc<dyn AgentScaling>,
    ) -> Self {
        let sandbox = crate::sandbox_wiring::build_sandbox_provider(&config, None);
        Self::from_config_with_sandbox(
            config,
            auth,
            runtime,
            oci_storage,
            db,
            agent_scaling,
            sandbox,
        )
        .await
    }

    /// [`from_config_with_scaling`](Self::from_config_with_scaling) with the edition's
    /// sandbox provider. Received, not constructed, like `runtime`: the backend is an
    /// edition decision.
    pub async fn from_config_with_sandbox(
        config: Config,
        auth: Arc<dyn AuthService>,
        runtime: Arc<dyn ContainerRuntime>,
        oci_storage: Arc<dyn nasiko_runtime::BlobStore>,
        db: PgPool,
        agent_scaling: Arc<dyn AgentScaling>,
        sandbox: Arc<dyn SandboxProvider>,
    ) -> Self {
        // One shared connection for every Redis call; see nasiko_redis.
        let redis = nasiko_redis::SharedRedis::new(
            redis::Client::open(config.redis_url.as_str()).expect("invalid redis url"),
        );

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
        // OAuth. Agent hops are *not* short, and go through `agent_client`
        // instead; LLM provider calls go through the router's own client.
        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(20)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("failed to build http client");
        // Agent calls set their own per-request timeout where they need one other
        // than the agent-call budget.
        let agent_client = crate::agent_client::build(std::time::Duration::from_secs(
            config.agent_call_timeout_secs,
        ));

        // Platform-key embeddings and agent selection go through the in-process LLM router,
        // so any configured provider (OpenAI, Vertex, ...) serves them and the router alone
        // writes their usage.
        let routing_backend: nasiko_orchestrator::llm_backend::SharedLlmBackend =
            Arc::new(crate::llm_router::platform_backend::RouterBackend::new(
                nasiko_llm_router::platform::PlatformLlm::new(nasiko_llm_router::LlmRouterCtx {
                    ledger: usage_tracker.ledger(),
                    payer: Arc::new(nasiko_llm_router::payer::CallerPays),
                    ..nasiko_llm_router::LlmRouterCtx::from_shared(db.clone())
                }),
            ));
        let routing_engine: Arc<dyn RoutingEngine> = Arc::new(
            nasiko_orchestrator::OssRoutingEngine::from_config(&config, http_client.clone())
                .with_backend(Arc::clone(&routing_backend)),
        );
        let orchestrator_policy: Arc<dyn crate::orchestrator_policy::OrchestratorPolicySource> =
            Arc::new(crate::orchestrator_policy::NoOrchestratorPolicy);
        let spend_gate: Arc<dyn crate::spend_gate::SpendGate> =
            Arc::new(crate::spend_gate::NoSpendGate);
        let session_admission =
            Arc::new(crate::sessions::admission::SwappableSessionAdmission::new(
                Arc::new(crate::sessions::admission::AdmitEverySession),
            ));
        let tiering_hold: Arc<dyn nasiko_event_store::TieringHold> =
            Arc::new(nasiko_event_store::NoTieringHold);
        let maf_executor: Arc<dyn nasiko_orchestrator::maf::MafExecutor> =
            Arc::new(nasiko_orchestrator::maf::LinearMafExecutor);
        let maf_policy: Arc<dyn crate::maf_policy::MafWorkflowPolicy> =
            Arc::new(crate::maf_policy::NoMafWorkflowPolicy);
        let prompt_context = Arc::new(SwappablePromptContext::new(Arc::new(
            crate::prompt_context::NoopPromptContextProvider,
        )));
        let agent_deletion_hook =
            Arc::new(crate::agent_lifecycle::SwappableAgentDeletionHook::new(
                Arc::new(crate::agent_lifecycle::NoopAgentDeletionHook),
            ));
        let turn_artifacts = Arc::new(SwappableTurnArtifactSource::new(Arc::new(NoTurnArtifacts)));
        let attachment_store = Arc::new(SwappableAttachmentStore::new(Arc::new(
            ChatUploadStore::new(oci_storage.clone()),
        )));
        let marker_signer =
            nasiko_llm_router::attachments::MarkerSigner::from_secret(&config.agent_jwt_secret)
                .map(Arc::new);
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
        // Over Redis: a flow's events are published wherever the causing request lands (an
        // agent's /api/mcp call, a proxy hop), not where the user's chat stream is subscribed.
        let flow_events = FlowEventBus::with_redis(redis.clone());
        // A parked agent with no workload gets one from a worker that holds the state, spawned
        // below once the state exists.
        let (workload_deployer, workload_deploys) = crate::agents::catalog_deploy::deployer();
        let agent_wake = Arc::new(crate::agent_wake::SwappableAgentWake::new(Arc::new(
            crate::agent_wake::parked::ParkedAgentWake::new(
                crate::agent_wake::parked::WakeDeps {
                    db: db.clone(),
                    runtime: runtime.clone(),
                    http: http_client.clone(),
                    flow_events: flow_events.clone(),
                    deployer: Arc::new(workload_deployer),
                },
                std::time::Duration::from_secs(config.agent_wake_budget_secs()),
            ),
        )));
        let genai_metrics = GenAiMetrics::new();

        // Tempo+Loki observability backend. Model pricing resolves through
        // the model_pricing DB table with the static table as fallback; the
        // session resolver maps session ↔ trace both ways for agents
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
        let continuation_events =
            crate::hitl::continuation::ContinuationRegistry::new(redis.clone());
        // Pauses and resume outcomes are written from several places (the store, the MCP
        // gateway's inserts, both resume dispatchers); each reports through this one observer. A
        // second AppState in the same process (tests) keeps the first one's: the instruments sit
        // on the process-wide meter either way.
        nasiko_hitl::install_observer(Arc::new(genai_metrics.clone()));
        let hitl_store = agent_scaling.observe_hitl_store(Arc::new(
            nasiko_hitl::PgHitlStore::with_ttl_days(db.clone(), config.hitl_request_ttl_days),
        ));

        // MCP gateway state: reuses the same pool, redis client, and pooled
        // HTTP client — no duplicated infrastructure.
        let mut mcp = nasiko_mcp_gateway::McpState::new(
            db.clone(),
            redis.clone(),
            http_client.clone(),
            &config,
            Some(Arc::clone(&routing_backend)),
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
        // From here on, a rebuild on this replica also has every other replica rebuild.
        let invalidations = crate::invalidation::Invalidations::start(redis.clone());
        crate::mcp::search_sync::install(&mut mcp, db.clone(), &invalidations);

        let coding_agent_ingest = crate::coding_agent_ingest_guard::IngestGuard::new(
            &config.coding_agent_ingest,
            &crate::rate_limit::RateLimitStore::Redis(redis.clone()),
        );

        let state = Self {
            pricing,
            runtime,
            sandbox,
            db,
            redis,
            invalidations,
            oci_storage,
            usage_tracker,
            resource_stats,
            http_client,
            agent_client,
            auth,
            mcp,
            flow_guard,
            flow_events,
            genai_metrics,
            coding_agent_ingest,
            dashboard_limit: crate::dashboard_limit::DashboardLimit::new(
                config.dashboard_max_concurrent,
            ),
            config: Arc::new(config),
            routing_engine,
            llm_backend: routing_backend,
            orchestrator_policy,
            spend_gate,
            shared_call_payer: Arc::new(nasiko_llm_router::payer::CallerPays),
            session_admission,
            agent_admission: Arc::new(crate::agents::admission::AdmitEveryAgent),
            tiering_hold,
            agent_wake,
            maf_executor,
            maf_policy,
            prompt_context,
            agent_deletion_hook,
            agent_scaling,
            turn_artifacts,
            attachment_store,
            marker_signer,
            llm_config_cache: Arc::new(nasiko_llm_router::ConfigCache::new(
                std::time::Duration::from_secs(
                    nasiko_llm_router::config::GatewayConfig::llm_config_cache_ttl_secs_from_env(),
                ),
            )),
            history_embedding_cache,
            observability,
            github_svc,
            build_tx,
            hitl_store,
            hitl_resume_tx,
            continuation_events,
            ui_mounts: &[crate::auth::UiMount::ROOT],
            shutdown: crate::shutdown::ShutdownSignal::new(),
        };

        state.register_invalidation_handlers();

        // Spawn the durable build worker. It owns the receiver and exits when sender drops.
        let worker_state = state.clone();
        tokio::spawn(crate::agents::build_worker::run(worker_state, build_rx));

        // Deploys the missing workload of a parked agent the wake asks for: same shape as the
        // build worker above.
        tokio::spawn(crate::agents::catalog_deploy::run(
            state.clone(),
            workload_deploys,
        ));

        // Spawn the HITL resume dispatcher — same shape as the build worker above.
        // Handles `direct_chat`/`agent_proxy`/`maf`/`orchestrator`-origin rows (real A2A task
        // resume, or an XADD continuation job for MAF); its own `claim_for_resume` is scoped to
        // just those four origins. `mcp_tool` rows have their own separate dispatcher
        // (`nasiko_hitl::dispatcher`, wired up elsewhere in this function).
        let hitl_state = state.clone();
        tokio::spawn(crate::hitl::run(hitl_state, hitl_resume_rx));

        // Keeps the ready-replica count of recently called agents current, so the per-agent
        // in-flight band describes one replica's load (telemetry/agent_replicas.rs).
        tokio::spawn(crate::telemetry::agent_replicas::run(
            state.genai_metrics.agent_ready_replicas(),
            state.runtime.clone(),
        ));

        // The `mcp_tool`-origin resume dispatcher (AuthRequired's auto-resume nudge, and any
        // ToolApproval push) — lives inside `nasiko-hitl` since it isn't A2A-task-shaped (an MCP
        // tool call has no `task_id` to resume; `RuntimeResumeNotifier` sends a standalone nudge
        // message instead). Its own `claim_for_resume` is scoped to `origin = 'mcp_tool'`, so it
        // can run alongside the dispatcher above without racing it for the same rows.
        let resume_notifier: Arc<dyn nasiko_hitl::ResumeNotifier> = Arc::new(
            nasiko_hitl::RuntimeResumeNotifier::new(
                state.db.clone(),
                state.runtime.clone(),
                state.agent_client.clone(),
                // Same bound `gateway.rs`'s `flow_user` enforces on the agent's retry, so the
                // nudge never registers a window the gateway will reject.
                i64::from(state.config.flow_timeout_secs),
            )
            // The nudge may be for an agent that parked while its human decided.
            .with_resolver(Arc::new(crate::agent_resolver::ServerTargetResolver::new(
                state.clone(),
            ))),
        );
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

        tokio::spawn(crate::coding_agent_turn_summary_backfill::run(
            state.db.clone(),
        ));
        tokio::spawn(crate::coding_agent_backlog::run(
            state.db.clone(),
            state.coding_agent_ingest.metrics.clone(),
        ));

        if let Some(endpoint) = state.config.coding_agent_otlp_endpoint.clone() {
            tokio::spawn(crate::coding_agent_otlp::run(
                state.db.clone(),
                state.http_client.clone(),
                endpoint,
                state.config.coding_agent_ingest.accept_chat_history,
            ));
        }

        // Container-hours meter: records per-instance run sessions for billing
        // (see agents/hours_meter.rs). 0 disables — used by tests that drive
        // reconcile_once directly.
        if state.config.container_hours_poll_secs > 0 {
            tokio::spawn(crate::agents::hours_meter::run(
                state.db.clone(),
                state.runtime.clone(),
                state.sandbox.clone(),
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
                std::time::Duration::from_secs(state.config.trace_usage_max_window_secs),
                state.config.trace_usage_batch_size,
            ));
        }

        // Reclaims the storage of agent state no one can read any more: empties expired rows into
        // tombstones, forgets old tombstones, and removes the rows of deleted agents. Each process
        // runs one and `SKIP LOCKED` keeps several from colliding.
        crate::agent_state::warn_if_state_expires_before_hitl_requests(&state.config);
        tokio::spawn(crate::agent_state::sweeper::run(
            state.db.clone(),
            state.config.agent_state_ttl_days,
        ));

        // Closes flows whose request died mid-flight, so their sessions stop reading as running.
        tokio::spawn(crate::flow_sweeper::run(
            state.db.clone(),
            crate::flow_sweeper::stale_after(
                state.config.flow_timeout_secs,
                state.config.agent_call_timeout_secs,
            ),
        ));

        // Retires the seeded brevity factor once the holdout has enough samples. Cheap (one
        // grouped scan) and idempotent, so it rides the same process as the other workers rather
        // than needing a scheduler.
        tokio::spawn(crate::observability::savings_factors::run(
            state.db.clone(),
            std::time::Duration::from_secs(state.config.savings_factor_refresh_secs),
            state.config.savings_factor_min_samples,
            effective_factor_window(
                state.config.savings_factor_window_days,
                state.config.tiering_enabled,
            ),
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

        // Before the seeding below, which needs the key to register toolkits.
        crate::composio_settings::apply_stored_key(self).await;
        tokio::spawn(crate::composio_settings::watch_stored_key(self.clone()));

        // Before the seeding below, which compares each seed's stored image with its mirror: after
        // a rename, references to the registry's former hosts move to its current one first, and
        // the agents they moved are redeployed in the background (see `agents::rehost`).
        let registry = &self.config.agent_image_registry;
        if self.config.agent_image_registry_placeholder.as_ref() != Some(registry) {
            let aliases = &self.config.agent_image_registry_aliases;
            match crate::agents::rehost::rewrite_references(&self.db, registry, aliases).await {
                Ok(0) => {}
                Ok(queued) => tracing::info!(
                    queued,
                    %registry,
                    "registry rehost: image references moved to the current registry host"
                ),
                Err(e) => tracing::error!(%e, "registry rehost: moving image references failed"),
            }
            tokio::spawn(crate::agents::rehost::retry_until_drained(self.clone()));
        }

        crate::sessions::readiness::log_at_boot(self);
        // One replica seeds at a time: a replica that starts while another is seeding (a rolling
        // update starts the new pod beside the old) skips it rather than register and deploy the
        // same agents alongside it.
        let state = self.clone();
        tokio::spawn(async move {
            nasiko_leader::single_flight(&state.db.clone(), BOOT_SEED_LOCK, || async {
                crate::seed::seed_agents_if_configured(&state).await;
                // After the agents: each step binds to a seed agent's row, which
                // the line above registers (its deploy may still be in progress;
                // a step finds its agent's endpoint when it runs).
                crate::seed_workflows::seed_workflows_if_configured(&state).await;
                crate::seed::seed_toolkits_if_configured(&state).await;
                // After both: it pairs the seed agents' rows with the registered toolkits.
                crate::seed::grant_toolkits_to_seed_agents(&state).await;
            })
            .await;
        });

        // Lakehouse tiering (docs/LAKEHOUSE_TRD.md §8). Every replica spawns the
        // loop; only the holder of the `tiering_job` advisory lock does work.
        // With tiering off the tiering run never happens, so pending erasures get a
        // loop of their own: a recorded erasure always completes.
        if self.config.tiering_enabled {
            self.event_store().spawn_tiering();
        } else {
            self.event_store().spawn_erasure_retries();
        }

        // Resumed tiered sessions: orchestrator history reads merge Lance rows.
        let store: nasiko_event_store::EventStore = event_store(&self.config, &self.db);
        nasiko_orchestrator::session_history::set_cold_history_loader(std::sync::Arc::new(
            move |session_id: String| {
                let store: nasiko_event_store::EventStore = store.clone();
                Box::pin(async move {
                    let rows = store.load_session(&session_id).await.ok()?;
                    Some(
                        rows.into_iter()
                            .map(|r| nasiko_orchestrator::session_history::ColdHistoryRow {
                                role: r.role,
                                content: r.content,
                                metadata: r.metadata,
                            })
                            .collect(),
                    )
                })
            },
        ));
        // Separate task: a slow third-party MCP server must not wait behind,
        // or delay, the agent and toolkit seeding above.
        let state = self.clone();
        tokio::spawn(async move {
            crate::seed_connectors::seed_connectors_if_configured(&state).await;
        });

        // Covers e.g. a tenant cluster restore, which recreates the database
        // but not the individual agent Deployments/Services — see
        // `agents::reconcile`'s module doc.
        let state = self.clone();
        tokio::spawn(async move {
            nasiko_leader::single_flight(&state.db.clone(), STARTUP_RECONCILE_LOCK, || {
                crate::agents::reconcile::reconcile_agents_on_startup(&state)
            })
            .await;
        });

        // Periodic refresh of materialized views (agent_selection_stats), on one replica.
        let db = self.db.clone();
        tokio::spawn(async move {
            const PERIOD: std::time::Duration = std::time::Duration::from_secs(3600);
            // Not at startup: the first refresh waits a full period.
            tokio::time::sleep(PERIOD).await;
            nasiko_leader::every(db.clone(), MATVIEW_REFRESH_LOCK, PERIOD, || async {
                let views = ["REFRESH MATERIALIZED VIEW CONCURRENTLY agent_selection_stats"];
                for sql in views {
                    if let Err(e) = sqlx::query(sql).execute(&db).await {
                        tracing::warn!(view = sql, error = %e, "materialized view refresh failed (non-fatal)");
                    }
                }
                tracing::debug!("materialized views refreshed");
            })
            .await;
        });
    }

    /// Platform-level fallback env vars applied to every agent deployment
    /// when the agent has no secret of the same name. Also served to the CLI
    /// (`GET /api/agents/dev-env`, deployer+) so `nasiko run` can give local
    /// containers the same defaults a CP deployment would get.
    pub fn platform_fallback_env(&self) -> std::collections::HashMap<String, String> {
        build_platform_fallback_env(
            self.config.agent_platform_key_fallback,
            self.config.openai_api_key.as_deref(),
            self.config.openai_base_url.as_deref(),
            explicit_openai_model(&self.config.openai_model).as_deref(),
        )
    }

    /// Build the full environment for an agent container: the owner's secrets with the agent's own
    /// on top (`agent_secrets::resolve_secret_env`), platform-level vars where neither sets one,
    /// and feature flags from metadata.
    pub async fn agent_env(
        &self,
        agent_id: uuid::Uuid,
    ) -> std::collections::HashMap<String, String> {
        let owner_id: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1")
                .bind(agent_id)
                .fetch_optional(&self.db)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(%e, %agent_id, "agent_env: could not read the agent's owner");
                    None
                });
        let mut env = match owner_id {
            Some(owner_id) => {
                crate::catalog::agent_secrets::resolve_secret_env(&self.db, owner_id, agent_id)
                    .await
            }
            None => crate::catalog::agent_secrets::resolve_agent_env(&self.db, agent_id).await,
        };
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

/// Builds the platform fallback environment map for agent deployments.
///
/// When `agent_platform_key_fallback` is false, `OPENAI_API_KEY` is omitted.
/// `OPENAI_BASE_URL` and `OPENAI_MODEL` are included when set. An unset model is left out so the
/// LLM router wiring (`inject_llm_env`) fills in the router's default model.
fn build_platform_fallback_env(
    agent_platform_key_fallback: bool,
    openai_api_key: Option<&str>,
    openai_base_url: Option<&str>,
    openai_model: Option<&str>,
) -> std::collections::HashMap<String, String> {
    let mut env = std::collections::HashMap::new();
    if agent_platform_key_fallback && let Some(key) = openai_api_key {
        env.insert("OPENAI_API_KEY".into(), key.to_string());
    }
    if let Some(url) = openai_base_url {
        env.insert("OPENAI_BASE_URL".into(), url.to_string());
    }
    if let Some(model) = openai_model {
        env.insert("OPENAI_MODEL".into(), model.to_string());
    }
    env
}

/// The configured model only when the operator set `OPENAI_MODEL`; `None` for the compiled
/// `gpt-4o-mini` default, which a router without OpenAI credentials cannot serve.
pub(crate) fn explicit_openai_model(configured: &str) -> Option<String> {
    let set: bool = std::env::var("OPENAI_MODEL").is_ok_and(|m| !m.trim().is_empty());
    set.then(|| configured.to_owned())
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

    #[test]
    fn platform_fallback_env_leaves_unset_model_to_the_router() {
        let env = build_platform_fallback_env(true, Some("test-key"), None, None);
        assert!(!env.contains_key("OPENAI_MODEL"));
    }

    #[test]
    fn platform_fallback_env_omits_key_when_flag_is_false() {
        let env = build_platform_fallback_env(
            false,
            Some("test-key"),
            Some("https://api.test.com"),
            Some("test-model"),
        );
        assert!(
            !env.contains_key("OPENAI_API_KEY"),
            "OPENAI_API_KEY should be omitted when flag is false"
        );
        assert_eq!(
            env.get("OPENAI_BASE_URL"),
            Some(&"https://api.test.com".to_string())
        );
        assert_eq!(env.get("OPENAI_MODEL"), Some(&"test-model".to_string()));
    }

    #[test]
    fn platform_fallback_env_includes_key_when_flag_is_true() {
        let env = build_platform_fallback_env(true, Some("test-key"), None, Some("test-model"));
        assert_eq!(
            env.get("OPENAI_API_KEY"),
            Some(&"test-key".to_string()),
            "OPENAI_API_KEY should be present when flag is true"
        );
        assert_eq!(env.get("OPENAI_MODEL"), Some(&"test-model".to_string()));
        assert!(!env.contains_key("OPENAI_BASE_URL"));
    }
}

/// `s3://` cold paths reuse the server's `S3_*` settings (rustfs/MinIO/AWS).
/// `gs://` paths get no options: Lance's native GCS store authenticates with
/// Application Default Credentials (Workload Identity, or the `GOOGLE_*` env).
/// Event store over the configured cold path (erasure, cold reads).
pub fn event_store(c: &nasiko_config::Config, db: &sqlx::PgPool) -> nasiko_event_store::EventStore {
    let cfg = nasiko_event_store::TieringConfig {
        cold_path: c.cold_path.clone(),
        storage_options: cold_storage_options(c),
        interval: std::time::Duration::from_secs(c.tiering_interval_secs),
        ..Default::default()
    };
    nasiko_event_store::EventStore::new(db.clone(), cfg)
}

pub(crate) fn cold_storage_options(
    c: &nasiko_config::Config,
) -> std::collections::HashMap<String, String> {
    if !c.cold_path.starts_with("s3://") {
        return Default::default();
    }
    [
        ("aws_access_key_id", c.s3_access_key.clone()),
        ("aws_secret_access_key", c.s3_secret_key.clone()),
        ("aws_region", c.s3_region.clone()),
        ("aws_endpoint", c.s3_endpoint.clone()),
        (
            "allow_http",
            c.s3_endpoint.starts_with("http://").to_string(),
        ),
        ("aws_virtual_hosted_style_request", "false".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Hot window kept in Postgres when tiering is on (`TieringConfig::min_hot_days`).
const TIERING_HOT_DAYS: i64 = 30;

/// `savings_factors` reads raw `token_usage` metadata, which only covers the hot
/// window once tiering runs. A longer window would silently lose samples, so it is
/// capped (with a warning) instead (NAS-1163).
fn effective_factor_window(days: i64, tiering_enabled: bool) -> i64 {
    if tiering_enabled && days > TIERING_HOT_DAYS {
        tracing::warn!(
            requested = days,
            capped = TIERING_HOT_DAYS,
            "SAVINGS_FACTOR_WINDOW_DAYS exceeds the tiering hot window; capping"
        );
        return TIERING_HOT_DAYS;
    }
    days
}

#[cfg(test)]
mod factor_window_tests {
    use super::effective_factor_window;

    #[test]
    fn capped_only_when_tiering() {
        assert_eq!(effective_factor_window(90, true), 30);
        assert_eq!(effective_factor_window(90, false), 90);
        assert_eq!(effective_factor_window(14, true), 14);
    }
}
