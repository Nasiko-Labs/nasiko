//! Context selection — the user's two stored preferences and the one call
//! that turns them into selected conversation history.
//!
//! One concern: *given a user, what slice of their session history should this
//! request carry?* That answer needs three things, which used to live in three
//! places — the budget tier (`pacms_budget.rs`), the strategy
//! (`context_strategy.rs`), and the tier→numbers→fetch assembly, which every
//! caller open-coded for itself.
//!
//! The strategies themselves stay in [`crate::session_history`] (`fetch`,
//! `fetch_pacms`, `fetch_topk`) and the PACMS algorithm stays in
//! [`crate::pacms_selector`]; this module owns the *preferences* and the
//! *dispatch into them*.
//!
//! Callers want one function: [`fetch_for_user`].

use sqlx::PgPool;
use uuid::Uuid;

use crate::session_history::{ContextFetchConfig, SessionHistory};
use crate::vector_store::VectorStore;
use nasiko_compress::Policy;

// ── Stored preferences ────────────────────────────────────────────────────────

/// A user's chosen conversation-history tier. Mirrors the Postgres
/// `pacms_budget_level` enum (migration 0032) — deriving `sqlx::Type` lets
/// sqlx decode the column directly instead of treating it as TEXT.
///
/// The tier only names *which* level a user picked; the actual token counts
/// (`Pacms` strategy, via [`Self::tokens`]) and item counts (`TopK`/`LastK`
/// strategies, via [`Self::k`]) are both operator-configurable (see
/// [`ContextTiers`]), so an operator can retune what "high" means per
/// deployment without touching user data.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, sqlx::Type,
)]
#[sqlx(type_name = "pacms_budget_level", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PacmsBudgetLevel {
    Low,
    #[default]
    Medium,
    High,
}

impl PacmsBudgetLevel {
    /// Resolve this tier to a token count using the operator-configured values
    /// for each tier.
    pub fn tokens(&self, low: usize, medium: usize, high: usize) -> usize {
        match self {
            Self::Low => low,
            Self::Medium => medium,
            Self::High => high,
        }
    }

    /// Resolve this tier to an item count using the operator-configured
    /// values for each tier — the same tier `tokens()` reads, but for the
    /// `topk`/`lastk` context-selection strategies' pair/message count
    /// instead of PACMS's token budget.
    pub fn k(&self, low: usize, medium: usize, high: usize) -> usize {
        match self {
            Self::Low => low,
            Self::Medium => medium,
            Self::High => high,
        }
    }

    /// Look up a user's persisted preference. Falls back to `Medium` (the
    /// column default) if the row is missing or the query fails — a lookup
    /// hiccup must never block a chat request.
    pub async fn for_user(pool: &PgPool, user_id: Uuid) -> Self {
        sqlx::query_scalar::<_, Self>("SELECT pacms_budget_level FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }
}

/// A user's chosen conversation-history context-selection strategy. Mirrors
/// the Postgres `context_selection_strategy` enum (migration 0033) —
/// deriving `sqlx::Type` lets sqlx decode the column directly instead of
/// treating it as TEXT.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, sqlx::Type,
)]
#[sqlx(type_name = "context_selection_strategy", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ContextSelectionStrategy {
    /// Budget-aware, coverage-diversified selection (`SessionHistory::fetch_pacms`).
    /// Falls back to a token-budget-limited recency selection internally if
    /// embeddings fail.
    #[default]
    Pacms,
    /// Pure cosine-similarity ranking over query/answer pairs, no token
    /// budget (`SessionHistory::fetch_topk`). Falls back to plain recency
    /// (`SessionHistory::fetch`) if embeddings fail or the session has no pairs.
    TopK,
    /// Plain recency, no embeddings at all (`SessionHistory::fetch`).
    LastK,
}

