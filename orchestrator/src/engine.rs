use async_trait::async_trait;
use dashmap::DashMap;
use reqwest::Client;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use crate::agent_registry;
use crate::context_selection::{self, ContextTiers};
use crate::error::RouterError;
use crate::models::AgentCardSummary;
use crate::policy::RoutingPolicy;
use crate::providers::LLMProvider;
use crate::reranker::Reranker;
use crate::selector::AgentSelector;
use crate::selector::ConversationMessage;
use crate::types::{AgentCard, RouteRequest, RouteResult, RouterLogEntry};
use crate::user_key;
use crate::vector_store::{TextEmbeddingCache, VectorStore};

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait RoutingEngine: Send + Sync {
    /// `policy` is resolved by the caller rather than by `route()` itself, so a
    /// caller making several `route()` calls for one request (one per MAF
    /// workflow step, for example) can resolve it once and share it, instead of
    /// every call paying for its own lookup. A caller with no policy to apply
    /// passes `None`, and routing is unconstrained.
    async fn route(
        &self,
        req: RouteRequest,
        pool: &PgPool,
        policy: Option<&dyn RoutingPolicy>,
    ) -> Result<RouteResult, RouterError>;
}

// ── Config ────────────────────────────────────────────────────────────────────

pub struct RouterConfig {
    /// Skip Stage 1 embedding shortlist when catalogue is smaller than this.
    pub shortlist_threshold: usize,
    /// Max candidates passed into Stage 3 (LLM selector).
    pub shortlist_size: usize,
    /// What each stored `PacmsBudgetLevel` tier means in this deployment —
    /// the token/item counts the user's chosen tier resolves against.
    pub context_tiers: ContextTiers,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            shortlist_threshold: 15,
            shortlist_size: 10,
            context_tiers: ContextTiers::default(),
        }
    }
}

// ── OSS Engine ────────────────────────────────────────────────────────────────

pub struct OssRoutingEngine {
    config: RouterConfig,
    http_client: Client,
    router_model: String,
    api_key: String,
    base_url: String,
    embedding_model: String,
    /// Cache of PACMS candidate/query embeddings shared across `route()` calls.
    /// PACMS's history pool overlaps heavily turn-to-turn within a session, so
    /// without this `SessionHistory::fetch_pacms` would re-embed the same
    /// messages on every call. See `TextEmbeddingCache` docs.
    history_embedding_cache: TextEmbeddingCache,
    /// In-process LLM router for platform-key calls (embeddings + selector).
    backend: Option<crate::llm_backend::SharedLlmBackend>,
}

impl OssRoutingEngine {
    /// Route platform-key embeddings and selection through `backend`.
    pub fn with_backend(mut self, backend: crate::llm_backend::SharedLlmBackend) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn new(
        config: RouterConfig,
        http_client: Client,
        api_key: String,
        base_url: String,
        router_model: String,
        embedding_model: String,
    ) -> Self {
        Self {
            config,
            http_client,
            router_model,
            api_key,
            base_url,
            embedding_model,
            history_embedding_cache: Arc::new(DashMap::new()),
            backend: None,
        }
    }

    pub fn from_config(config: &nasiko_config::Config, http_client: Client) -> Self {
        let router_config = RouterConfig {
            shortlist_threshold: config.router_shortlist_threshold,
            shortlist_size: config.router_shortlist_size,
            context_tiers: ContextTiers::from_config(config),
        };
        Self::new(
            router_config,
            http_client,
            config.openai_api_key.clone().unwrap_or_default(),
            config
                .openai_base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com".into()),
            config.router_model.clone(),
            config.embedding_model.clone(),
        )
    }
}

