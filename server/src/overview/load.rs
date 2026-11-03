//! Every read the estate needs, as one [`Snapshot`] of plain rows.
//!
//! Nothing here decides anything: [`super::estate`] draws the map and [`super::rules`] finds the
//! problems from these rows alone, so both are tested without a database.
//!
//! Two populations, never mixed:
//! - `trace_usage` (OTel spans, materialized per trace and agent) is what an agent or a coding
//!   harness *did*: its runs, spend, models, latency and failed runs (`error_count`; NULL means the
//!   trace isn't materialized yet, so a rate counts only the `checked` rows).
//! - `token_usage` (metered by the platform itself) is what went through the LLM router and the
//!   MCP gateway: tool calls (`operation_type = 'mcp_tool_call'`) and the routed spend that
//!   payload compression applies to.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use nasiko_mcp_gateway::error::McpError;
use nasiko_mcp_gateway::permissions::toolkit_from_composio_slug;
use nasiko_mcp_gateway::repo::McpConnector;
use nasiko_mcp_gateway::types::connector_prefix;
use serde::Deserialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::{OverviewError, Range, Scope};
use crate::catalog::routes::{agent_access_predicate, live_status};
use crate::observability::savings::{PRICE_JOIN, PRICED_COST};
use crate::state::AppState;

/// The days the history slider and every trend cover, today included.
pub(super) const HISTORY_DAYS: i64 = 14;
/// A coding harness keeps this many of its latest sessions on the map.
const SESSIONS_PER_HARNESS: i64 = 30;
/// A person counts as working on an agent while their latest call to it is this recent.
const WORKING_ON_MINUTES: i64 = 10;
/// How far back a harness's sessions are read (the map shows its last [`SESSIONS_PER_HARNESS`]).
const SESSION_LOOKBACK_DAYS: i64 = 90;

/// The time windows one estate read covers. All end at `now`.
pub(super) struct Windows {
    pub now: DateTime<Utc>,
    /// The range the page asked for: volumes, spend and flows.
    pub range: Window,
    /// The last 7 days and the 7 before: every "worse than last week" rule.
    pub week: Window,
    pub prev_week: Window,
    /// 30 days: per-operation baselines and monthly savings.
    pub month: Window,
    /// Midnight UTC [`HISTORY_DAYS`] − 1 days ago to now: the daily series.
    pub history: Window,
}

#[derive(Clone, Copy)]
pub(super) struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl Windows {
    pub(super) fn new(range: Range, now: DateTime<Utc>) -> Self {
        let back = |days: i64| now - Duration::days(days);
        let today = now.date_naive();
        let first_day = today - Duration::days(HISTORY_DAYS - 1);
        Windows {
            now,
            range: Window {
                from: back(range.days()),
                to: now,
            },
            week: Window {
                from: back(7),
                to: now,
            },
            prev_week: Window {
                from: back(14),
                to: back(7),
            },
            month: Window {
                from: back(30),
                to: now,
            },
            history: Window {
                from: first_day
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is valid")
                    .and_utc(),
                to: now,
            },
        }
    }

    /// The UTC days of [`Windows::history`], oldest first.
    pub(super) fn days(&self) -> Vec<NaiveDate> {
        let first = self.history.from.date_naive();
        (0..HISTORY_DAYS)
            .map(|i| first + Duration::days(i))
            .collect()
    }

    /// The earliest instant any read needs (the range may be shorter or longer than the history).
    fn earliest(&self) -> DateTime<Utc> {
        self.range
            .from
            .min(self.history.from)
            .min(self.prev_week.from)
    }
}

/// Everything the estate is built from.
pub(super) struct Snapshot {
    pub windows: Windows,
    pub me: Uuid,
    pub people: Vec<PersonRow>,
    pub agents: Vec<AgentRow>,
    pub deployments: Vec<DeploymentRow>,
    pub failed_builds: Vec<BuildRow>,
    /// Per agent, over each window.
    pub usage_range: Vec<AgentUsage>,
    pub usage_week: Vec<AgentUsage>,
    pub usage_prev_week: Vec<AgentUsage>,
    pub usage_month: Vec<AgentUsage>,
    pub usage_daily: Vec<AgentDay>,
    pub agent_models: Vec<AgentModelRow>,
    pub models_range: Vec<ModelUsage>,
    pub models_week: Vec<ModelUsage>,
    pub models_prev_week: Vec<ModelUsage>,
    pub person_usage: Vec<PersonUsageRow>,
    pub working_on: Vec<WorkingOnRow>,
    pub sessions: Vec<SessionRow>,
    pub connectors: Vec<ConnectorRow>,
    pub expired_connections: Vec<Uuid>,
    pub enabled_access: Vec<AccessRow>,
    /// Tool calls per agent, connector and UTC day, from the earliest window on.
    pub tool_calls: Vec<ToolCall>,
    pub routers: Vec<RouterRow>,
    pub provider_failures: Vec<ProviderFailure>,
    pub workflows: Vec<WorkflowRow>,
    pub runs_daily: Vec<RunDay>,
    pub workflow_runs: Vec<WorkflowRuns>,
    pub steps_range: Vec<StepOutcome>,
    pub steps_week: Vec<StepOutcome>,
    pub spend_daily: Vec<DayAmount>,
    pub saved_daily: Vec<DayAmount>,
    pub compression: CompressionRate,
    pub routed_spend: Vec<AgentAmount>,
    pub cache: Vec<CacheRow>,
}

