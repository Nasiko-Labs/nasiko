//! `GET /api/observability/finops/savings` — what each optimization layer saved, and what it was
//! worth, over the same window and filters as the FinOps dashboard beside it.
//!
//! # Two kinds of number, one total
//!
//! Compression (IP-1/IP-3/IP-4) and context budgeting are **measured**: the code held both the
//! original and the reduced form, so each `token_savings` row is a subtraction. The brevity
//! directive and the minimal-code ladder are not measurable — nobody can know what the model would
//! have written without them — so each gets one percentage from `optimization_effect_factors`
//! applied to a *counted* volume of eligible traffic. The assumption is always the rate, never the
//! denominator, which is why a seeded category still responds correctly to the feature being
//! switched on or off.
//!
//! Every figure carries `basis` (`measured` | `fixture` | `seed_default` | `mixed`) so a consumer
//! can tell the two apart from the response alone.
//!
//! # Why this reads `token_usage` and not `trace_usage`
//!
//! The dashboard aggregates `trace_usage`, materialized from Tempo spans. Savings rows are written
//! per LLM call beside `token_usage` rows and share their dimensions exactly. Mixing the two would
//! put a numerator and a denominator from different populations into one percentage. So this
//! endpoint is internally consistent against `token_usage`, and its spend totals can differ
//! slightly from the dashboard's — the percentages are the point here, and they are coherent.

use std::collections::HashMap;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, QueryBuilder};
use utoipa::ToSchema;

/// Percentage change, rounded to 2dp. `None` when the baseline is zero — an undefined percentage,
/// not a fabricated 0. Mirrors `KpiValue::change_pct`'s rule in `service.rs`.
fn pct(part: f64, whole: f64) -> Option<f64> {
    if whole == 0.0 {
        return None;
    }
    Some((part / whole * 100.0 * 100.0).round() / 100.0)
}

/// The five fields every savings object carries, at every level of the response.
///
/// One shape for the total, each program, each layer, each agent and each session, so a consumer
/// renders them all the same way and the two percentages cannot be defined differently in two
/// places. Both are computed here rather than client-side for exactly that reason.
#[derive(Debug, Default, Clone, Serialize, ToSchema)]
pub struct Savings {
    pub saved_tokens: i64,
    pub saved_input_tokens: i64,
    pub saved_output_tokens: i64,
    pub saved_cost_usd: f64,
    /// What was actually billed over the same scope.
    pub actual_tokens: i64,
    pub actual_cost_usd: f64,
    /// `actual + saved` — what it would have cost without the layer.
    pub baseline_tokens: i64,
    pub baseline_cost_usd: f64,
    /// `saved / baseline`. Expect this to exceed `cost_reduction_pct`: savings are overwhelmingly
    /// input tokens, and input is the cheap side.
    pub token_reduction_pct: Option<f64>,
    pub cost_reduction_pct: Option<f64>,
    /// `measured` | `fixture` | `seed_default` | `mixed`.
    pub basis: &'static str,
}

impl Savings {
    fn new(
        saved_input: i64,
        saved_output: i64,
        saved_cost: f64,
        actual_tokens: i64,
        actual_cost: f64,
        basis: &'static str,
    ) -> Self {
        let saved_tokens = saved_input + saved_output;
        let baseline_tokens = actual_tokens + saved_tokens;
        let baseline_cost = actual_cost + saved_cost;
        Self {
            saved_tokens,
            saved_input_tokens: saved_input,
            saved_output_tokens: saved_output,
            saved_cost_usd: saved_cost,
            actual_tokens,
            actual_cost_usd: actual_cost,
            baseline_tokens,
            baseline_cost_usd: baseline_cost,
            token_reduction_pct: pct(saved_tokens as f64, baseline_tokens as f64),
            cost_reduction_pct: pct(saved_cost, baseline_cost),
            basis,
        }
    }
}