#[async_trait]
impl RoutingEngine for OssRoutingEngine {
    async fn route(
        &self,
        req: RouteRequest,
        pool: &PgPool,
        policy: Option<&dyn RoutingPolicy>,
    ) -> Result<RouteResult, RouterError> {
        tracing::info!(query = %req.query, "routing_engine: route() start");
        let t0 = Instant::now();

        // Fetch available agents + conversation history in parallel. History
        // is selected per the caller's own stored strategy and budget tier —
        // see `context_selection::fetch_for_user`.
        // The key a user saved from the UI wins over the server's own.
        let saved_key: Option<String> = user_key::saved_openai_key(pool, req.user_id).await;
        // No saved user key + router available: platform calls go through the router.
        let routed: Option<crate::vector_store::RoutedEmbedder> = match (&saved_key, &self.backend)
        {
            (None, Some(b)) => Some(crate::vector_store::RoutedEmbedder {
                backend: Arc::clone(b),
                call: crate::llm_backend::LlmCallContext {
                    user_id: req.user_id,
                    component: crate::llm_backend::PlatformComponent::RouterSelection.to_string(),
                    session_id: Some(req.session_id.clone()),
                },
            }),
            _ => None,
        };
        let api_key: String = saved_key.unwrap_or_else(|| self.api_key.clone());
        // Direct (non-routed) calls hit OpenAI: use an OpenAI embedding model there.
        let direct_model: String =
            crate::providers::direct_embedding_model(&self.embedding_model, &self.base_url);
        let history_store = match &routed {
            Some(r) => VectorStore::for_embedding_routed(
                r.clone(),
                self.embedding_model.clone(),
                Arc::clone(&self.history_embedding_cache),
            ),
            None => VectorStore::for_embedding(
                api_key.clone(),
                self.base_url.clone(),
                direct_model.clone(),
                Arc::clone(&self.history_embedding_cache),
            ),
        };
        // Resolved before the join: the workspace override is a borrow, and a temporary inside the
        // join would not outlive it.
        let tiers = self.config.context_tiers.with_overrides(pool).await;
        let (agents, history) = tokio::join!(
            agent_registry::get_agents_for_user(req.user_id, &req.org_granted_agent_ids, pool),
            context_selection::fetch_for_user(
                pool,
                req.user_id,
                &req.session_id,
                &history_store,
                &req.query,
                &tiers,
                // Orchestrator-routed: the Orchestrator's own model reads this history, so no
                // agent's switch gates it and the deployment flag alone decides.
                true,
            ),
        );
        let agents = agents?;

        if agents.is_empty() {
            return Err(RouterError::NoAgentsAvailable);
        }

        let registry_ms = t0.elapsed().as_millis() as i32;
        tracing::info!(
            agent_count = agents.len(),
            elapsed_ms = registry_ms,
            "routing_engine: registry+history fetched"
        );

        // Stage 1 — vector store semantic shortlist (OpenAI embeddings, skipped if no key)
        let t1 = Instant::now();
        let store = Arc::new(if agents.len() < self.config.shortlist_threshold {
            tracing::info!("routing_engine: stage 1 (shortlist) skipped — fleet below threshold");
            // Catalog too small for semantic shortlisting to matter — skip
            // embedding entirely rather than paying for embeddings API calls
            // we're going to throw away (shortlist() would return `all` anyway).
            VectorStore::disabled_from(agents.clone())
        } else {
            tracing::info!("routing_engine: stage 1 (shortlist) — building vector store");
            match &routed {
                Some(r) => {
                    VectorStore::build_routed(
                        agents.clone(),
                        r.clone(),
                        self.embedding_model.clone(),
                        pool,
                    )
                    .await
                }
                None => {
                    VectorStore::build(
                        agents.clone(),
                        api_key.clone(),
                        self.base_url.clone(),
                        direct_model.clone(),
                        pool,
                    )
                    .await
                }
            }
        });
        let shortlist = store
            .shortlist(
                &req.query,
                self.config.shortlist_size,
                self.config.shortlist_threshold,
            )
            .await;
        let stage1_count = shortlist.len();
        let stage1_ms = t1.elapsed().as_millis() as i32;
        tracing::info!(
            candidates = stage1_count,
            elapsed_ms = stage1_ms,
            "routing_engine: stage 1 (shortlist) done"
        );

        // Stage 2 — conversation-aware reranking
        let t2 = Instant::now();
        let reranker = Reranker::new(Arc::clone(&store));
        let candidates = reranker
            .rerank(shortlist, &history, &req.query, self.config.shortlist_size)
            .await;
        let stage2_count = candidates.len();
        tracing::info!(
            candidates = stage2_count,
            elapsed_ms = t2.elapsed().as_millis() as i32,
            "routing_engine: stage 2 (rerank) done"
        );

        if candidates.is_empty() {
            return Err(RouterError::NoAgentsAvailable);
        }

        // Stage 3 — LLM final selection
        tracing::info!("routing_engine: stage 3 (select) — calling LLM");
        let t3 = Instant::now();
        let summaries: Vec<AgentCardSummary> = candidates.iter().map(card_to_summary).collect();
        let history_msgs: Vec<ConversationMessage> = history
            .to_llm_messages()
            .into_iter()
            .map(|m| ConversationMessage {
                role: m.role,
                content: m.content,
            })
            .collect();

        let mut provider =
            LLMProvider::new(self.http_client.clone(), api_key, self.base_url.clone());
        if let Some(r) = &routed {
            provider = provider.with_backend(Arc::clone(&r.backend), r.call.clone());
        }
        // Router writes trace_usage for routed calls; skip our own write.
        let record_usage: bool = routed.is_none();
        let selector = AgentSelector::new(provider, self.router_model.clone());
        let (selected_agent, fallback_used, reasoning, selector_usage) = match selector
            .select_agent(&req.query, &history_msgs, &summaries, policy)
            .await
        {
            Ok((sel, completion_result, hallucinated_fallback)) => {
                let agent = candidates
                    .iter()
                    .find(|a| a.id == sel.agent_id)
                    .cloned()
                    .unwrap_or_else(|| candidates[0].clone());
                let reasoning = sel.reasoning.clone();
                let usage = Some(completion_result);
                (agent, hallucinated_fallback, reasoning, usage)
            }
            // A refusal is a decision, not a failure: propagate it. The
            // first-candidate fallback below exists for infrastructure faults
            // (provider down, unparseable response) where delegating to *some*
            // agent still beats erroring — but applying it here would hand the
            // request to an agent the model just said cannot do the job, which
            // is precisely what a policy is for.
            Err(crate::selector::SelectorError::PolicyRefused { reason, usage }) => {
                tracing::info!(
                    %reason,
                    agents_considered = candidates.len(),
                    "routing refused by the operator's policy"
                );
                // The refused selection cost exactly what an accepted one
                // costs — the provider call already happened — so its tokens
                // are recorded on the same path and by the same helper. Written
                // before the log row rather than after, because the row points
                // at it; skipping this is what made a tuning session's rejected
                // calls free in FinOps.
                if record_usage {
                    record_selector_usage(pool, req.user_id, &req.session_id, &usage).await;
                }
                // trace_usage rows are keyed (trace_id, agent_name), not by uuid.
                let selection_token_usage_id: Option<Uuid> = None;
                // A refusal is still a routing decision: log it like any other,
                // so it isn't invisible to /api/orchestrator/stats and FinOps —
                // only successful selections used to reach this log table.
                spawn_router_log(
                    pool,
                    RouterLogEntry {
                        request_id: Uuid::new_v4().to_string(),
                        user_id: req.user_id,
                        session_id: req.session_id,
                        query: req.query,
                        agents_considered: agents.len() as i32,
                        selected_agent_id: None,
                        selected_agent_name: None,
                        selection_reasoning: None,
                        fallback_used: false,
                        total_latency_ms: t0.elapsed().as_millis() as i32,
                        registry_fetch_ms: Some(registry_ms),
                        stage1_candidates: Some(stage1_count as i32),
                        stage2_candidates: Some(stage2_count as i32),
                        embedding_model: Some(self.embedding_model.clone()),
                        selection_llm_ms: Some(t3.elapsed().as_millis() as i32),
                        file_count: req.file_parts.len() as i32,
                        selection_token_usage_id,
                        success: false,
                        // The policy's own wording, relayed rather than
                        // rephrased — this crate does not know what it checked.
                        error_message: Some(reason.clone()),
                    },
                );
                return Err(RouterError::PolicyRefused { reason });
            }
            Err(e) => {
                tracing::warn!(%e, "Stage 3 selector failed, using first candidate as fallback");
                (candidates[0].clone(), true, format!("fallback: {e}"), None)
            }
        };
        let stage3_ms = t3.elapsed().as_millis() as i32;
        let total_ms = t0.elapsed().as_millis() as i32;
        tracing::info!(
            selected_agent = %selected_agent.name,
            fallback_used,
            elapsed_ms = stage3_ms,
            "routing_engine: stage 3 (select) done"
        );
        tracing::info!(total_elapsed_ms = total_ms, "routing_engine: route() done");

        // Add selector usage to trace_usage (non-fatal). The log row's
        // selection_token_usage_id stays NULL: trace_usage has no row uuid.
        if let Some(ref cr) = selector_usage.filter(|_| record_usage) {
            record_selector_usage(pool, req.user_id, &req.session_id, cr).await;
        }
        let selection_token_usage_id: Option<Uuid> = None;

        // Fire-and-forget log insert (never blocks the response)
        let entry = RouterLogEntry {
            request_id: Uuid::new_v4().to_string(),
            user_id: req.user_id,
            session_id: req.session_id,
            query: req.query,
            agents_considered: agents.len() as i32,
            selected_agent_id: Some(selected_agent.id),
            selected_agent_name: Some(selected_agent.name.clone()),
            selection_reasoning: Some(reasoning),
            fallback_used,
            total_latency_ms: total_ms,
            registry_fetch_ms: Some(registry_ms),
            stage1_candidates: Some(stage1_count as i32),
            stage2_candidates: Some(stage2_count as i32),
            embedding_model: Some(self.embedding_model.clone()),
            selection_llm_ms: Some(stage3_ms),
            file_count: req.file_parts.len() as i32,
            selection_token_usage_id,
            success: true,
            error_message: None,
        };
        spawn_router_log(pool, entry);

        Ok(RouteResult {
            agent: selected_agent,
            fallback_used,
        })
    }
}