#[derive(FromRow, Clone)]
pub(super) struct PersonRow {
    pub id: Uuid,
    pub name: String,
}

#[derive(FromRow, Clone)]
pub(super) struct AgentRow {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    /// The attached config, else the owner's default: the router its calls resolve through.
    pub router_id: Option<Uuid>,
    /// `claude`, `codex`, … on a coding harness; `None` on an agent.
    pub harness: Option<String>,
    pub compress_enabled: bool,
}

impl AgentRow {
    pub(super) fn is_harness(&self) -> bool {
        self.harness.is_some()
    }
}

/// The latest deployment row. Only an orchestrator with a crash guardian (EE on Kubernetes)
/// fills the crash columns; elsewhere they stay empty and the rules fall back to the status.
#[derive(FromRow)]
pub(super) struct DeploymentRow {
    pub agent_id: Uuid,
    pub restart_count: i32,
    pub crash_reason: Option<String>,
    pub crashed_at: Option<DateTime<Utc>>,
}

/// An agent whose latest build job failed (no later successful one).
#[derive(FromRow)]
pub(super) struct BuildRow {
    pub agent_id: Uuid,
    pub error_msg: Option<String>,
    pub failed_at: DateTime<Utc>,
}

/// One agent's (or harness's) traces in a window.
#[derive(FromRow, Clone, Default)]
pub(super) struct UsageStats {
    pub runs: i64,
    pub spend: f64,
    /// Runs with a price: none priced means the spend is unknown, not zero.
    pub priced: i64,
    /// Runs whose errors have been counted (`error_count` materialized).
    pub checked: i64,
    pub failed: i64,
    pub failed_spend: f64,
    pub p95_ms: Option<f64>,
}

impl UsageStats {
    /// Failed share of the checked runs, in percent; `None` with nothing checked.
    pub(super) fn failed_pct(&self) -> Option<f64> {
        (self.checked > 0).then(|| self.failed as f64 * 100.0 / self.checked as f64)
    }
}

#[derive(FromRow, Clone)]
pub(super) struct AgentUsage {
    pub agent_id: Uuid,
    #[sqlx(flatten)]
    pub stats: UsageStats,
}

#[derive(FromRow, Clone)]
pub(super) struct AgentDay {
    pub agent_id: Uuid,
    pub day: NaiveDate,
    pub spend: f64,
    pub runs: i64,
    pub checked: i64,
    pub failed: i64,
    pub p95_ms: Option<f64>,
}

#[derive(FromRow, Clone)]
pub(super) struct AgentModelRow {
    pub agent_id: Uuid,
    pub model: String,
    pub calls: i64,
    pub spend: f64,
}

#[derive(FromRow, Clone)]
pub(super) struct ModelUsage {
    pub model: String,
    pub provider: String,
    pub calls: i64,
    pub spend: f64,
    /// Agent calls only: what [`ModelUsage::p95_ms`] is measured over.
    pub timed_calls: i64,
    pub p95_ms: Option<f64>,
}

/// One person's traces on one agent or harness in the range.
#[derive(FromRow, Clone)]
pub(super) struct PersonUsageRow {
    pub user_id: Uuid,
    pub agent_id: Uuid,
    pub runs: i64,
    pub sessions: i64,
    pub spend: f64,
    pub input_tokens: i64,
    pub checked: i64,
    pub failed: i64,
}

#[derive(FromRow)]
pub(super) struct WorkingOnRow {
    pub user_id: Uuid,
    pub agent_id: Uuid,
}

/// One coding-harness session.
#[derive(FromRow, Clone)]
pub(super) struct SessionRow {
    pub agent_id: Uuid,
    pub last_at: DateTime<Utc>,
    pub cost: f64,
    pub latency_ms: i64,
    pub failed: bool,
}

#[derive(Clone)]
pub(super) struct ConnectorRow {
    pub id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    /// A Composio toolkit (no page of its own) rather than an MCP server.
    pub toolkit: bool,
    pub is_active: bool,
    pub updated_at: DateTime<Utc>,
    pub setup_failed: Option<String>,
}

#[derive(FromRow)]
pub(super) struct AccessRow {
    pub agent_id: Uuid,
    pub connector_id: Uuid,
}

#[derive(Clone)]
pub(super) struct ToolCall {
    pub agent_id: Uuid,
    pub connector_id: Uuid,
    pub day: NaiveDate,
    pub calls: i64,
    pub errors: i64,
}

#[derive(FromRow, Clone)]
pub(super) struct RouterRow {
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    pub fallback_models: sqlx::types::Json<Vec<serde_json::Value>>,
}

/// A custom provider whose last model-catalog sync failed.
#[derive(FromRow, Clone)]
pub(super) struct ProviderFailure {
    pub label: String,
    pub display_name: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub(super) struct WorkflowRow {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Deserialize, Clone)]
pub(super) struct WorkflowStep {
    pub step_index: i32,
    pub agent_id: Uuid,
    pub agent_name: String,
}

#[derive(FromRow, Clone)]
pub(super) struct RunDay {
    pub maf_id: Uuid,
    pub day: NaiveDate,
    pub runs: i64,
    pub failed: i64,
}