/// Combine the bases of several child figures into the parent's.
///
/// A parent of one measured and one seeded child is `mixed`, not `measured` — rolling up to the
/// more confident of the two would quietly launder an assumption into a measurement.
fn combine_basis(parts: impl IntoIterator<Item = &'static str>) -> &'static str {
    let mut seen: Option<&'static str> = None;
    for b in parts {
        match seen {
            None => seen = Some(b),
            Some(existing) if existing == b => {}
            Some(_) => return "mixed",
        }
    }
    seen.unwrap_or("measured")
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LayerSavings {
    pub layer: String,
    #[serde(flatten)]
    pub savings: Savings,
    /// Present only on factor-derived layers: the assumption, and what it was applied to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub factor: Option<FactorDetail>,
}

/// The provenance of a factor-derived figure, served so a consumer can show how an assumed number
/// was arrived at without looking anything up.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FactorDetail {
    pub input_token_delta_pct: f64,
    pub output_token_delta_pct: f64,
    pub basis: String,
    pub measured_at: DateTime<Utc>,
    /// One sentence saying where the number came from. Served verbatim from the factor row.
    pub notes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<f64>,
    /// The counted traffic the percentage was applied to. Real even when the rate is assumed.
    pub eligible_input_tokens: i64,
    pub eligible_output_tokens: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProgramSavings {
    pub program: String,
    /// How this program is named to users.
    pub label: &'static str,
    #[serde(flatten)]
    pub savings: Savings,
    pub layers: Vec<LayerSavings>,
    /// Only on `pacms`: the budget tier is the variable that moves its number, and a `high`-tier
    /// user with a negative saving is a one-field fix.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub by_tier: Vec<TierSavings>,
    /// Why a program's figure is zero, when it is. An empty row that explains itself is actionable;
    /// one that does not reads as a broken feature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TierSavings {
    pub tier: String,
    pub saved_tokens: i64,
    pub saved_cost_usd: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentSavings {
    pub agent_id: String,
    pub agent_name: String,
    pub calls: i64,
    /// Prompt-side tokens before the optimizer ran — `actual + saved`.
    pub input_tokens_before: i64,
    pub input_tokens_after: i64,
    #[serde(flatten)]
    pub savings: Savings,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionSavings {
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub turn_count: i64,
    pub agent_names: Vec<String>,
    #[serde(flatten)]
    pub savings: Savings,
}

/// Context that makes a small percentage readable. Without it a 3% fleet reduction looks like a
/// weak feature when it is a strong feature enabled on four agents out of thirty-one.
#[derive(Debug, Serialize, ToSchema)]
pub struct Coverage {
    pub calls_in_window: i64,
    pub calls_with_any_layer_enabled: i64,
    pub agents_total: i64,
    pub agents_optimized: i64,
    pub agents_with_compress_enabled: i64,
    pub agents_with_minimal_code_enabled: i64,
    pub agents_with_prompt_comments: i64,
    /// Spend by agents that have at least one layer on, and by those that have none. Drives the
    /// "turn it on for X" prompt, which is the most actionable thing on the page.
    pub optimized_spend_usd: f64,
    pub unoptimized_spend_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_unoptimized: Option<TopUnoptimized>,
    /// Share of measured saved tokens whose chars-per-token was calibrated against real reported
    /// usage rather than the fallback divisor.
    pub calibrated_pct: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TopUnoptimized {
    pub agent_id: String,
    pub agent_name: String,
    pub spend_usd: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SavingsWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SavingsData {
    pub window: SavingsWindow,
    pub total: Savings,
    pub by_program: Vec<ProgramSavings>,
    pub by_agent: Vec<AgentSavings>,
    pub by_session: Vec<SessionSavings>,
    pub coverage: Coverage,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SavingsResponse {
    pub data: SavingsData,
    pub status_code: u16,
    pub message: String,
}

/// Resolved query scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Total,
    Agent,
    Session,
}

impl Scope {
    pub fn parse(raw: Option<&str>) -> Result<Self, Response> {
        match raw {
            None | Some("total") => Ok(Self::Total),
            Some("agent") => Ok(Self::Agent),
            Some("session") => Ok(Self::Session),
            Some(other) => Err((
                StatusCode::BAD_REQUEST,
                format!("invalid scope '{other}' (expected total|agent|session)"),
            )
                .into_response()),
        }
    }
}

/// Everything the query needs to scope itself, resolved by the handler.
pub struct SavingsQuery<'a> {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub scope: Scope,
    pub agent_id: Option<uuid::Uuid>,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub session_id: Option<&'a str>,
    /// Rows for the agent/session scopes.
    pub limit: i64,
    /// Restricts every aggregate to agents the caller may see. `None` means unrestricted (OSS
    /// superuser); an empty slice means the caller can see nothing and every figure is zero.
    pub accessible_agent_ids: Option<&'a [uuid::Uuid]>,
}

/// One row of the measured-savings aggregate.
#[derive(sqlx::FromRow)]
struct LayerRow {
    layer: String,
    program: String,
    saved_input: i64,
    saved_output: i64,
    saved_cost: f64,
    calibrated: i64,
    uncalibrated: i64,
}

/// One row of the actual-spend aggregate, over the same window and filters.
#[derive(sqlx::FromRow)]
struct SpendRow {
    tokens: i64,
    cost: f64,
    calls: i64,
}

#[derive(sqlx::FromRow)]
struct FactorRow {
    layer: String,
    input_token_delta_pct: f64,
    output_token_delta_pct: f64,
    basis: String,
    measured_at: DateTime<Utc>,
    notes: String,
    sample_count: Option<i32>,
    confidence_pct: Option<f64>,
}

/// Eligible traffic for each factor-derived layer: counted, never assumed.
#[derive(Debug, Default, Clone, Copy)]
struct Eligible {
    input_tokens: i64,
    output_tokens: i64,
    cost: f64,
}

/// Append the window and the shared dimension filters to a builder.
///
/// One function for both the savings and the spend aggregate, so the numerator and the denominator
/// of every percentage are scoped identically. Two hand-written `WHERE` clauses would be the
/// easiest possible way to publish an incoherent number.
fn push_filters<'a>(
    qb: &mut QueryBuilder<'a, Postgres>,
    q: &'a SavingsQuery<'a>,
    time_column: &str,
    agent_column: &str,
) {
    qb.push(" WHERE ")
        .push(time_column)
        .push(" >= ")
        .push_bind(q.start)
        .push(" AND ")
        .push(time_column)
        .push(" < ")
        .push_bind(q.end);

    if let Some(agent) = q.agent_id {
        qb.push(" AND ")
            .push(agent_column)
            .push(" = ")
            .push_bind(agent);
    }
    if let Some(provider) = q.provider {
        qb.push(" AND provider = ").push_bind(provider);
    }
    if let Some(model) = q.model {
        qb.push(" AND model = ").push_bind(model);
    }
    if let Some(ids) = q.accessible_agent_ids {
        // An empty allow-list must match nothing. `= ANY('{}')` already does, but spelling it out
        // keeps the intent visible: a caller who can see no agents sees no savings, not all of them.
        qb.push(" AND ")
            .push(agent_column)
            .push(" = ANY(")
            .push_bind(ids)
            .push(")");
    }
}

/// Measured savings grouped by layer.
async fn measured_by_layer(
    db: &PgPool,
    q: &SavingsQuery<'_>,
) -> Result<Vec<LayerRow>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT layer, program,
                COALESCE(SUM(saved_input_tokens), 0)::BIGINT  AS saved_input,
                COALESCE(SUM(saved_output_tokens), 0)::BIGINT AS saved_output,
                COALESCE(SUM(saved_cost_usd), 0)::FLOAT8      AS saved_cost,
                COALESCE(SUM(CASE WHEN NOT token_estimated THEN saved_input_tokens ELSE 0 END), 0)::BIGINT AS calibrated,
                COALESCE(SUM(CASE WHEN token_estimated     THEN saved_input_tokens ELSE 0 END), 0)::BIGINT AS uncalibrated
         FROM token_savings",
    );
    push_filters(&mut qb, q, "created_at", "agent_id");
    if let Some(session) = q.session_id {
        qb.push(" AND session_id = ").push_bind(session);
    }
    qb.push(" GROUP BY layer, program");
    qb.build_query_as().fetch_all(db).await
}

/// Actual billed spend over the same scope — the denominator of every percentage.
async fn actual_spend(db: &PgPool, q: &SavingsQuery<'_>) -> Result<SpendRow, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT COALESCE(SUM(total_tokens), 0)::BIGINT AS tokens,
                COALESCE(SUM(cost_usd), 0)::FLOAT8     AS cost,
                COUNT(*)::BIGINT                       AS calls
         FROM token_usage",
    );
    push_filters(&mut qb, q, "created_at", "agent_id");
    qb.build_query_as().fetch_one(db).await
}

/// Traffic each factor-derived layer was eligible to act on.
///
/// Read from `token_usage.metadata`, where each layer already records what it decided per call —
/// `brevity.applied` is written on every call precisely so "it ran" and "it was skipped, and why"
/// are both answerable after the fact.
async fn eligible_for_factors(
    db: &PgPool,
    q: &SavingsQuery<'_>,
) -> Result<HashMap<String, Eligible>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT 'brevity' AS layer,
                COALESCE(SUM(input_tokens), 0)::BIGINT  AS input_tokens,
                COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
                COALESCE(SUM(cost_usd), 0)::FLOAT8      AS cost
         FROM token_usage",
    );
    push_filters(&mut qb, q, "created_at", "agent_id");
    qb.push(" AND metadata->'brevity'->>'applied' = 'true'");

    #[derive(sqlx::FromRow)]
    struct Row {
        layer: String,
        input_tokens: i64,
        output_tokens: i64,
        cost: f64,
    }

    let rows: Vec<Row> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.layer,
                Eligible {
                    input_tokens: r.input_tokens,
                    output_tokens: r.output_tokens,
                    cost: r.cost,
                },
            )
        })
        .collect())
}

