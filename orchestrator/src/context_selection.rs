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

use crate::session_history::{ContextFetchConfig, ContextReport, SessionHistory};
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
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    sqlx::Type,
    utoipa::ToSchema,
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

    /// The wire and ledger spelling of this tier. One function, because the string ends up in
    /// `token_savings.context_tier`, in `context_reports.level` and in an API response, and three
    /// hand-written `match`es would be three chances to drift.
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Look up a user's persisted preference. Falls back to `Medium` (the
    /// column default) if the row is missing or the query fails — a lookup
    /// hiccup must never block a chat request.
    pub async fn for_user(pool: &PgPool, user_id: Uuid) -> Self {
        Self::for_user_checked(pool, user_id)
            .await
            .unwrap_or_default()
    }

    /// Same lookup, but a real database error is reported rather than swallowed into the default
    /// (CX-4) — for the `GET /api/me/pacms-budget` read path, which must 500 on a failed read
    /// instead of quietly claiming the default was saved. A missing row (`fetch_optional` returning
    /// `None` — no such user) still resolves to the column default, same as `for_user`; only an
    /// actual query failure is `Err`.
    pub async fn for_user_checked(pool: &PgPool, user_id: Uuid) -> Result<Self, sqlx::Error> {
        Ok(
            sqlx::query_scalar::<_, Self>("SELECT pacms_budget_level FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(pool)
                .await?
                .unwrap_or_default(),
        )
    }
}