/// One workflow's runs in the range and what they cost.
#[derive(FromRow, Clone)]
pub(super) struct WorkflowRuns {
    pub maf_id: Uuid,
    pub runs: i64,
    pub cost: f64,
}

/// How one agent's steps went inside one workflow's runs.
#[derive(FromRow, Clone)]
pub(super) struct StepOutcome {
    pub maf_id: Uuid,
    pub agent_id: Uuid,
    pub steps: i64,
    pub failed: i64,
}

#[derive(FromRow, Clone)]
pub(super) struct DayAmount {
    pub day: NaiveDate,
    pub amount: f64,
}

#[derive(FromRow, Clone)]
pub(super) struct AgentAmount {
    pub agent_id: Uuid,
    pub amount: f64,
}

/// What payload compression measurably saved on the agents that have it on, over 30 days.
#[derive(FromRow, Clone, Default)]
pub(super) struct CompressionRate {
    pub saved: f64,
    pub spend: f64,
}

/// One agent's cached-input use on one model over 30 days, with that model's current prices.
#[derive(FromRow, Clone)]
pub(super) struct CacheRow {
    pub agent_id: Uuid,
    pub model: String,
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub input_price_per_1m: Option<f64>,
    pub cache_read_price_per_1m: Option<f64>,
}

/// Reads everything the estate needs for `scope`, in two rounds: what is visible, then what
/// those things did.
pub(super) async fn snapshot(
    state: &AppState,
    scope: &Scope,
    windows: Windows,
) -> Result<Snapshot, OverviewError> {
    let db = &state.db;
    let connectors = connectors(db, scope).await?;
    let (people, mut agents, workflows) =
        tokio::try_join!(people(db, scope), agents(db, scope), workflows(db, scope))?;
    for agent in &mut agents {
        if let Some(live) = live_status(state, agent.id, &agent.status).await {
            agent.status = live;
        }
    }
    let ids: Vec<Uuid> = agents.iter().map(|a| a.id).collect();
    let harness_ids: Vec<Uuid> = agents
        .iter()
        .filter(|a| a.is_harness())
        .map(|a| a.id)
        .collect();
    let person_ids: Vec<Uuid> = people.iter().map(|p| p.id).collect();
    let maf_ids: Vec<Uuid> = workflows.iter().map(|w| w.id).collect();
    let router_ids: Vec<Uuid> = agents.iter().filter_map(|a| a.router_id).collect();
    let w = &windows;

    let (deployments, failed_builds, usage_range, usage_week, usage_prev_week, usage_month) = tokio::try_join!(
        deployments(db, &ids),
        failed_builds(db, &ids),
        agent_usage(db, &ids, w.range),
        agent_usage(db, &ids, w.week),
        agent_usage(db, &ids, w.prev_week),
        agent_usage(db, &ids, w.month),
    )?;
    let (usage_daily, agent_models, models_range, models_week, models_prev_week, person_usage) = tokio::try_join!(
        agent_daily(db, &ids, w.history),
        agent_models(db, &ids, w.range),
        model_usage(db, &ids, &harness_ids, w.range),
        model_usage(db, &ids, &harness_ids, w.week),
        model_usage(db, &ids, &harness_ids, w.prev_week),
        person_usage(db, &ids, &person_ids, w.range),
    )?;
    let (working_on, sessions, expired_connections, enabled_access, raw_tool_calls, system_tools) =
        tokio::try_join!(
            working_on(db, &ids, &person_ids, w.now),
            sessions(db, &agents, w.now),
            expired_connections(db, scope.me),
            enabled_access(db, &ids),
            tool_calls(
                db,
                &ids,
                Window {
                    from: w.earliest(),
                    to: w.now
                }
            ),
            system_tools(db),
        )?;
    let (routers, provider_failures, runs_daily, workflow_runs, steps_range, steps_week) = tokio::try_join!(
        routers(db, scope, &router_ids),
        provider_failures(db),
        runs_daily(
            db,
            &maf_ids,
            Window {
                from: w.earliest(),
                to: w.now
            }
        ),
        workflow_runs(db, &maf_ids, w.range),
        step_outcomes(db, &maf_ids, w.range),
        step_outcomes(db, &maf_ids, w.week),
    )?;
    let (spend_daily, saved_daily, compression, routed_spend, cache) = tokio::try_join!(
        spend_daily(db, &ids, w.history),
        saved_daily(db, scope, &ids, w.history),
        compression_rate(db, &agents, w.month),
        routed_spend(db, &ids, w.month),
        cache_use(db, &ids, w.month),
    )?;

    let tool_calls = attribute_tool_calls(raw_tool_calls, &connectors, &system_tools);
    Ok(Snapshot {
        me: scope.me,
        people,
        agents,
        deployments,
        failed_builds,
        usage_range,
        usage_week,
        usage_prev_week,
        usage_month,
        usage_daily,
        agent_models,
        models_range,
        models_week,
        models_prev_week,
        person_usage,
        working_on,
        sessions,
        connectors,
        expired_connections,
        enabled_access,
        tool_calls,
        routers,
        provider_failures,
        workflows,
        runs_daily,
        workflow_runs,
        steps_range,
        steps_week,
        spend_daily,
        saved_daily,
        compression,
        routed_spend,
        cache,
        windows,
    })
}