/// Traffic eligible for the minimal-code ladder: turns dispatched to an agent that has the flag on
/// **and** whose card reads as code work.
///
/// The same conjunction `a2a_dispatch.rs` gates the injection on, so the denominator matches what
/// actually happened rather than what was configured.
async fn eligible_for_minimal_code(
    db: &PgPool,
    q: &SavingsQuery<'_>,
) -> Result<Eligible, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT COALESCE(SUM(tu.input_tokens), 0)::BIGINT  AS input_tokens,
                COALESCE(SUM(tu.output_tokens), 0)::BIGINT AS output_tokens,
                COALESCE(SUM(tu.cost_usd), 0)::FLOAT8      AS cost
         FROM token_usage tu
         JOIN agents a ON a.id = tu.agent_id AND a.minimal_code_enabled",
    );
    push_filters(&mut qb, q, "tu.created_at", "tu.agent_id");

    #[derive(sqlx::FromRow)]
    struct Row {
        input_tokens: i64,
        output_tokens: i64,
        cost: f64,
    }
    let row: Row = qb.build_query_as().fetch_one(db).await?;
    Ok(Eligible {
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cost: row.cost,
    })
}

/// Traffic eligible for prompt-comment stripping.
///
/// The predicate is `= 'enabled'`, not `= 'true'`. The agent reads this value as an env var and
/// only the literal `enabled` switches the feature on (`oss/agents/coding/src/instructions.rs`);
/// a `true` written by some other path reads as **disabled** there. Accepting it here would count
/// agents whose feature is off into the denominator and silently deflate the figure.
async fn eligible_for_prompt_comments(
    db: &PgPool,
    q: &SavingsQuery<'_>,
) -> Result<Eligible, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT COALESCE(SUM(tu.input_tokens), 0)::BIGINT  AS input_tokens,
                COALESCE(SUM(tu.output_tokens), 0)::BIGINT AS output_tokens,
                COALESCE(SUM(tu.cost_usd), 0)::FLOAT8      AS cost
         FROM token_usage tu
         JOIN agents a ON a.id = tu.agent_id
              AND COALESCE(a.metadata->'features'->>'prompt_comments', '') = 'enabled'",
    );
    push_filters(&mut qb, q, "tu.created_at", "tu.agent_id");

    #[derive(sqlx::FromRow)]
    struct Row {
        input_tokens: i64,
        output_tokens: i64,
        cost: f64,
    }
    let row: Row = qb.build_query_as().fetch_one(db).await?;
    Ok(Eligible {
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cost: row.cost,
    })
}