impl ContextSelectionStrategy {
    /// Look up a user's persisted preference. Falls back to `Pacms` (the
    /// column default) if the row is missing or the query fails — a lookup
    /// hiccup must never block a chat request.
    pub async fn for_user(pool: &PgPool, user_id: Uuid) -> Self {
        sqlx::query_scalar::<_, Self>("SELECT context_selection_strategy FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }
}

// ── Operator-configured tier values ───────────────────────────────────────────

/// What each [`PacmsBudgetLevel`] tier actually *means* in this deployment —
/// the operator-tunable numbers a stored tier resolves against.
///
/// Both config structs that carry these (`nasiko_config::Config` for the
/// server call sites, [`crate::engine::RouterConfig`] for the routing engine)
/// build one of these rather than each re-deriving the token/item counts
/// itself.
#[derive(Debug, Clone, Copy)]
pub struct ContextTiers {
    /// Candidate pool `Pacms` selects a budget-fitting subset from.
    pub pool_size: usize,
    /// Most-recent messages `Pacms` always keeps, whatever they score.
    pub mandatory_recent: usize,
    /// `Pacms` token budget per tier.
    pub budget_low: usize,
    pub budget_medium: usize,
    pub budget_high: usize,
    /// `TopK`/`LastK` item count per tier.
    pub k_low: usize,
    pub k_medium: usize,
    pub k_high: usize,
    /// History compression (IP-4), applied at the read so every strategy
    /// budgets and embeds the text that will actually be sent. Deployment-wide,
    /// not per tier: the tier chooses how much context a user gets, and
    /// compression changes how much fits — the two are independent knobs.
    pub compress: Policy<'static>,
}

impl Default for ContextTiers {
    /// Mirrors the `env_parse` defaults in `nasiko_config::Config`.
    fn default() -> Self {
        Self {
            pool_size: 150,
            mandatory_recent: 3,
            budget_low: 500,
            budget_medium: 1000,
            budget_high: 5000,
            k_low: 1,
            k_medium: 5,
            k_high: 20,
            compress: Policy::default(),
        }
    }
}

impl ContextTiers {
    /// The single place the eight `PACMS_*`/`CONTEXT_K_*` env keys become a
    /// tier table — both the routing engine and the server's chat handlers
    /// come through here rather than each reading `config` field by field.
    pub fn from_config(config: &nasiko_config::Config) -> Self {
        Self {
            pool_size: config.pacms_history_pool_size,
            mandatory_recent: config.pacms_history_mandatory_recent,
            budget_low: config.pacms_budget_low,
            budget_medium: config.pacms_budget_medium,
            budget_high: config.pacms_budget_high,
            k_low: config.context_k_low,
            k_medium: config.context_k_medium,
            k_high: config.context_k_high,
            compress: Policy {
                enabled: config.history_compress_enabled,
                min_bytes: config.history_compress_min_bytes,
                ..Policy::default()
            },
        }
    }

    /// Resolve this deployment's tier table against one user's stored tier.
    fn resolve(&self, level: PacmsBudgetLevel) -> ContextFetchConfig {
        let k = level.k(self.k_low, self.k_medium, self.k_high);
        ContextFetchConfig {
            pool_size: self.pool_size,
            token_budget: level.tokens(self.budget_low, self.budget_medium, self.budget_high),
            mandatory_recent: self.mandatory_recent,
            topk_count: k,
            lastk_limit: k,
            compress: self.compress,
        }
    }
}

// ── The one entry point ───────────────────────────────────────────────────────

/// Fetch the conversation history for `query` the way `user_id` has asked for
/// it: look up their strategy and budget tier, resolve the tier against this
/// deployment's [`ContextTiers`], and dispatch into the matching
/// `SessionHistory::fetch_*`.
///
/// The two preference lookups are two cheap indexed row reads, issued
/// concurrently. Neither can fail the request — both fall back to their
/// column default (see `for_user` on each).
/// Whether the orchestrator-scoped compression layers (IP-3 tool results, IP-4 session history)
/// are opted into for `user_id`.
///
/// # Why this is aggregated rather than read off one agent
///
/// `agents.compress_enabled` is a per-agent switch, and the per-request layers (IP-1, IP-2, IP-5)
/// read it off the agent actually being called. These two layers have no such agent: the ReAct
/// loop stores results from whichever agents it routes to, and history is fetched *before* any
/// routing happens. An orchestrator-routed `chat_sessions` row carries `agent_id = NULL` for
/// exactly that reason (`a2a_dispatch.rs`), so there is nothing single to read.
///
/// So the rule is unanimity: on only when the caller owns at least one live agent and **every**
/// one of them has opted in. That keeps the switch behaving as one layer — flip it and the whole
/// stack moves — while degrading safely: adding an agent that has not opted in turns the shared
/// context back to verbatim rather than quietly compressing text destined for it.
pub async fn compression_opt_in(pool: &PgPool, user_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT count(*) > 0 AND bool_and(compress_enabled) \
         FROM agents WHERE owner_id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

pub async fn fetch_for_user(
    pool: &PgPool,
    user_id: Uuid,
    session_id: &str,
    vector_store: &VectorStore,
    query: &str,
    tiers: &ContextTiers,
) -> SessionHistory {
    let (level, strategy, opted_in) = tokio::join!(
        PacmsBudgetLevel::for_user(pool, user_id),
        ContextSelectionStrategy::for_user(pool, user_id),
        compression_opt_in(pool, user_id),
    );
    // IP-4 is gated by the deployment flag AND the per-agent switch, so the UI toggle stops it
    // as part of one layer rather than leaving history compressed after the rest is off.
    let mut cfg = tiers.resolve(level);
    cfg.compress.enabled = cfg.compress.enabled && opted_in;

    SessionHistory::fetch_context(strategy, session_id, pool, vector_store, query, &cfg).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers() -> ContextTiers {
        ContextTiers {
            pool_size: 150,
            mandatory_recent: 3,
            budget_low: 500,
            budget_medium: 1000,
            budget_high: 5000,
            k_low: 1,
            k_medium: 5,
            k_high: 20,
            compress: Default::default(),
        }
    }

    #[test]
    fn tier_resolves_to_its_own_budget_and_k() {
        let t = tiers();

        let low = t.resolve(PacmsBudgetLevel::Low);
        assert_eq!(low.token_budget, 500);
        assert_eq!(low.topk_count, 1);
        assert_eq!(low.lastk_limit, 1);

        let high = t.resolve(PacmsBudgetLevel::High);
        assert_eq!(high.token_budget, 5000);
        assert_eq!(high.topk_count, 20);
        assert_eq!(high.lastk_limit, 20);
    }

    #[test]
    fn pool_and_mandatory_recent_are_tier_independent() {
        let t = tiers();
        for level in [
            PacmsBudgetLevel::Low,
            PacmsBudgetLevel::Medium,
            PacmsBudgetLevel::High,
        ] {
            let cfg = t.resolve(level);
            assert_eq!(cfg.pool_size, 150);
            assert_eq!(cfg.mandatory_recent, 3);
        }
    }

    #[test]
    fn defaults_match_the_column_defaults() {
        assert_eq!(PacmsBudgetLevel::default(), PacmsBudgetLevel::Medium);
        assert_eq!(
            ContextSelectionStrategy::default(),
            ContextSelectionStrategy::Pacms
        );
    }
}