// ── What is visible ──────────────────────────────────────────────────────────

/// The people the caller may see: everyone for a superuser; otherwise themselves plus whoever
/// the edition's org chart opens up (OSS has none — its user directory is superuser-only).
async fn people(db: &PgPool, scope: &Scope) -> Result<Vec<PersonRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, COALESCE(display_name, username) AS name FROM users \
         WHERE deleted_at IS NULL AND is_active \
           AND ($1::uuid[] IS NULL OR id = ANY($1)) \
         ORDER BY name",
    )
    .bind(scope.visible_people.as_deref())
    .fetch_all(db)
    .await
}

/// The agents and coding harnesses the catalog would list for the caller.
async fn agents(db: &PgPool, scope: &Scope) -> Result<Vec<AgentRow>, sqlx::Error> {
    let sql = format!(
        r#"SELECT a.id, COALESCE(a.display_name, a.name) AS name, a.owner_id, a.status, a.updated_at,
                  COALESCE(a.llm_config_id, (
                      SELECT c.id FROM llm_configs c
                       WHERE c.created_by = a.owner_id AND c.is_default AND c.deleted_at IS NULL
                       LIMIT 1)) AS router_id,
                  a.coding_agent_integration_id AS harness, a.compress_enabled
             FROM agents a
            WHERE a.deleted_at IS NULL AND NOT a.is_internal AND {access}
            ORDER BY a.created_at"#,
        access = agent_access_predicate("$1", "$2", "a")
    );
    sqlx::query_as(&sql)
        .bind(scope.listing_user())
        .bind(&scope.org_granted)
        .fetch_all(db)
        .await
}

/// Every connector for a superuser; otherwise the MCP page's own rule (Composio ∪ owned ∪ granted).
async fn connectors(db: &PgPool, scope: &Scope) -> Result<Vec<ConnectorRow>, McpError> {
    let rows = if scope.superuser {
        sqlx::query_as::<_, McpConnector>(
            "SELECT * FROM mcp_connectors ORDER BY provider_type, name",
        )
        .fetch_all(db)
        .await?
    } else {
        nasiko_mcp_gateway::repo::list_accessible_connectors(db, scope.me).await?
    };
    Ok(rows
        .into_iter()
        .map(|c| ConnectorRow {
            toolkit: c.is_composio(),
            is_active: c.is_active.unwrap_or(true),
            setup_failed: (c.setup_status.as_deref() == Some("failed"))
                .then(|| c.setup_error.clone().unwrap_or_default()),
            id: c.id,
            name: c.name,
            display_name: c.display_name,
            updated_at: c.updated_at,
        })
        .collect())
}

/// Active workflows: all of them for a superuser, the caller's own otherwise.
async fn workflows(db: &PgPool, scope: &Scope) -> Result<Vec<WorkflowRow>, sqlx::Error> {
    #[derive(FromRow)]
    struct Row {
        id: Uuid,
        name: String,
        user_id: Uuid,
        maf_json: sqlx::types::Json<serde_json::Value>,
    }
    #[derive(Deserialize, Default)]
    struct Definition {
        #[serde(default)]
        steps: Vec<WorkflowStep>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, user_id, maf_json FROM mafs \
         WHERE status = 'active' AND ($1::uuid IS NULL OR user_id = $1) ORDER BY created_at",
    )
    .bind(scope.listing_user())
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            // A definition this server can't read still draws the workflow, without steps.
            let mut steps = serde_json::from_value::<Definition>(r.maf_json.0)
                .unwrap_or_default()
                .steps;
            steps.sort_by_key(|s| s.step_index);
            WorkflowRow {
                id: r.id,
                name: r.name,
                owner_id: r.user_id,
                steps,
            }
        })
        .collect())
}

/// The configs the caller made (all for a superuser), plus any their visible agents route through.
async fn routers(db: &PgPool, scope: &Scope, used: &[Uuid]) -> Result<Vec<RouterRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, provider, model, fallback_models FROM llm_configs \
         WHERE deleted_at IS NULL AND ($1::uuid IS NULL OR created_by = $1 OR id = ANY($2)) \
         ORDER BY created_at",
    )
    .bind(scope.listing_user())
    .bind(used)
    .fetch_all(db)
    .await
}

async fn provider_failures(db: &PgPool) -> Result<Vec<ProviderFailure>, sqlx::Error> {
    sqlx::query_as(
        "SELECT label, display_name, last_sync_error AS error FROM custom_providers \
         WHERE last_sync_status = 'failed'",
    )
    .fetch_all(db)
    .await
}

// ── What it did ──────────────────────────────────────────────────────────────

async fn deployments(db: &PgPool, ids: &[Uuid]) -> Result<Vec<DeploymentRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT ON (agent_id) agent_id, restart_count, crash_reason, crashed_at \
           FROM agent_deployments WHERE agent_id = ANY($1) \
          ORDER BY agent_id, created_at DESC",
    )
    .bind(ids)
    .fetch_all(db)
    .await
}