async fn factors(db: &PgPool) -> Result<HashMap<String, FactorRow>, sqlx::Error> {
    let rows: Vec<FactorRow> = sqlx::query_as(
        "SELECT layer, input_token_delta_pct, output_token_delta_pct, basis,
                measured_at, notes, sample_count, confidence_pct
         FROM optimization_effect_factors",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.layer.clone(), r)).collect())
}

/// Apply one factor to its eligible traffic.
///
/// A delta of `-35` means "the model wrote 35% fewer output tokens than it otherwise would have",
/// so what was *saved* is relative to the unoptimized baseline, not to what was billed:
/// `billed = baseline * (1 - 0.35)`, hence `saved = billed * 0.35 / 0.65`. Treating the percentage
/// as a fraction of the billed figure would under-report it, and inconsistently with how the
/// holdout will later measure the same quantity.
fn apply_factor(delta_pct: f64, billed: f64) -> f64 {
    let reduction = -delta_pct / 100.0;
    if reduction <= 0.0 || reduction >= 1.0 {
        return 0.0;
    }
    billed * reduction / (1.0 - reduction)
}

fn layer_savings_from_factor(
    layer: &str,
    factor: &FactorRow,
    eligible: Eligible,
    actual_tokens: i64,
    actual_cost: f64,
) -> LayerSavings {
    let saved_input = apply_factor(factor.input_token_delta_pct, eligible.input_tokens as f64);
    let saved_output = apply_factor(factor.output_token_delta_pct, eligible.output_tokens as f64);
    let saved_cost = apply_factor(
        // Cost follows whichever side the layer acts on; when it acts on both, the blended delta is
        // their average weighted by this window's own token split rather than a fixed guess.
        blended_delta(factor, eligible),
        eligible.cost,
    );
    let basis: &'static str = if factor.basis == "fixture" {
        "fixture"
    } else {
        "seed_default"
    };

    LayerSavings {
        layer: layer.to_string(),
        savings: Savings::new(
            saved_input.round() as i64,
            saved_output.round() as i64,
            saved_cost,
            actual_tokens,
            actual_cost,
            basis,
        ),
        factor: Some(FactorDetail {
            input_token_delta_pct: factor.input_token_delta_pct,
            output_token_delta_pct: factor.output_token_delta_pct,
            basis: factor.basis.clone(),
            measured_at: factor.measured_at,
            notes: factor.notes.clone(),
            sample_count: factor.sample_count,
            confidence_pct: factor.confidence_pct,
            eligible_input_tokens: eligible.input_tokens,
            eligible_output_tokens: eligible.output_tokens,
        }),
    }
}

/// Weight the input and output deltas by this window's actual token split.
fn blended_delta(factor: &FactorRow, eligible: Eligible) -> f64 {
    let total = (eligible.input_tokens + eligible.output_tokens) as f64;
    if total == 0.0 {
        return 0.0;
    }
    (factor.input_token_delta_pct * eligible.input_tokens as f64
        + factor.output_token_delta_pct * eligible.output_tokens as f64)
        / total
}

/// How a program is named to users.
///
/// Named for what gets shorter, not for the mechanism. A spend dashboard is read by people who do
/// not know what a payload, a ladder or a context budget is, and who should not have to.
fn program_label(program: &str) -> &'static str {
    match program {
        "caveman" => "Smaller prompts",
        // Named for the budget, not the selector: every strategy fills the same budget, so the
        // saving belongs to the tier rather than to the selection algorithm.
        "pacms" => "Shorter chat history",
        "ponytail" => "Less code written",
        "prompt_comments" => "Leaner agent instructions",
        _ => "Other",
    }
}

/// Per-agent savings and spend, for `scope=agent` and for the optimisation panel.
async fn by_agent(db: &PgPool, q: &SavingsQuery<'_>) -> Result<Vec<AgentSavings>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        agent_id: uuid::Uuid,
        agent_name: String,
        calls: i64,
        input_tokens: i64,
        tokens: i64,
        cost: f64,
        saved_input: i64,
        saved_output: i64,
        saved_cost: f64,
    }

    let mut qb = QueryBuilder::new(
        "WITH spend AS (SELECT agent_id, COUNT(*)::BIGINT AS calls, COALESCE(SUM(input_tokens),0)::BIGINT AS input_tokens, COALESCE(SUM(total_tokens),0)::BIGINT AS tokens, COALESCE(SUM(cost_usd),0)::FLOAT8 AS cost FROM token_usage",
    );
    push_filters(&mut qb, q, "created_at", "agent_id");
    qb.push(" AND agent_id IS NOT NULL GROUP BY agent_id), sav AS (SELECT agent_id, COALESCE(SUM(saved_input_tokens),0)::BIGINT AS saved_input, COALESCE(SUM(saved_output_tokens),0)::BIGINT AS saved_output, COALESCE(SUM(saved_cost_usd),0)::FLOAT8 AS saved_cost FROM token_savings");
    push_filters(&mut qb, q, "created_at", "agent_id");
    qb.push(
        " AND agent_id IS NOT NULL GROUP BY agent_id)
         SELECT s.agent_id,
                COALESCE(a.display_name, a.name) AS agent_name,
                s.calls, s.input_tokens, s.tokens, s.cost,
                COALESCE(v.saved_input, 0)  AS saved_input,
                COALESCE(v.saved_output, 0) AS saved_output,
                COALESCE(v.saved_cost, 0)   AS saved_cost
         FROM spend s
         JOIN agents a ON a.id = s.agent_id
         LEFT JOIN sav v ON v.agent_id = s.agent_id
         ORDER BY COALESCE(v.saved_cost, 0) DESC, s.cost DESC
         LIMIT ",
    )
    .push_bind(q.limit);

    let rows: Vec<Row> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| AgentSavings {
            agent_id: r.agent_id.to_string(),
            agent_name: r.agent_name,
            calls: r.calls,
            // "Before" is what the model would have read: what it did read, plus what we removed.
            input_tokens_before: r.input_tokens + r.saved_input,
            input_tokens_after: r.input_tokens,
            savings: Savings::new(
                r.saved_input,
                r.saved_output,
                r.saved_cost,
                r.tokens,
                r.cost,
                "measured",
            ),
        })
        .collect())
}