/// A user's chosen conversation-history context-selection strategy. Mirrors
/// the Postgres `context_selection_strategy` enum (migration 0033) —
/// deriving `sqlx::Type` lets sqlx decode the column directly instead of
/// treating it as TEXT.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    sqlx::Type,
    utoipa::ToSchema,
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
    /// The wire and ledger spelling of this strategy. See [`PacmsBudgetLevel::as_label`].
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Pacms => "pacms",
            Self::TopK => "topk",
            Self::LastK => "lastk",
        }
    }

    /// Look up a user's persisted preference. Falls back to `Pacms` (the
    /// column default) if the row is missing or the query fails — a lookup
    /// hiccup must never block a chat request.
    pub async fn for_user(pool: &PgPool, user_id: Uuid) -> Self {
        Self::for_user_checked(pool, user_id)
            .await
            .unwrap_or_default()
    }

    /// Same lookup, but a real database error is reported rather than swallowed into the default
    /// (CX-4) — for the `GET /api/me/context-strategy` read path. See
    /// [`PacmsBudgetLevel::for_user_checked`] for the missing-row-vs-error distinction.
    pub async fn for_user_checked(pool: &PgPool, user_id: Uuid) -> Result<Self, sqlx::Error> {
        Ok(sqlx::query_scalar::<_, Self>(
            "SELECT context_selection_strategy FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_default())
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
    /// This deployment's tiers with any workspace override applied (CX-T1).
    ///
    /// The environment is the floor, the `settings` row is the override, and a NULL column means
    /// "not chosen here". Resolved per request rather than at startup, so changing a tier takes
    /// effect on the next turn instead of the next restart — the tiers page is a settings screen,
    /// and a settings screen that needs a deploy to take effect is a trap.
    ///
    /// A failed read falls back to the environment. A database hiccup must not silently halve
    /// somebody's context budget.
    pub async fn with_overrides(&self, db: &PgPool) -> Self {
        #[derive(sqlx::FromRow)]
        struct Row {
            pacms_budget_low: Option<i32>,
            pacms_budget_medium: Option<i32>,
            pacms_budget_high: Option<i32>,
            context_k_low: Option<i32>,
            context_k_medium: Option<i32>,
            context_k_high: Option<i32>,
            compress_history: Option<bool>,
            context_pool_size: Option<i32>,
            context_mandatory_recent: Option<i32>,
            compress_min_bytes: Option<i32>,
        }

        let Ok(Some(row)) = sqlx::query_as::<_, Row>(
            "SELECT pacms_budget_low, pacms_budget_medium, pacms_budget_high, \
                    context_k_low, context_k_medium, context_k_high, compress_history, \
                    context_pool_size, context_mandatory_recent, compress_min_bytes \
               FROM settings WHERE id = 1",
        )
        .fetch_optional(db)
        .await
        else {
            return *self;
        };

        let pos = |v: Option<i32>| v.filter(|n| *n > 0).map(|n| n as usize);
        let mut out = *self;
        out.budget_low = pos(row.pacms_budget_low).unwrap_or(out.budget_low);
        out.budget_medium = pos(row.pacms_budget_medium).unwrap_or(out.budget_medium);
        out.budget_high = pos(row.pacms_budget_high).unwrap_or(out.budget_high);
        out.k_low = pos(row.context_k_low).unwrap_or(out.k_low);
        out.k_medium = pos(row.context_k_medium).unwrap_or(out.k_medium);
        out.k_high = pos(row.context_k_high).unwrap_or(out.k_high);
        out.pool_size = pos(row.context_pool_size).unwrap_or(out.pool_size);
        out.mandatory_recent = pos(row.context_mandatory_recent).unwrap_or(out.mandatory_recent);
        if let Some(enabled) = row.compress_history {
            // Narrowing only: the workspace can switch history compression off, never on against a
            // deployment that has not enabled the layer at all.
            out.compress.enabled = out.compress.enabled && enabled;
        }
        if let Some(min) = pos(row.compress_min_bytes) {
            out.compress.min_bytes = min;
        }
        out
    }

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

// ── Preview (CX-6) ────────────────────────────────────────────────────────────

/// What one tier would keep.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct TierCounts {
    pub messages: usize,
    pub tokens: usize,
}

/// A dry run of selection over the caller's latest chat, at every tier.
///
/// Shown beside the tier picker so the choice is concrete — "Medium keeps 7 of your 64 messages"
/// rather than "Medium ≈ 1,000 tokens". Sends nothing to any model: it runs the real selectors over
/// real history and counts what comes back, so a preview cannot drift from what a turn would do.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ContextPreview {
    /// The whole chat, for the caption.
    pub session_messages: usize,
    /// What selection would choose from — the baseline, same definition as everywhere else.
    pub pool: TierCounts,
    pub low: TierCounts,
    pub medium: TierCounts,
    pub high: TierCounts,
}

/// Run `strategy` over `session_id` at all three tiers.
///
/// Embeddings are the cost here, and they are paid once in practice rather than three times: every
/// tier embeds the same pool, and the vector store the caller passes in is the shared, cached one.
pub async fn preview(
    pool: &PgPool,
    session_id: &str,
    vector_store: &VectorStore,
    query: &str,
    tiers: &ContextTiers,
    strategy: ContextSelectionStrategy,
) -> ContextPreview {
    let baseline = SessionHistory::fetch_pool_unselected(
        session_id,
        pool,
        tiers.pool_size,
        &Policy::default(),
    )
    .await;

    let mut out = ContextPreview {
        session_messages: session_message_count(pool, session_id).await,
        pool: TierCounts {
            messages: baseline.report.pool_messages,
            tokens: baseline.report.pool_tokens,
        },
        ..Default::default()
    };

    for level in [
        PacmsBudgetLevel::Low,
        PacmsBudgetLevel::Medium,
        PacmsBudgetLevel::High,
    ] {
        let cfg = tiers.resolve(level);
        let history =
            SessionHistory::fetch_context(strategy, session_id, pool, vector_store, query, &cfg)
                .await;
        let counts = TierCounts {
            messages: history.report.kept_messages,
            tokens: history.report.kept_tokens,
        };
        match level {
            PacmsBudgetLevel::Low => out.low = counts,
            PacmsBudgetLevel::Medium => out.medium = counts,
            PacmsBudgetLevel::High => out.high = counts,
        }
    }
    out
}

/// The whole chat's length, not the pool's — the caption says "of your 64 messages", and a user
/// whose chat is longer than the pool should see that.
async fn session_message_count(pool: &PgPool, session_id: &str) -> usize {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
        .bind(session_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
        .max(0) as usize
}

/// Whether `user_id` has history selection switched on at all (CX-5).
///
/// Off means every request carries recent history as read, with no budget applied. Defaults to on,
/// so a deployment that predates the column behaves exactly as it did.
pub async fn selection_enabled(pool: &PgPool, user_id: Uuid) -> bool {
    selection_enabled_checked(pool, user_id)
        .await
        .unwrap_or(true)
}

/// Same check, but a real database error is reported rather than swallowed into the default
/// (CX-4) — for the `GET /api/me/context-strategy` read path. See
/// [`PacmsBudgetLevel::for_user_checked`] for the missing-row-vs-error distinction.
pub async fn selection_enabled_checked(pool: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT context_selection_enabled FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(true),
    )
}

// ── The one entry point ───────────────────────────────────────────────────────

/// Whether one agent has opted into compression. A missing or soft-deleted agent, or a failed
/// read, is `false` — history then goes out verbatim, the safe direction.
///
/// The `deleted_at IS NULL` guard is the reason callers use this rather than reading
/// `agents.compress_enabled` inline: a soft-deleted agent otherwise reports its stale flag.
pub async fn agent_compression_enabled(pool: &PgPool, agent_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT compress_enabled FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// Fetch the conversation history for `query` the way `user_id` has asked for it: look up their
/// strategy and budget tier, resolve the tier against this deployment's [`ContextTiers`], and
/// dispatch into the matching `SessionHistory::fetch_*`.
///
/// The preference lookups are cheap indexed row reads, issued concurrently. None can fail the
/// request — each falls back to its column default (see `for_user` on each).
///
/// # `compress_history` — whose switch gates IP-4
///
/// The opt-in of whoever actually *reads* the history, resolved by the caller rather than here:
///
/// * **A direct chat with one agent** — that agent's own `compress_enabled`, via
///   [`agent_compression_enabled`]. The history is stitched into the message forwarded to it, so
///   it is the reader.
/// * **An orchestrator-routed chat** — `true`. The history goes into the Orchestrator's own
///   prompt; agents downstream receive whatever that model then writes, not this text. No agent is
///   the reader, so no agent's switch applies and the deployment flag alone decides.
///
/// Resolved by the caller because the caller usually has the answer already — `a2a_dispatch` holds
/// the target agent row, so re-reading `compress_enabled` here would be a second query for a value
/// in hand. An earlier design passed a `CompressionScope` enum and resolved it internally, which
/// also made orchestrator-routed chats depend on a unanimity rule across *every* agent the user
/// owned; `162d50bd9` retired that rule in the UI, and the backend follows it here.
pub async fn fetch_for_user(
    pool: &PgPool,
    user_id: Uuid,
    session_id: &str,
    vector_store: &VectorStore,
    query: &str,
    tiers: &ContextTiers,
    compress_history: bool,
) -> SessionHistory {
    let (level, strategy, enabled) = tokio::join!(
        PacmsBudgetLevel::for_user(pool, user_id),
        ContextSelectionStrategy::for_user(pool, user_id),
        selection_enabled(pool, user_id),
    );
    // IP-4 is gated by the deployment flag AND the reader's switch, so the UI toggle stops it
    // as part of one layer rather than leaving history compressed after the rest is off.
    let mut cfg = tiers.resolve(level);
    cfg.compress.enabled = cfg.compress.enabled && compress_history;

    // Selection off: the pool goes out as read, so this user's requests carry the whole recent
    // window. The report then has pool == kept, which is the honest answer — the page should show
    // them saving nothing, because they are.
    let mut history = if enabled {
        SessionHistory::fetch_context(strategy, session_id, pool, vector_store, query, &cfg).await
    } else {
        SessionHistory::fetch_pool_unselected(session_id, pool, cfg.pool_size, &cfg.compress).await
    };
    // Stamped here, not read from `users` at query time: the chart asks what each request actually
    // carried, and a user who switches strategy must not rewrite their own history.
    history.report.strategy = enabled.then(|| strategy.as_label());
    history.report.level = Some(level.as_label());
    history
}

/// Who a [`ContextReport`] belongs to. A struct rather than five positional parameters, so a
/// transposed `trace_id`/`span_id` pair cannot compile.
#[derive(Debug, Clone, Default)]
pub struct ReportAttribution<'a> {
    pub user_id: Uuid,
    /// The agent the request was finally dispatched to. An orchestrator-routed chat selects history
    /// *before* routing, so callers on that path record after the decision rather than leaving this
    /// `None` — a NULL here drops the row out of the per-agent breakdown.
    pub agent_id: Option<Uuid>,
    /// A2A contextId — the chat.
    pub session_id: Option<&'a str>,
    /// W3C trace id of the request span, and that span's own id.
    pub trace_id: Option<&'a str>,
    pub span_id: Option<&'a str>,
}

/// Persist one request's selection report, and the two ledger rows derived from it.
///
/// Best-effort, like the `token_usage` write it rides beside: accounting is downstream of the
/// product working, never a precondition for it. A failed write is a warning, not a 500 on a turn
/// that already succeeded.
///
/// The two `token_savings` rows exist so `/finops/savings` stops reporting PACMS as unmeasured.
/// They are split — what compression removed, then what selection dropped from what was left — so
/// the two layers sum to `pool - kept` and neither counts the other's bytes.
pub async fn record_context_report(
    pool: &PgPool,
    attr: &ReportAttribution<'_>,
    report: &ContextReport,
) {
    if report.pool_messages == 0 {
        return; // nothing was read, so nothing was chosen from
    }

    let result = sqlx::query(
        "INSERT INTO context_reports \
             (user_id, agent_id, session_id, trace_id, span_id, strategy, level, budget, \
              fallback, pool_messages, pool_tokens, kept_messages, kept_tokens, \
              compressed_bytes_saved) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(attr.user_id)
    .bind(attr.agent_id)
    .bind(attr.session_id)
    .bind(attr.trace_id)
    .bind(attr.span_id)
    .bind(report.strategy)
    .bind(report.level)
    .bind(report.budget.map(|b| b as i32))
    .bind(report.fallback)
    .bind(report.pool_messages as i32)
    .bind(report.pool_tokens as i64)
    .bind(report.kept_messages as i32)
    .bind(report.kept_tokens as i64)
    .bind(report.compressed_bytes_saved as i64)
    .execute(pool)
    .await;

    if let Err(e) = result {
        tracing::warn!(error = %e, "context_reports write failed (swallowed)");
    }

    record_derived_savings(pool, attr, report).await;
}

/// Split one report's saving between the two layers that produced it.
///
/// `(compressed, dropped)`. Compression ran first, over the whole pool; selection then dropped part
/// of what compression left. Attributing the same bytes to both would double the page's total, so
/// selection is credited only with what survived compression:
///
/// ```text
/// compressed + dropped == pool_tokens - kept_tokens
/// ```
fn split_saved_tokens(report: &ContextReport) -> (usize, usize) {
    let compressed = crate::session_history::est_tokens(report.compressed_bytes_saved);
    // Saturating, not wrapping: an estimated byte-derived figure can round past the token figure it
    // is subtracted from on a tiny pool, and a negative saving there would be an artifact of the
    // divisor rather than a real one.
    let dropped = report
        .pool_tokens
        .saturating_sub(compressed)
        .saturating_sub(report.kept_tokens);
    (compressed, dropped)
}

/// The `compress_history` and `context_selection` halves of the same measurement.
async fn record_derived_savings(
    pool: &PgPool,
    attr: &ReportAttribution<'_>,
    report: &ContextReport,
) {
    let (compressed_tokens, dropped_tokens) = split_saved_tokens(report);

    let row = |layer: nasiko_savings::Layer, saved: i64| nasiko_savings::SavingsRow {
        user_id: attr.user_id,
        agent_id: attr.agent_id,
        flow_id: attr.trace_id.map(str::to_owned),
        session_id: attr.session_id.map(str::to_owned),
        // No model call has happened yet at this seam, so there is no provider, no model and no
        // rate to price against. The Optimization page prices these at read time from the
        // request's first LLM call; this ledger carries the token figure only.
        provider: None,
        model: None,
        layer,
        bytes_before: None,
        bytes_after: None,
        saved_input_tokens: saved,
        saved_output_tokens: 0,
        saved_cost_usd: 0.0,
        method: nasiko_savings::Method::MeasuredBytes,
        token_estimated: true,
        context_tier: report.level.map(str::to_owned),
    };

    if compressed_tokens > 0 {
        nasiko_savings::insert(
            pool,
            &row(
                nasiko_savings::Layer::CompressHistory,
                compressed_tokens as i64,
            ),
        )
        .await;
    }
    if dropped_tokens > 0 {
        nasiko_savings::insert(
            pool,
            &row(
                nasiko_savings::Layer::ContextSelection,
                dropped_tokens as i64,
            ),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(pool: usize, kept: usize, compressed_bytes: usize) -> ContextReport {
        ContextReport {
            pool_tokens: pool,
            kept_tokens: kept,
            compressed_bytes_saved: compressed_bytes,
            ..Default::default()
        }
    }

    /// The whole reason the saving is split rather than written as one row.
    #[test]
    fn the_two_layers_sum_to_what_the_pool_lost() {
        let r = report(1_000, 250, 400); // 400 bytes ≈ 100 tokens
        let (compressed, dropped) = split_saved_tokens(&r);
        assert_eq!(compressed, 100);
        assert_eq!(dropped, 650);
        assert_eq!(compressed + dropped, r.pool_tokens - r.kept_tokens);
    }

    /// `lastk` reads uncompressed, so the whole delta belongs to selection.
    #[test]
    fn with_no_compression_selection_is_credited_with_all_of_it() {
        let (compressed, dropped) = split_saved_tokens(&report(900, 300, 0));
        assert_eq!(compressed, 0);
        assert_eq!(dropped, 600);
    }

    /// A turn that kept everything it pooled saved nothing, and must write no row at all.
    #[test]
    fn keeping_the_whole_pool_saves_nothing() {
        let (compressed, dropped) = split_saved_tokens(&report(500, 500, 0));
        assert_eq!((compressed, dropped), (0, 0));
    }

    /// The byte-derived half is an estimate, so it can round past the token-derived half on a tiny
    /// pool. That must read as zero, never as a negative saving.
    #[test]
    fn an_estimate_that_overshoots_the_pool_does_not_go_negative() {
        let (_, dropped) = split_saved_tokens(&report(10, 8, 4_000));
        assert_eq!(dropped, 0);
    }

    #[test]
    fn labels_are_the_vocabulary_the_ledger_and_the_api_share() {
        assert_eq!(ContextSelectionStrategy::Pacms.as_label(), "pacms");
        assert_eq!(ContextSelectionStrategy::TopK.as_label(), "topk");
        assert_eq!(ContextSelectionStrategy::LastK.as_label(), "lastk");
        assert_eq!(PacmsBudgetLevel::Medium.as_label(), "medium");
    }

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