// ── Logging ───────────────────────────────────────────────────────────────────

pub async fn write_router_log(pool: &PgPool, e: RouterLogEntry) {
    let result = sqlx::query(
        r#"INSERT INTO router_request_log (
            request_id, user_id, session_id, query,
            agents_considered, selected_agent_id, selected_agent_name,
            selection_reasoning, fallback_used, selection_token_usage_id,
            total_latency_ms, registry_fetch_ms, selection_llm_ms,
            stage1_candidates, stage2_candidates, embedding_model,
            success, error_message, file_count, streaming
        ) VALUES (
            $1, $2, $3, $4,
            $5, $6, $7,
            $8, $9, $10,
            $11, $12, $13,
            $14, $15, $16,
            $17, $18, $19, false
        )"#,
    )
    .bind(&e.request_id)
    .bind(e.user_id)
    .bind(&e.session_id)
    .bind(&e.query)
    .bind(e.agents_considered)
    .bind(e.selected_agent_id)
    .bind(&e.selected_agent_name)
    .bind(&e.selection_reasoning)
    .bind(e.fallback_used)
    .bind(e.selection_token_usage_id)
    .bind(e.total_latency_ms)
    .bind(e.registry_fetch_ms)
    .bind(e.selection_llm_ms)
    .bind(e.stage1_candidates)
    .bind(e.stage2_candidates)
    .bind(&e.embedding_model)
    .bind(e.success)
    .bind(&e.error_message)
    .bind(e.file_count)
    .execute(pool)
    .await;

    if let Err(err) = result {
        tracing::warn!(%err, "failed to write orchestrator log (non-fatal)");
    }
}