/// Per-session savings.
///
/// `token_savings.session_id` is NULL on rows written by the llm-router, which holds the flow id
/// rather than the A2A contextId. Rather than spend a lookup per call on the hot path, the session
/// is resolved here by joining `flow_id` to the trace that carries it.
async fn by_session(db: &PgPool, q: &SavingsQuery<'_>) -> Result<Vec<SessionSavings>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        session_id: String,
        started_at: DateTime<Utc>,
        turn_count: i64,
        agent_names: Vec<String>,
        saved_input: i64,
        saved_output: i64,
        saved_cost: f64,
        tokens: i64,
        cost: f64,
    }

    let mut qb = QueryBuilder::new(
        "WITH sav AS (
           SELECT COALESCE(ts.session_id, tr.session_id) AS session_id,
                  MIN(ts.created_at) AS started_at,
                  COUNT(DISTINCT ts.flow_id)::BIGINT AS turn_count,
                  COALESCE(SUM(ts.saved_input_tokens),0)::BIGINT  AS saved_input,
                  COALESCE(SUM(ts.saved_output_tokens),0)::BIGINT AS saved_output,
                  COALESCE(SUM(ts.saved_cost_usd),0)::FLOAT8      AS saved_cost,
                  COALESCE(ARRAY_AGG(DISTINCT tr.agent_name) FILTER (WHERE tr.agent_name IS NOT NULL), '{}') AS agent_names
           FROM token_savings ts
           LEFT JOIN trace_usage tr ON tr.trace_id = ts.flow_id",
    );
    push_filters(&mut qb, q, "ts.created_at", "ts.agent_id");
    if let Some(session) = q.session_id {
        qb.push(" AND COALESCE(ts.session_id, tr.session_id) = ")
            .push_bind(session);
    }
    qb.push(
        " GROUP BY 1 HAVING COALESCE(ts.session_id, tr.session_id) IS NOT NULL),
         spend AS (
           SELECT tu.session_id AS flow_id,
                  COALESCE(SUM(tu.total_tokens),0)::BIGINT AS tokens,
                  COALESCE(SUM(tu.cost_usd),0)::FLOAT8     AS cost
           FROM token_usage tu",
    );
    push_filters(&mut qb, q, "tu.created_at", "tu.agent_id");
    qb.push(
        " GROUP BY 1)
         SELECT sav.session_id, sav.started_at, sav.turn_count, sav.agent_names,
                sav.saved_input, sav.saved_output, sav.saved_cost,
                COALESCE(SUM(spend.tokens), 0)::BIGINT AS tokens,
                COALESCE(SUM(spend.cost), 0)::FLOAT8   AS cost
         FROM sav
         LEFT JOIN trace_usage tr2 ON tr2.session_id = sav.session_id
         LEFT JOIN spend ON spend.flow_id = tr2.trace_id
         GROUP BY sav.session_id, sav.started_at, sav.turn_count, sav.agent_names,
                  sav.saved_input, sav.saved_output, sav.saved_cost
         ORDER BY sav.saved_cost DESC
         LIMIT ",
    )
    .push_bind(q.limit);

    let rows: Vec<Row> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| SessionSavings {
            session_id: r.session_id,
            started_at: r.started_at,
            turn_count: r.turn_count,
            agent_names: r.agent_names,
            savings: Savings::new(
                r.saved_input,
                r.saved_output,
                r.saved_cost,
                r.tokens,
                r.cost,
                "measured",
            ),
        })
        .collect())
}

/// Fleet coverage: how much of the estate has any layer on, and what the rest is spending.
async fn coverage(
    db: &PgPool,
    q: &SavingsQuery<'_>,
    calls_in_window: i64,
    calibrated: i64,
    uncalibrated: i64,
) -> Result<Coverage, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Flags {
        agents_total: i64,
        compress: i64,
        minimal: i64,
        comments: i64,
        optimized: i64,
    }

    let flags: Flags = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT AS agents_total,
                COUNT(*) FILTER (WHERE compress_enabled)::BIGINT AS compress,
                COUNT(*) FILTER (WHERE minimal_code_enabled)::BIGINT AS minimal,
                COUNT(*) FILTER (WHERE COALESCE(metadata->'features'->>'prompt_comments','') = 'enabled')::BIGINT AS comments,
                COUNT(*) FILTER (WHERE compress_enabled OR minimal_code_enabled
                       OR COALESCE(metadata->'features'->>'prompt_comments','') = 'enabled')::BIGINT AS optimized
         FROM agents WHERE deleted_at IS NULL",
    )
    .fetch_one(db)
    .await?;

    #[derive(sqlx::FromRow)]
    struct SpendSplit {
        optimized_spend: f64,
        unoptimized_spend: f64,
        enabled_calls: i64,
    }

    let mut qb = QueryBuilder::new(
        "SELECT COALESCE(SUM(tu.cost_usd) FILTER (WHERE opt.on), 0)::FLOAT8     AS optimized_spend,
                COALESCE(SUM(tu.cost_usd) FILTER (WHERE NOT opt.on), 0)::FLOAT8 AS unoptimized_spend,
                COUNT(*) FILTER (WHERE opt.on)::BIGINT                          AS enabled_calls
         FROM token_usage tu
         JOIN LATERAL (
           SELECT (a.compress_enabled OR a.minimal_code_enabled
                   OR COALESCE(a.metadata->'features'->>'prompt_comments','') = 'enabled') AS on
           FROM agents a WHERE a.id = tu.agent_id
         ) opt ON true",
    );
    push_filters(&mut qb, q, "tu.created_at", "tu.agent_id");
    let split: SpendSplit = qb.build_query_as().fetch_one(db).await?;

    #[derive(sqlx::FromRow)]
    struct TopRow {
        agent_id: uuid::Uuid,
        agent_name: String,
        spend_usd: f64,
    }

    let mut qb = QueryBuilder::new(
        "SELECT tu.agent_id, COALESCE(a.display_name, a.name) AS agent_name,
                COALESCE(SUM(tu.cost_usd), 0)::FLOAT8 AS spend_usd
         FROM token_usage tu
         JOIN agents a ON a.id = tu.agent_id
              AND NOT a.compress_enabled AND NOT a.minimal_code_enabled
              AND COALESCE(a.metadata->'features'->>'prompt_comments','') <> 'enabled'",
    );
    push_filters(&mut qb, q, "tu.created_at", "tu.agent_id");
    qb.push(" GROUP BY tu.agent_id, agent_name ORDER BY spend_usd DESC LIMIT 1");
    let top: Option<TopRow> = qb.build_query_as().fetch_optional(db).await?;

    Ok(Coverage {
        calls_in_window,
        calls_with_any_layer_enabled: split.enabled_calls,
        agents_total: flags.agents_total,
        agents_optimized: flags.optimized,
        agents_with_compress_enabled: flags.compress,
        agents_with_minimal_code_enabled: flags.minimal,
        agents_with_prompt_comments: flags.comments,
        optimized_spend_usd: split.optimized_spend,
        unoptimized_spend_usd: split.unoptimized_spend,
        top_unoptimized: top.map(|t| TopUnoptimized {
            agent_id: t.agent_id.to_string(),
            agent_name: t.agent_name,
            spend_usd: t.spend_usd,
        }),
        calibrated_pct: pct(calibrated as f64, (calibrated + uncalibrated) as f64),
    })
}