async fn failed_builds(db: &PgPool, ids: &[Uuid]) -> Result<Vec<BuildRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, error_msg, failed_at FROM ( \
             SELECT DISTINCT ON (agent_id) agent_id, status, error_msg, \
                    COALESCE(completed_at, created_at) AS failed_at \
               FROM build_jobs WHERE agent_id = ANY($1) AND status IN ('done', 'failed') \
              ORDER BY agent_id, created_at DESC) latest \
          WHERE status = 'failed'",
    )
    .bind(ids)
    .fetch_all(db)
    .await
}

/// Per-trace columns every usage read aggregates the same way.
const USAGE_STATS: &str = "COUNT(*)::bigint AS runs, \
     COALESCE(SUM(cost_usd), 0)::float8 AS spend, \
     COUNT(*) FILTER (WHERE cost_usd > 0)::bigint AS priced, \
     COUNT(*) FILTER (WHERE error_count IS NOT NULL)::bigint AS checked, \
     COUNT(*) FILTER (WHERE error_count > 0)::bigint AS failed, \
     COALESCE(SUM(cost_usd) FILTER (WHERE error_count > 0), 0)::float8 AS failed_spend, \
     percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms)::float8 AS p95_ms";

async fn agent_usage(db: &PgPool, ids: &[Uuid], w: Window) -> Result<Vec<AgentUsage>, sqlx::Error> {
    let sql = format!(
        "SELECT agent_id, {USAGE_STATS} FROM trace_usage \
         WHERE agent_id = ANY($1) AND started_at >= $2 AND started_at < $3 GROUP BY agent_id"
    );
    sqlx::query_as(&sql)
        .bind(ids)
        .bind(w.from)
        .bind(w.to)
        .fetch_all(db)
        .await
}

async fn agent_daily(db: &PgPool, ids: &[Uuid], w: Window) -> Result<Vec<AgentDay>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, (started_at AT TIME ZONE 'UTC')::date AS day, \
                COALESCE(SUM(cost_usd), 0)::float8 AS spend, COUNT(*)::bigint AS runs, \
                COUNT(*) FILTER (WHERE error_count IS NOT NULL)::bigint AS checked, \
                COUNT(*) FILTER (WHERE error_count > 0)::bigint AS failed, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms)::float8 AS p95_ms \
           FROM trace_usage WHERE agent_id = ANY($1) AND started_at >= $2 AND started_at < $3 \
          GROUP BY 1, 2",
    )
    .bind(ids)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

async fn agent_models(
    db: &PgPool,
    ids: &[Uuid],
    w: Window,
) -> Result<Vec<AgentModelRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, model, COUNT(*)::bigint AS calls, COALESCE(SUM(cost_usd), 0)::float8 AS spend \
           FROM trace_usage \
          WHERE agent_id = ANY($1) AND model IS NOT NULL AND started_at >= $2 AND started_at < $3 \
          GROUP BY 1, 2",
    )
    .bind(ids)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

/// Calls and spend count every caller; latency counts agents only. A coding harness's trace is a
/// whole turn (often minutes), not one model call, so its time would read as the model's.
async fn model_usage(
    db: &PgPool,
    ids: &[Uuid],
    harnesses: &[Uuid],
    w: Window,
) -> Result<Vec<ModelUsage>, sqlx::Error> {
    sqlx::query_as(
        "SELECT model, COALESCE(provider, 'unknown') AS provider, COUNT(*)::bigint AS calls, \
                COALESCE(SUM(cost_usd), 0)::float8 AS spend, \
                COUNT(*) FILTER (WHERE agent_id <> ALL($4))::bigint AS timed_calls, \
                percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms) \
                    FILTER (WHERE agent_id <> ALL($4))::float8 AS p95_ms \
           FROM trace_usage \
          WHERE agent_id = ANY($1) AND model IS NOT NULL AND started_at >= $2 AND started_at < $3 \
          GROUP BY 1, 2",
    )
    .bind(ids)
    .bind(w.from)
    .bind(w.to)
    .bind(harnesses)
    .fetch_all(db)
    .await
}

async fn person_usage(
    db: &PgPool,
    ids: &[Uuid],
    people: &[Uuid],
    w: Window,
) -> Result<Vec<PersonUsageRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id, agent_id, COUNT(*)::bigint AS runs, \
                COUNT(DISTINCT session_id)::bigint AS sessions, \
                COALESCE(SUM(cost_usd), 0)::float8 AS spend, \
                COALESCE(SUM(input_tokens), 0)::bigint AS input_tokens, \
                COUNT(*) FILTER (WHERE error_count IS NOT NULL)::bigint AS checked, \
                COUNT(*) FILTER (WHERE error_count > 0)::bigint AS failed \
           FROM trace_usage \
          WHERE agent_id = ANY($1) AND user_id = ANY($2) AND started_at >= $3 AND started_at < $4 \
          GROUP BY 1, 2",
    )
    .bind(ids)
    .bind(people)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

async fn working_on(
    db: &PgPool,
    ids: &[Uuid],
    people: &[Uuid],
    now: DateTime<Utc>,
) -> Result<Vec<WorkingOnRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT ON (user_id) user_id, agent_id FROM trace_usage \
          WHERE agent_id = ANY($1) AND user_id = ANY($2) AND started_at >= $3 \
          ORDER BY user_id, started_at DESC",
    )
    .bind(ids)
    .bind(people)
    .bind(now - Duration::minutes(WORKING_ON_MINUTES))
    .fetch_all(db)
    .await
}

