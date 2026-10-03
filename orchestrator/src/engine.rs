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
    selector: AgentSelector,
    api_key: String,
    base_url: String,
    embedding_model: String,
    /// Cache of PACMS candidate/query embeddings shared across `route()` calls.
    /// PACMS's history pool overlaps heavily turn-to-turn within a session, so
    /// without this `SessionHistory::fetch_pacms` would re-embed the same
    /// messages on every call. See `TextEmbeddingCache` docs.
    history_embedding_cache: TextEmbeddingCache,
}

impl OssRoutingEngine {
    pub fn new(
        config: RouterConfig,
        http_client: Client,
        api_key: String,
        base_url: String,
        router_model: String,
        embedding_model: String,
    ) -> Self {
        let provider = LLMProvider::new(http_client, api_key.clone(), base_url.clone());
        let selector = AgentSelector::new(provider, router_model);
        Self {
            config,
            selector,
            api_key,
            base_url,
            embedding_model,
            history_embedding_cache: Arc::new(DashMap::new()),
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
        let history_store = VectorStore::for_embedding(
            self.api_key.clone(),
            self.base_url.clone(),
            self.embedding_model.clone(),
            Arc::clone(&self.history_embedding_cache),
        );
        let (agents, history) = tokio::join!(
            agent_registry::get_agents_for_user(req.user_id, pool),
            context_selection::fetch_for_user(
                pool,
                req.user_id,
                &req.session_id,
                &history_store,
                &req.query,
                &self.config.context_tiers,
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
            VectorStore::build(
                agents.clone(),
                self.api_key.clone(),
                self.base_url.clone(),
                self.embedding_model.clone(),
                pool,
            )
            .await
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

        let (selected_agent, fallback_used, reasoning, selector_usage) = match self
            .selector
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
                let selection_token_usage_id =
                    write_selector_token_usage(pool, req.user_id, &req.session_id, &usage).await;
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

        // Write selector token usage to the token_usage table (fire-and-forget)
        let selection_token_usage_id: Option<Uuid> = if let Some(ref cr) = selector_usage {
            write_selector_token_usage(pool, req.user_id, &req.session_id, cr).await
        } else {
            None
        };

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

/// Writes the Stage 3 selector's token usage to the `token_usage` table.
/// Returns the UUID of the inserted row, or None on failure.
async fn write_selector_token_usage(
    pool: &PgPool,
    user_id: Uuid,
    session_id: &str,
    cr: &crate::providers::CompletionResult,
) -> Option<Uuid> {
    let input = cr.usage.prompt_tokens;
    let output = cr.usage.completion_tokens;
    let total = cr.usage.total_tokens;

    let result = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO token_usage (
            user_id, operation_type, session_id, provider, model,
            input_tokens, output_tokens, total_tokens,
            latency_ms, streaming, metadata
        ) VALUES ($1, 'router_selection', $2, $3, $4, $5, $6, $7, $8, false, '{}'::jsonb)
        RETURNING id"#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(&cr.provider)
    .bind(&cr.model)
    .bind(input)
    .bind(output)
    .bind(total)
    .bind(cr.latency_ms)
    .fetch_one(pool)
    .await;

    match result {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, "failed to write selector token usage (non-fatal)");
            None
        }
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