/// Assemble the whole response.
pub async fn get_savings(db: &PgPool, q: &SavingsQuery<'_>) -> Result<SavingsData, sqlx::Error> {
    let measured = measured_by_layer(db, q).await?;
    let spend = actual_spend(db, q).await?;

    let calibrated: i64 = measured.iter().map(|r| r.calibrated).sum();
    let uncalibrated: i64 = measured.iter().map(|r| r.uncalibrated).sum();

    // Measured layers, grouped into their programs.
    let mut programs: HashMap<String, Vec<LayerSavings>> = HashMap::new();
    for row in &measured {
        programs
            .entry(row.program.clone())
            .or_default()
            .push(LayerSavings {
                layer: row.layer.clone(),
                savings: Savings::new(
                    row.saved_input,
                    row.saved_output,
                    row.saved_cost,
                    spend.tokens,
                    spend.cost,
                    "measured",
                ),
                factor: None,
            });
    }

    // Factor-derived layers. Each is emitted even at zero, with the reason, because an absent row
    // and a real zero are indistinguishable to a consumer — and the difference between "nobody
    // enabled it" and "it did nothing" is the whole point of the coverage block.
    let factors = factors(db).await?;
    let brevity_eligible = eligible_for_factors(db, q)
        .await?
        .get("brevity")
        .copied()
        .unwrap_or_default();
    let minimal_eligible = eligible_for_minimal_code(db, q).await?;
    let comments_eligible = eligible_for_prompt_comments(db, q).await?;

    for (layer, program, eligible) in [
        ("brevity", "caveman", brevity_eligible),
        ("minimal_code", "ponytail", minimal_eligible),
        ("prompt_comments", "prompt_comments", comments_eligible),
    ] {
        if let Some(f) = factors.get(layer) {
            programs
                .entry(program.to_string())
                .or_default()
                .push(layer_savings_from_factor(
                    layer,
                    f,
                    eligible,
                    spend.tokens,
                    spend.cost,
                ));
        }
    }

    let mut by_program: Vec<ProgramSavings> = programs
        .into_iter()
        .map(|(program, layers)| {
            let saved_input: i64 = layers.iter().map(|l| l.savings.saved_input_tokens).sum();
            let saved_output: i64 = layers.iter().map(|l| l.savings.saved_output_tokens).sum();
            let saved_cost: f64 = layers.iter().map(|l| l.savings.saved_cost_usd).sum();
            let basis = combine_basis(layers.iter().map(|l| l.savings.basis));
            let note = if saved_input == 0 && saved_output == 0 {
                Some(zero_note(&program, &layers).to_string())
            } else {
                partial_note(&program)
            };
            ProgramSavings {
                label: program_label(&program),
                savings: Savings::new(
                    saved_input,
                    saved_output,
                    saved_cost,
                    spend.tokens,
                    spend.cost,
                    basis,
                ),
                program,
                layers,
                by_tier: Vec::new(),
                note,
            }
        })
        .collect();
    by_program.sort_by(|a, b| {
        b.savings
            .saved_cost_usd
            .total_cmp(&a.savings.saved_cost_usd)
    });

    if let Some(p) = by_program.iter_mut().find(|p| p.program == "pacms") {
        p.by_tier = tier_breakdown(db, q).await?;
    }

    let total_saved_input: i64 = by_program
        .iter()
        .map(|p| p.savings.saved_input_tokens)
        .sum();
    let total_saved_output: i64 = by_program
        .iter()
        .map(|p| p.savings.saved_output_tokens)
        .sum();
    let total_saved_cost: f64 = by_program.iter().map(|p| p.savings.saved_cost_usd).sum();
    let total_basis = combine_basis(by_program.iter().map(|p| p.savings.basis));

    Ok(SavingsData {
        window: SavingsWindow {
            start: q.start,
            end: q.end,
        },
        total: Savings::new(
            total_saved_input,
            total_saved_output,
            total_saved_cost,
            spend.tokens,
            spend.cost,
            total_basis,
        ),
        // `total` carries both rollups so a dashboard panel showing agents *and* sessions needs one
        // request rather than two; the narrower scopes return only what they name, for a caller
        // that wants one of them cheaply.
        by_agent: match q.scope {
            Scope::Session => Vec::new(),
            _ => by_agent(db, q).await?,
        },
        by_session: match q.scope {
            Scope::Agent => Vec::new(),
            _ => by_session(db, q).await?,
        },
        by_program,
        coverage: coverage(db, q, spend.calls, calibrated, uncalibrated).await?,
    })
}