/// Fire-and-forget: write one `router_request_log` row without blocking the caller.
fn spawn_router_log(pool: &PgPool, entry: RouterLogEntry) {
    let pool = pool.clone();
    tokio::spawn(async move {
        write_router_log(&pool, entry).await;
    });
}

/// Writes the selector's usage row in its own task and waits for it.
///
/// The provider call is already paid for, so the row must land even if the
/// caller's `route()` future is dropped mid-write (client disconnect). The
/// spawned task outlives the caller; awaiting the handle keeps the write
/// ordered before the router log row.
async fn record_selector_usage(
    pool: &PgPool,
    user_id: Uuid,
    session_id: &str,
    cr: &crate::providers::CompletionResult,
) {
    let pool: PgPool = pool.clone();
    let session_id: String = session_id.to_owned();
    let cr: crate::providers::CompletionResult = cr.clone();
    let handle: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        write_selector_usage(&pool, user_id, &session_id, &cr).await;
    });
    if let Err(err) = handle.await {
        tracing::warn!(%err, "selector usage task failed (non-fatal)");
    }
}

/// Adds the Stage 3 selector's token usage to `trace_usage` under
/// `(session_id, "router_selection")`. Failures are logged and swallowed.
async fn write_selector_usage(
    pool: &PgPool,
    user_id: Uuid,
    session_id: &str,
    cr: &crate::providers::CompletionResult,
) {
    let usage = crate::maf::llm::LlmUsage {
        input_tokens: cr.usage.prompt_tokens as i64,
        output_tokens: cr.usage.completion_tokens as i64,
        total_tokens: cr.usage.total_tokens as i64,
        cached_tokens: 0,
        model: cr.model.clone(),
        latency_ms: cr.latency_ms as i64,
    };
    let metadata = serde_json::json!({ "operation_type": "router_selection" });
    // Not a MAF call, so not one for the ledger, which would record it as one. The selector
    // records no key source, so a ledger entry could not say who paid for it.
    let result = crate::maf::executor::insert_trace_usage(
        pool,
        &nasiko_pricing::NoUsageLedger,
        user_id,
        Some(session_id),
        "router_selection",
        &cr.provider,
        &usage,
        metadata,
    )
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to write selector usage (non-fatal)");
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn card_to_summary(a: &AgentCard) -> AgentCardSummary {
    AgentCardSummary {
        id: a.id,
        name: a.name.clone(),
        description: a.description.clone(),
        skills: a
            .skills
            .iter()
            .map(|s| crate::models::SkillSummary {
                name: s.clone(),
                description: s.clone(),
                // This card carries skills as bare strings — no examples to carry.
                examples: Vec::new(),
            })
            .collect(),
        tags: a.tags.clone(),
    }
}