/// Each coding harness's latest sessions, oldest first.
async fn sessions(
    db: &PgPool,
    agents: &[AgentRow],
    now: DateTime<Utc>,
) -> Result<Vec<SessionRow>, sqlx::Error> {
    let harnesses: Vec<Uuid> = agents
        .iter()
        .filter(|a| a.is_harness())
        .map(|a| a.id)
        .collect();
    sqlx::query_as(
        "SELECT agent_id, last_at, cost, latency_ms, failed FROM ( \
             SELECT agent_id, MAX(started_at) AS last_at, \
                    COALESCE(SUM(cost_usd), 0)::float8 AS cost, \
                    COALESCE(SUM(latency_ms), 0)::bigint AS latency_ms, \
                    COALESCE(BOOL_OR(error_count > 0), false) AS failed, \
                    ROW_NUMBER() OVER (PARTITION BY agent_id ORDER BY MAX(started_at) DESC) AS n \
               FROM trace_usage \
              WHERE agent_id = ANY($1) AND session_id IS NOT NULL AND started_at >= $2 \
              GROUP BY agent_id, session_id) s \
          WHERE n <= $3 ORDER BY agent_id, last_at",
    )
    .bind(&harnesses)
    .bind(now - Duration::days(SESSION_LOOKBACK_DAYS))
    .bind(SESSIONS_PER_HARNESS)
    .fetch_all(db)
    .await
}

/// The caller's own connections that need signing in again.
async fn expired_connections(db: &PgPool, me: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT connector_id FROM mcp_user_connections \
          WHERE user_id = $1 AND status = 'EXPIRED'",
    )
    .bind(me)
    .fetch_all(db)
    .await
}

async fn enabled_access(db: &PgPool, ids: &[Uuid]) -> Result<Vec<AccessRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, connector_id FROM mcp_agent_connector_access \
          WHERE enabled AND agent_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(db)
    .await
}

/// A gateway tool call, before it is attributed to a connector.
#[derive(FromRow)]
struct RawToolCall {
    agent_id: Uuid,
    tool: String,
    day: NaiveDate,
    calls: i64,
    errors: i64,
}

async fn tool_calls(db: &PgPool, ids: &[Uuid], w: Window) -> Result<Vec<RawToolCall>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, model AS tool, (created_at AT TIME ZONE 'UTC')::date AS day, \
                COUNT(*)::bigint AS calls, \
                COUNT(*) FILTER (WHERE finish_reason = 'error')::bigint AS errors \
           FROM token_usage \
          WHERE operation_type = 'mcp_tool_call' AND agent_id = ANY($1) \
            AND created_at >= $2 AND created_at < $3 \
          GROUP BY 1, 2, 3",
    )
    .bind(ids)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

/// Bare tool names a system connector serves (theirs carry no namespace prefix).
async fn system_tools(db: &PgPool) -> Result<Vec<(String, Uuid)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT t.tool_name, t.connector_id FROM mcp_connector_tools t \
           JOIN mcp_connectors c ON c.id = t.connector_id WHERE c.provider_type = 'system'",
    )
    .fetch_all(db)
    .await
}

/// Which connector served each tool call, by the gateway's own naming: `{prefix}__{tool}` is the
/// MCP server whose id starts with that prefix ([`connector_prefix`]); a bare name is a system
/// connector's tool, else a Composio slug whose toolkit is the connector's name. A call that
/// matches no visible connector is left out.
fn attribute_tool_calls(
    raw: Vec<RawToolCall>,
    connectors: &[ConnectorRow],
    system: &[(String, Uuid)],
) -> Vec<ToolCall> {
    // Only MCP and system servers are namespaced: a Composio tool is a bare slug, so a toolkit
    // never claims a prefix (two ids can share their first 16 hex digits).
    let by_prefix: HashMap<String, Uuid> = connectors
        .iter()
        .filter(|c| !c.toolkit)
        .map(|c| (connector_prefix(c.id), c.id))
        .collect();
    let by_toolkit: HashMap<String, Uuid> = connectors
        .iter()
        .filter(|c| c.toolkit)
        .map(|c| (c.name.to_ascii_lowercase(), c.id))
        .collect();
    let visible: HashSet<Uuid> = connectors.iter().map(|c| c.id).collect();
    let by_system: HashMap<&str, Uuid> = system
        .iter()
        .filter(|(_, id)| visible.contains(id))
        .map(|(name, id)| (name.as_str(), *id))
        .collect();
    raw.into_iter()
        .filter_map(|r| {
            let connector = match r.tool.split_once("__") {
                Some((prefix, _)) => by_prefix.get(prefix).copied(),
                None => by_system
                    .get(r.tool.as_str())
                    .or_else(|| by_toolkit.get(&toolkit_from_composio_slug(&r.tool)))
                    .copied(),
            }?;
            Some(ToolCall {
                agent_id: r.agent_id,
                connector_id: connector,
                day: r.day,
                calls: r.calls,
                errors: r.errors,
            })
        })
        .collect()
}