/// Layers that run in production but do not yet write to the ledger.
///
/// Their savings are real and happening; nothing records them. Listing them here keeps the one
/// thing a dashboard must never do — report "this saved nothing" when the truth is "this is not
/// measured" — out of the response. **Delete an entry the moment its emitter ships**, or the note
/// starts lying in the other direction.
const UNINSTRUMENTED_LAYERS: &[&str] = &[
    "compress_tool_result", // IP-3, oss/react-agent/src/context.rs
    "compress_history",     // IP-4, oss/orchestrator/src/session_history.rs
    "context_selection",    // the PACMS budget effect
    "prompt_comments",      // measured agent-side; no span attribute carries it out yet
];

/// Whether a program's whole contribution is still uninstrumented.
fn program_uninstrumented(program: &str) -> bool {
    match program {
        // Its only layer is the budget effect, which nothing emits yet.
        "pacms" => UNINSTRUMENTED_LAYERS.contains(&"context_selection"),
        "prompt_comments" => UNINSTRUMENTED_LAYERS.contains(&"prompt_comments"),
        _ => false,
    }
}

/// Why a program's figure is zero, or why it is incomplete.
///
/// A row that explains itself is actionable; one that does not reads as a broken feature, and one
/// that explains itself *wrongly* is worse than either.
fn zero_note(program: &str, layers: &[LayerSavings]) -> &'static str {
    if program_uninstrumented(program) {
        return "Running, but not measured yet — we do not record what this one saves.";
    }
    let no_eligible = layers.iter().all(|l| {
        l.factor
            .as_ref()
            .is_none_or(|f| f.eligible_input_tokens + f.eligible_output_tokens == 0)
    });
    match (program, no_eligible) {
        ("ponytail", true) => "No coding agent has this turned on.",
        ("prompt_comments", true) => "No agent has this turned on.",
        ("caveman", _) => "No agent had this turned on during this period.",
        _ => "No eligible traffic in this window.",
    }
}

/// Names the layers a program is not yet counting, when some of it *is* counted.
///
/// Caveman is the case this exists for: payload compression reports, while tool-result and history
/// compression do not, so the category total is real but low. Saying so is the difference between
/// an understated number and a wrong one.
fn partial_note(program: &str) -> Option<String> {
    let missing: Vec<&str> = match program {
        "caveman" => UNINSTRUMENTED_LAYERS
            .iter()
            .filter(|l| l.starts_with("compress_"))
            .copied()
            .collect(),
        _ => Vec::new(),
    };
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "We do not yet count {}, so the real saving is higher than this.",
        missing
            .iter()
            .map(|l| match *l {
                "compress_tool_result" => "repeated tool output",
                "compress_history" => "chat history",
                other => other,
            })
            .collect::<Vec<_>>()
            .join(" or ")
    ))
}