async fn runs_daily(db: &PgPool, mafs: &[Uuid], w: Window) -> Result<Vec<RunDay>, sqlx::Error> {
    sqlx::query_as(
        "SELECT maf_id, (started_at AT TIME ZONE 'UTC')::date AS day, COUNT(*)::bigint AS runs, \
                COUNT(*) FILTER (WHERE status = 'failed')::bigint AS failed \
           FROM maf_executions \
          WHERE maf_id = ANY($1) AND started_at >= $2 AND started_at < $3 GROUP BY 1, 2",
    )
    .bind(mafs)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

async fn workflow_runs(
    db: &PgPool,
    mafs: &[Uuid],
    w: Window,
) -> Result<Vec<WorkflowRuns>, sqlx::Error> {
    sqlx::query_as(
        "SELECT maf_id, COUNT(*)::bigint AS runs, COALESCE(SUM(cost_usd), 0)::float8 AS cost \
           FROM maf_executions \
          WHERE maf_id = ANY($1) AND started_at >= $2 AND started_at < $3 GROUP BY 1",
    )
    .bind(mafs)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

/// Each workflow step that ran, by the agent that ran it (`step_results`, written as it goes).
async fn step_outcomes(
    db: &PgPool,
    mafs: &[Uuid],
    w: Window,
) -> Result<Vec<StepOutcome>, sqlx::Error> {
    sqlx::query_as(
        "SELECT e.maf_id, (s->>'agent_id')::uuid AS agent_id, COUNT(*)::bigint AS steps, \
                COUNT(*) FILTER (WHERE s->>'status' = 'failed')::bigint AS failed \
           FROM maf_executions e \
          CROSS JOIN LATERAL jsonb_array_elements( \
                CASE WHEN jsonb_typeof(e.step_results) = 'array' THEN e.step_results ELSE '[]'::jsonb END) s \
          WHERE e.maf_id = ANY($1) AND e.started_at >= $2 AND e.started_at < $3 \
            AND s->>'status' IN ('success', 'failed') \
            AND s->>'agent_id' ~ '^[0-9a-fA-F-]{36}$' \
          GROUP BY 1, 2",
    )
    .bind(mafs)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

async fn spend_daily(db: &PgPool, ids: &[Uuid], w: Window) -> Result<Vec<DayAmount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT (started_at AT TIME ZONE 'UTC')::date AS day, COALESCE(SUM(cost_usd), 0)::float8 AS amount \
           FROM trace_usage WHERE agent_id = ANY($1) AND started_at >= $2 AND started_at < $3 \
          GROUP BY 1",
    )
    .bind(ids)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

/// What every measured layer saved per UTC day: the whole workspace for a superuser, otherwise the
/// caller's own requests and their visible agents' (the savings endpoint's own pricing).
async fn saved_daily(
    db: &PgPool,
    scope: &Scope,
    ids: &[Uuid],
    w: Window,
) -> Result<Vec<DayAmount>, sqlx::Error> {
    let sql = format!(
        "SELECT (ts.created_at AT TIME ZONE 'UTC')::date AS day, {PRICED_COST} AS amount \
           FROM token_savings ts {PRICE_JOIN} \
          WHERE ts.created_at >= $1 AND ts.created_at < $2 \
            AND ($3::uuid IS NULL OR ts.user_id = $3 OR ts.agent_id = ANY($4)) \
          GROUP BY 1"
    );
    sqlx::query_as(&sql)
        .bind(w.from)
        .bind(w.to)
        .bind(scope.listing_user())
        .bind(ids)
        .fetch_all(db)
        .await
}

/// Payload compression's measured saving on the agents that have it on, against what they spent
/// through the router: the rate an agent without it can expect.
async fn compression_rate(
    db: &PgPool,
    agents: &[AgentRow],
    w: Window,
) -> Result<CompressionRate, sqlx::Error> {
    let on: Vec<Uuid> = agents
        .iter()
        .filter(|a| a.compress_enabled)
        .map(|a| a.id)
        .collect();
    let sql = format!(
        "SELECT COALESCE((SELECT {PRICED_COST} FROM token_savings ts {PRICE_JOIN} \
                           WHERE ts.layer = 'compress_payload' AND ts.agent_id = ANY($1) \
                             AND ts.created_at >= $2 AND ts.created_at < $3), 0)::float8 AS saved, \
                COALESCE((SELECT SUM(cost_usd) FROM token_usage \
                           WHERE agent_id = ANY($1) AND operation_type <> 'mcp_tool_call' \
                             AND created_at >= $2 AND created_at < $3), 0)::float8 AS spend"
    );
    sqlx::query_as(&sql)
        .bind(&on)
        .bind(w.from)
        .bind(w.to)
        .fetch_one(db)
        .await
}

/// Each agent's spend through the LLM router (what payload compression applies to).
async fn routed_spend(
    db: &PgPool,
    ids: &[Uuid],
    w: Window,
) -> Result<Vec<AgentAmount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT agent_id, COALESCE(SUM(cost_usd), 0)::float8 AS amount FROM token_usage \
          WHERE agent_id = ANY($1) AND operation_type <> 'mcp_tool_call' \
            AND created_at >= $2 AND created_at < $3 GROUP BY 1",
    )
    .bind(ids)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

async fn cache_use(db: &PgPool, ids: &[Uuid], w: Window) -> Result<Vec<CacheRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT u.agent_id, u.model, \
                COALESCE(SUM(u.input_tokens), 0)::bigint AS input_tokens, \
                COALESCE(SUM(u.cache_read_tokens), 0)::bigint AS cache_read_tokens, \
                MAX(p.input_price_per_1m)::float8 AS input_price_per_1m, \
                MAX(p.cache_read_price_per_1m)::float8 AS cache_read_price_per_1m \
           FROM trace_usage u \
           LEFT JOIN model_pricing p \
             ON p.provider = u.provider AND p.model = u.model AND p.effective_until IS NULL \
          WHERE u.agent_id = ANY($1) AND u.model IS NOT NULL \
            AND u.started_at >= $2 AND u.started_at < $3 \
          GROUP BY 1, 2",
    )
    .bind(ids)
    .bind(w.from)
    .bind(w.to)
    .fetch_all(db)
    .await
}

#[cfg(test)]
impl Snapshot {
    /// An estate with nothing in it, read at `now`, for tests to fill.
    pub(super) fn empty(now: DateTime<Utc>) -> Self {
        Snapshot {
            windows: Windows::new(Range::Days30, now),
            me: Uuid::from_u128(1),
            people: Vec::new(),
            agents: Vec::new(),
            deployments: Vec::new(),
            failed_builds: Vec::new(),
            usage_range: Vec::new(),
            usage_week: Vec::new(),
            usage_prev_week: Vec::new(),
            usage_month: Vec::new(),
            usage_daily: Vec::new(),
            agent_models: Vec::new(),
            models_range: Vec::new(),
            models_week: Vec::new(),
            models_prev_week: Vec::new(),
            person_usage: Vec::new(),
            working_on: Vec::new(),
            sessions: Vec::new(),
            connectors: Vec::new(),
            expired_connections: Vec::new(),
            enabled_access: Vec::new(),
            tool_calls: Vec::new(),
            routers: Vec::new(),
            provider_failures: Vec::new(),
            workflows: Vec::new(),
            runs_daily: Vec::new(),
            workflow_runs: Vec::new(),
            steps_range: Vec::new(),
            steps_week: Vec::new(),
            spend_daily: Vec::new(),
            saved_daily: Vec::new(),
            compression: CompressionRate::default(),
            routed_spend: Vec::new(),
            cache: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector(id: u128, name: &str, toolkit: bool) -> ConnectorRow {
        ConnectorRow {
            id: Uuid::from_u128(id),
            name: name.into(),
            display_name: None,
            toolkit,
            is_active: true,
            updated_at: Utc::now(),
            setup_failed: None,
        }
    }

    fn call(tool: &str) -> RawToolCall {
        RawToolCall {
            agent_id: Uuid::from_u128(9),
            tool: tool.into(),
            day: NaiveDate::from_ymd_opt(2026, 10, 1).expect("valid date"),
            calls: 3,
            errors: 1,
        }
    }

    #[test]
    fn tool_calls_go_to_the_connector_the_gateway_routed_them_to() {
        let server = connector(
            0x1234_5678_9abc_def0_1111_2222_3333_4444,
            "jira-cloud",
            false,
        );
        let gmail = connector(2, "gmail", true);
        let system = connector(3, "workspace", false);
        let prefixed = format!("{}__create_issue", connector_prefix(server.id));
        let calls = attribute_tool_calls(
            vec![
                call(&prefixed),
                call("GMAIL_SEND_EMAIL"),
                call("read_file"),
                call("unknown_tool"),
            ],
            &[server.clone(), gmail.clone(), system.clone()],
            &[("read_file".into(), system.id)],
        );
        let ids: Vec<Uuid> = calls.iter().map(|c| c.connector_id).collect();
        assert_eq!(ids, vec![server.id, gmail.id, system.id]);
    }

    #[test]
    fn a_call_to_a_connector_the_caller_cannot_see_is_left_out() {
        // A prefix is the id's first 16 hex digits: give it a high half no visible one shares.
        let hidden = Uuid::from_u128(0xffff_0000_0000_0000_0000_0000_0000_0077);
        let calls = attribute_tool_calls(
            vec![
                call(&format!("{}__x", connector_prefix(hidden))),
                call("read_file"),
            ],
            &[connector(2, "gmail", true)],
            &[("read_file".into(), hidden)],
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn a_toolkit_sharing_a_servers_prefix_never_takes_its_calls() {
        let server = connector(
            0x5eed_000e_0000_4000_8000_0000_0000_0001,
            "jira-cloud",
            false,
        );
        let toolkit = connector(0x5eed_000e_0000_4000_8000_0000_0000_0002, "github", true);
        assert_eq!(connector_prefix(server.id), connector_prefix(toolkit.id));
        for order in [
            [server.clone(), toolkit.clone()],
            [toolkit.clone(), server.clone()],
        ] {
            let calls = attribute_tool_calls(
                vec![call(&format!("{}__search", connector_prefix(server.id)))],
                &order,
                &[],
            );
            assert_eq!(calls[0].connector_id, server.id);
        }
    }

    #[test]
    fn the_history_covers_fourteen_utc_days_ending_today() {
        let now = DateTime::parse_from_rfc3339("2026-10-21T15:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let w = Windows::new(Range::Days30, now);
        let days = w.days();
        assert_eq!(days.len(), 14);
        assert_eq!(days.last().copied(), Some(now.date_naive()));
        assert_eq!(w.history.from.to_rfc3339(), "2026-10-08T00:00:00+00:00");
    }
}