/// Context-budgeting savings split by the user's budget tier — the variable that moves the number.
async fn tier_breakdown(
    db: &PgPool,
    q: &SavingsQuery<'_>,
) -> Result<Vec<TierSavings>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        tier: String,
        saved_tokens: i64,
        saved_cost_usd: f64,
    }

    let mut qb = QueryBuilder::new(
        "SELECT context_tier AS tier,
                COALESCE(SUM(saved_input_tokens + saved_output_tokens), 0)::BIGINT AS saved_tokens,
                COALESCE(SUM(saved_cost_usd), 0)::FLOAT8 AS saved_cost_usd
         FROM token_savings",
    );
    push_filters(&mut qb, q, "created_at", "agent_id");
    qb.push(" AND context_tier IS NOT NULL GROUP BY context_tier ORDER BY saved_tokens DESC");

    let rows: Vec<Row> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| TierSavings {
            tier: r.tier,
            saved_tokens: r.saved_tokens,
            saved_cost_usd: r.saved_cost_usd,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factor(input: f64, output: f64, basis: &str) -> FactorRow {
        FactorRow {
            layer: "brevity".into(),
            input_token_delta_pct: input,
            output_token_delta_pct: output,
            basis: basis.into(),
            measured_at: Utc::now(),
            notes: "test".into(),
            sample_count: None,
            confidence_pct: None,
        }
    }

    // ── percentages ──────────────────────────────────────────────────────────

    #[test]
    fn a_zero_baseline_yields_no_percentage_rather_than_a_fabricated_zero() {
        assert_eq!(pct(0.0, 0.0), None);
        assert_eq!(pct(10.0, 0.0), None);
    }

    #[test]
    fn reduction_is_measured_against_the_baseline_not_the_billed_amount() {
        // 1000 billed, 250 saved → the unoptimized run would have been 1250, so the reduction is
        // 20%, not 25%. Dividing by the billed figure is the most natural way to overstate this.
        let s = Savings::new(250, 0, 0.0, 1000, 0.0, "measured");
        assert_eq!(s.baseline_tokens, 1250);
        assert_eq!(s.token_reduction_pct, Some(20.0));
    }

    #[test]
    fn token_and_cost_percentages_are_allowed_to_diverge() {
        // Savings are input-side and input is cheap, so the cost figure lags the token figure.
        // A test pins this because "they should match" is a plausible-sounding bug report.
        let s = Savings::new(1000, 0, 0.10, 1000, 9.90, "measured");
        assert_eq!(s.token_reduction_pct, Some(50.0));
        assert_eq!(s.cost_reduction_pct, Some(1.0));
    }

    // ── basis roll-up ────────────────────────────────────────────────────────

    #[test]
    fn a_parent_of_measured_and_seeded_children_is_mixed() {
        // Not "measured". Rolling up to the more confident child would launder an assumption into
        // a measurement, which is the one thing this field exists to prevent.
        assert_eq!(combine_basis(["measured", "seed_default"]), "mixed");
        assert_eq!(combine_basis(["measured", "measured"]), "measured");
        assert_eq!(
            combine_basis(["seed_default", "seed_default"]),
            "seed_default"
        );
        assert_eq!(combine_basis(["fixture", "seed_default"]), "mixed");
    }

    #[test]
    fn an_empty_roll_up_is_measured_because_nothing_was_assumed() {
        assert_eq!(combine_basis([]), "measured");
    }

    // ── factor application ───────────────────────────────────────────────────

    #[test]
    fn a_factor_is_applied_relative_to_the_unoptimized_baseline() {
        // -50% means the baseline was double what was billed, so 1000 billed implies 1000 saved.
        assert!((apply_factor(-50.0, 1000.0) - 1000.0).abs() < 1e-9);
        // -35% on 650 billed implies a 1000 baseline, so 350 saved.
        assert!((apply_factor(-35.0, 650.0) - 350.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_or_nonsensical_factor_saves_nothing() {
        assert_eq!(apply_factor(0.0, 1000.0), 0.0);
        // A positive delta means the layer made things worse; crediting it as a saving would be
        // backwards, and a >=100% reduction is not representable at all.
        assert_eq!(apply_factor(10.0, 1000.0), 0.0);
        assert_eq!(apply_factor(-100.0, 1000.0), 0.0);
        assert_eq!(apply_factor(-150.0, 1000.0), 0.0);
    }

    #[test]
    fn a_factor_over_no_eligible_traffic_saves_nothing() {
        // The denominator is counted, never assumed — which is what makes a seeded category still
        // respond correctly to the feature being off.
        assert_eq!(apply_factor(-35.0, 0.0), 0.0);
    }

    #[test]
    fn a_seeded_factor_is_reported_as_seeded_and_a_fixture_as_fixture() {
        let seeded = layer_savings_from_factor(
            "brevity",
            &factor(0.0, -35.0, "seed_default"),
            Eligible {
                input_tokens: 0,
                output_tokens: 650,
                cost: 1.0,
            },
            1000,
            10.0,
        );
        assert_eq!(seeded.savings.basis, "seed_default");
        assert_eq!(seeded.savings.saved_output_tokens, 350);
        assert_eq!(seeded.factor.as_ref().unwrap().eligible_output_tokens, 650);

        let measured = layer_savings_from_factor(
            "brevity",
            &factor(0.0, -35.0, "fixture"),
            Eligible {
                input_tokens: 0,
                output_tokens: 650,
                cost: 1.0,
            },
            1000,
            10.0,
        );
        assert_eq!(measured.savings.basis, "fixture");
    }

    #[test]
    fn the_blended_delta_follows_this_windows_own_token_split() {
        let f = factor(-30.0, -30.0, "seed_default");
        // Both sides equal: the blend is that value whatever the split.
        let e = Eligible {
            input_tokens: 700,
            output_tokens: 300,
            cost: 1.0,
        };
        assert!((blended_delta(&f, e) - -30.0).abs() < 1e-9);

        // Output-only layer over mostly-input traffic blends down, because most of the tokens it
        // was eligible for are ones it does not touch.
        let out_only = factor(0.0, -35.0, "seed_default");
        assert!((blended_delta(&out_only, e) - -10.5).abs() < 1e-9);
    }

    #[test]
    fn no_eligible_tokens_means_no_blended_delta_rather_than_a_divide_by_zero() {
        assert_eq!(
            blended_delta(&factor(-30.0, -30.0, "seed_default"), Eligible::default()),
            0.0
        );
    }

    // ── scope parsing ────────────────────────────────────────────────────────

    #[test]
    fn scope_defaults_to_total_and_rejects_anything_unknown() {
        assert_eq!(Scope::parse(None).ok(), Some(Scope::Total));
        assert_eq!(Scope::parse(Some("total")).ok(), Some(Scope::Total));
        assert_eq!(Scope::parse(Some("agent")).ok(), Some(Scope::Agent));
        assert_eq!(Scope::parse(Some("session")).ok(), Some(Scope::Session));
        assert!(Scope::parse(Some("everything")).is_err());
    }

    #[test]
    fn program_labels_name_the_budget_not_the_selector() {
        // "PACMS saves X" credits the algorithm; the saving is the budget tier's.
        assert_eq!(program_label("pacms"), "Shorter chat history");
        assert_eq!(program_label("caveman"), "Smaller prompts");
        assert_eq!(program_label("ponytail"), "Less code written");
    }
    // ── honesty of the zero/partial notes ────────────────────────────────────

    #[test]
    fn an_uninstrumented_program_says_so_instead_of_claiming_nothing_happened() {
        // The bug this guards: "No session exceeded its context budget" reads as a measurement
        // when the truth is that nothing measures it. A dashboard that mis-explains a zero is
        // worse than one that leaves it bare.
        let note = zero_note("pacms", &[]);
        assert!(note.contains("not measured yet"), "{note}");
        // The old wording claimed no session had exceeded its budget, which reads as a finding.
        assert!(!note.contains("exceeded"), "{note}");
    }

    #[test]
    fn an_instrumented_program_still_explains_a_real_zero() {
        assert!(zero_note("ponytail", &[]).contains("coding agent"));
        assert!(zero_note("caveman", &[]).contains("turned on"));
    }

    #[test]
    fn a_partially_counted_program_declares_itself_an_under_estimate() {
        // Caveman reports payload compression but not the tool-result or history layers, so its
        // total is real and low. Saying so is the difference between understated and wrong.
        let note = partial_note("caveman").expect("caveman is partial while IP-3/IP-4 are silent");
        assert!(note.contains("higher than this"), "{note}");
        // Named in plain words, not by the ledger's layer key.
        assert!(note.contains("chat history"), "{note}");
        assert!(
            !note.contains("compress_"),
            "no internal layer names in user copy: {note}"
        );
    }

    #[test]
    fn a_fully_counted_program_carries_no_partial_note() {
        assert_eq!(partial_note("ponytail"), None);
    }
}
