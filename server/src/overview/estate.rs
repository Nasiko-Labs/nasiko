//! The estate (OV-1): every node the Overview's map draws, the flows between its six cards, and
//! what the viewer owns or touches. Pure: built from a [`Snapshot`].
//!
//! The wire shape is the UI's `estateSchema` (`ui/common/src/features/overview/estate/types.ts`).
//! Ids: people, agents, harnesses, workflows, tools and routers are their own UUIDs; a model is
//! `model:<name>`, a provider `provider:<name>`, and the path coding harnesses take with no router
//! is `router:direct`.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::load::{AgentRow, Snapshot, UsageStats};
use super::rules::{self, Chain, Opportunity, Problem, median};

/// A person with fewer sessions than this in the range is a light user.
const LIGHT_SESSIONS: i64 = 5;
/// A person whose runs fail more often than this (in percent) is stuck.
const STUCK_SUCCESS_PCT: f64 = 80.0;
/// A person whose sessions cost this many times the typical person's is expensive.
const EXPENSIVE_FACTOR: f64 = 2.0;
/// Models listed per agent or harness, by spend.
const MODELS_PER_NODE: usize = 3;
/// The Map leaves out a tool turned off longer ago than this; a recent one it fades.
const DISABLED_SHOWN_DAYS: i64 = 14;
const DIRECT_ROUTER: &str = "router:direct";

#[derive(Serialize)]
pub(super) struct Estate {
    as_of: DateTime<Utc>,
    days: Vec<NaiveDate>,
    me: Uuid,
    mine: Vec<String>,
    /// Teams. This edition has none: always empty (the contract's "server without teams"), so the
    /// village draws no districts and no team budgets.
    groups: Vec<serde_json::Value>,
    people: Vec<Person>,
    workflows: Vec<Workflow>,
    routers: Vec<Router>,
    providers: Vec<Provider>,
    models: Vec<Model>,
    agents: Vec<Agent>,
    tools: Vec<Tool>,
    harnesses: Vec<Harness>,
    flows: Vec<Flow>,
    pub problems: Vec<Problem>,
    chains: Vec<Chain>,
    opportunities: Vec<Opportunity>,
    daily: Vec<Daily>,
    /// The viewer's own allowance. Allowances need budgets, which this edition doesn't have.
    allowance: Option<()>,
}

#[derive(Serialize)]
struct Person {
    id: Uuid,
    name: String,
    group_id: Option<Uuid>,
    spend_usd: f64,
    sessions: i64,
    success_pct: Option<f64>,
    avg_context_tokens: Option<f64>,
    segment: &'static str,
    working_on: Option<WorkingOn>,
}

#[derive(Serialize)]
struct WorkingOn {
    kind: &'static str,
    id: Uuid,
}

#[derive(Serialize)]
struct Workflow {
    id: Uuid,
    name: String,
    owner_id: Uuid,
    runs: i64,
    steps: Vec<Step>,
}

#[derive(Serialize)]
struct Step {
    kind: &'static str,
    r#ref: Option<Uuid>,
    label: String,
}

#[derive(Serialize)]
struct Router {
    id: String,
    name: String,
    direct: bool,
    note: Option<String>,
}

#[derive(Serialize)]
struct Provider {
    id: String,
    name: String,
    brand: Option<&'static str>,
    share_pct: f64,
}

#[derive(Serialize)]
struct Model {
    id: String,
    name: String,
    provider_id: String,
    spend_usd: f64,
    calls: i64,
    p95_ms: Option<f64>,
}

#[derive(Serialize)]
struct Agent {
    id: Uuid,
    name: String,
    owner_id: Uuid,
    router_id: Option<Uuid>,
    model_ids: Vec<String>,
    tool_ids: Vec<Uuid>,
    spend_usd: f64,
    runs: i64,
    success_pct: Option<f64>,
    p95_ms: Option<f64>,
    idle: bool,
    failed_spend_usd: f64,
}

#[derive(Serialize)]
struct Tool {
    id: Uuid,
    name: String,
    brand: Option<&'static str>,
    connector_id: Option<Uuid>,
    disabled_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct Harness {
    id: Uuid,
    name: String,
    harness: String,
    owner_id: Uuid,
    model_ids: Vec<String>,
    tool_ids: Vec<Uuid>,
    spend_usd: Option<f64>,
    sessions: Vec<&'static str>,
}

#[derive(Serialize)]
struct Flow {
    id: &'static str,
    from: &'static str,
    to: &'static str,
    volume: i64,
    unit: &'static str,
    spend_usd: Option<f64>,
    direct: bool,
}

#[derive(Serialize)]
struct Daily {
    budgeted_usd: f64,
    unbudgeted_usd: f64,
    uncapped_usd: f64,
    saved_usd: f64,
}

pub(super) fn build(snap: &Snapshot) -> Estate {
    let range: HashMap<Uuid, &UsageStats> = snap
        .usage_range
        .iter()
        .map(|u| (u.agent_id, &u.stats))
        .collect();
    let tools_by_agent = tools_by_agent(snap);
    let models_by_agent = models_by_agent(snap);
    let agents: Vec<Agent> = snap
        .agents
        .iter()
        .filter(|a| !a.is_harness())
        .map(|a| {
            agent(
                a,
                range.get(&a.id).copied(),
                &models_by_agent,
                &tools_by_agent,
            )
        })
        .collect();
    let harnesses: Vec<Harness> = snap
        .agents
        .iter()
        .filter(|a| a.is_harness())
        .map(|h| {
            harness(
                snap,
                h,
                range.get(&h.id).copied(),
                &models_by_agent,
                &tools_by_agent,
            )
        })
        .collect();
    let models = models(snap);
    let routers = routers(snap);
    let workflows = workflows(snap);
    let mine = mine(snap, &agents, &harnesses, &models);
    let rules::Findings {
        mut problems,
        chains,
        opportunities,
    } = rules::detect(snap);
    for p in &mut problems {
        p.mine = p.entities.iter().any(|e| mine.contains(e));
    }
    Estate {
        as_of: snap.windows.now,
        days: snap.windows.days(),
        me: snap.me,
        mine: mine.into_iter().collect(),
        groups: Vec::new(),
        people: people(snap),
        providers: providers(snap, &models),
        flows: flows(snap),
        daily: daily(snap),
        workflows,
        routers,
        models,
        agents,
        tools: tools(snap),
        harnesses,
        problems,
        chains,
        opportunities,
        allowance: None,
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn success_pct(checked: i64, failed: i64) -> Option<f64> {
    (checked > 0).then(|| round2((checked - failed) as f64 * 100.0 / checked as f64))
}

/// Each agent's models in the range, by spend.
fn models_by_agent(snap: &Snapshot) -> HashMap<Uuid, Vec<String>> {
    let mut rows: Vec<_> = snap.agent_models.iter().collect();
    rows.sort_by(|a, b| b.spend.total_cmp(&a.spend).then(b.calls.cmp(&a.calls)));
    let mut out: HashMap<Uuid, Vec<String>> = HashMap::new();
    for r in rows {
        let list = out.entry(r.agent_id).or_default();
        if list.len() < MODELS_PER_NODE {
            list.push(format!("model:{}", r.model));
        }
    }
    out
}

/// The connectors each agent called in the range, plus any it was explicitly given.
fn tools_by_agent(snap: &Snapshot) -> HashMap<Uuid, Vec<Uuid>> {
    let from = snap.windows.range.from.date_naive();
    let mut out: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let called = snap
        .tool_calls
        .iter()
        .filter(|c| c.day >= from)
        .map(|c| (c.agent_id, c.connector_id));
    let given = snap
        .enabled_access
        .iter()
        .map(|a| (a.agent_id, a.connector_id));
    let visible: HashSet<Uuid> = snap.connectors.iter().map(|c| c.id).collect();
    for (agent, connector) in called.chain(given) {
        let list = out.entry(agent).or_default();
        if visible.contains(&connector) && !list.contains(&connector) {
            list.push(connector);
        }
    }
    out
}

fn agent(
    a: &AgentRow,
    usage: Option<&UsageStats>,
    models: &HashMap<Uuid, Vec<String>>,
    tools: &HashMap<Uuid, Vec<Uuid>>,
) -> Agent {
    let u = usage.cloned().unwrap_or_default();
    Agent {
        id: a.id,
        name: a.name.clone(),
        owner_id: a.owner_id,
        router_id: a.router_id,
        model_ids: models.get(&a.id).cloned().unwrap_or_default(),
        tool_ids: tools.get(&a.id).cloned().unwrap_or_default(),
        spend_usd: round2(u.spend),
        runs: u.runs,
        success_pct: success_pct(u.checked, u.failed),
        p95_ms: u.p95_ms.map(f64::round),
        // Deployed but not answering, or answering nothing in the range: drawn grey.
        idle: a.status != "running" || u.runs == 0,
        failed_spend_usd: round2(u.failed_spend),
    }
}

/// A harness's display name: the tool, and whose it is.
fn harness_label(kind: &str) -> &str {
    match kind {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        other => other,
    }
}

fn harness(
    snap: &Snapshot,
    h: &AgentRow,
    usage: Option<&UsageStats>,
    models: &HashMap<Uuid, Vec<String>>,
    tools: &HashMap<Uuid, Vec<Uuid>>,
) -> Harness {
    let kind = h.harness.clone().unwrap_or_default();
    let owner = snap
        .people
        .iter()
        .find(|p| p.id == h.owner_id)
        .and_then(|p| p.name.split_whitespace().next().map(str::to_owned));
    let sessions: Vec<_> = snap
        .sessions
        .iter()
        .filter(|s| s.agent_id == h.id)
        .collect();
    let typical_cost = median(sessions.iter().map(|s| s.cost).collect());
    let typical_latency = median(sessions.iter().map(|s| s.latency_ms as f64).collect());
    let u = usage.cloned().unwrap_or_default();
    Harness {
        id: h.id,
        name: match owner {
            Some(first) => format!("{} ({first})", harness_label(&kind)),
            None => harness_label(&kind).to_owned(),
        },
        harness: kind,
        owner_id: h.owner_id,
        model_ids: models.get(&h.id).cloned().unwrap_or_default(),
        tool_ids: tools.get(&h.id).cloned().unwrap_or_default(),
        // Every call unpriced: the spend is unknown, not zero.
        spend_usd: (u.runs == 0 || u.priced > 0).then(|| round2(u.spend)),
        sessions: sessions
            .iter()
            .map(|s| {
                session_outcome(
                    s.failed,
                    s.cost,
                    s.latency_ms as f64,
                    typical_cost,
                    typical_latency,
                )
            })
            .collect(),
    }
}

/// A session's outcome against its own harness's typical session: failed (an error span), costly
/// or slow (more than twice the typical one), else fine.
fn session_outcome(
    failed: bool,
    cost: f64,
    latency: f64,
    typical_cost: f64,
    typical_latency: f64,
) -> &'static str {
    if failed {
        "failed"
    } else if typical_cost > 0.0 && cost > 2.0 * typical_cost {
        "cost"
    } else if typical_latency > 0.0 && latency > 2.0 * typical_latency {
        "slow"
    } else {
        "ok"
    }
}

/// The models agents and harnesses called in the range. A model served by two providers counts
/// under the one that served most of its spend.
fn models(snap: &Snapshot) -> Vec<Model> {
    let mut by_name: BTreeMap<&str, Model> = BTreeMap::new();
    let mut rows: Vec<_> = snap.models_range.iter().collect();
    rows.sort_by(|a, b| b.spend.total_cmp(&a.spend));
    for m in rows {
        by_name
            .entry(&m.model)
            .and_modify(|x| {
                x.spend_usd = round2(x.spend_usd + m.spend);
                x.calls += m.calls;
            })
            .or_insert_with(|| Model {
                id: format!("model:{}", m.model),
                name: m.model.clone(),
                provider_id: format!("provider:{}", m.provider),
                spend_usd: round2(m.spend),
                calls: m.calls,
                p95_ms: m.p95_ms.map(f64::round),
            });
    }
    let mut out: Vec<Model> = by_name.into_values().collect();
    out.sort_by(|a, b| b.spend_usd.total_cmp(&a.spend_usd));
    out
}

/// A provider's display name and the brand mark the UI draws for it.
fn provider_name(key: &str) -> (String, Option<&'static str>) {
    match key {
        "anthropic" => ("Anthropic".into(), Some("anthropic")),
        "openai" | "azure-openai" => ("OpenAI".into(), Some("openai")),
        "gemini" | "google" | "vertex" => ("Google".into(), Some("gemini")),
        other => (other.to_owned(), None),
    }
}

fn providers(snap: &Snapshot, models: &[Model]) -> Vec<Provider> {
    let total: f64 = models.iter().map(|m| m.spend_usd).sum();
    let mut spend: BTreeMap<&str, f64> = BTreeMap::new();
    for m in models {
        *spend.entry(m.provider_id.as_str()).or_default() += m.spend_usd;
    }
    let custom: HashMap<&str, &str> = snap
        .provider_failures
        .iter()
        .map(|p| (p.label.as_str(), p.display_name.as_str()))
        .collect();
    spend
        .into_iter()
        .map(|(id, amount)| {
            let key = id.trim_start_matches("provider:");
            let (name, brand) = provider_name(key);
            Provider {
                id: id.to_owned(),
                name: custom.get(key).map_or(name, |n| (*n).to_owned()),
                brand,
                share_pct: if total > 0.0 {
                    (amount * 100.0 / total).round()
                } else {
                    0.0
                },
            }
        })
        .collect()
}

/// The configs agents route through, and the direct path coding harnesses take when any exist.
fn routers(snap: &Snapshot) -> Vec<Router> {
    let mut out: Vec<Router> = snap
        .routers
        .iter()
        .map(|r| {
            let fallbacks = r.fallback_models.0.len();
            Router {
                id: r.id.to_string(),
                name: r.name.clone(),
                direct: false,
                note: Some(match fallbacks {
                    0 => "No fallback".to_owned(),
                    1 => "1 fallback model".to_owned(),
                    n => format!("{n} fallback models"),
                }),
            }
        })
        .collect();
    if snap.agents.iter().any(AgentRow::is_harness) {
        out.push(Router {
            id: DIRECT_ROUTER.into(),
            name: "No router".into(),
            direct: true,
            note: Some("Coding harnesses call providers directly".into()),
        });
    }
    out
}

/// Brand marks the UI knows, by connector name.
fn tool_brand(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    ["github", "gmail", "slack", "linear", "jira"]
        .into_iter()
        .find(|b| n.contains(b))
}

/// Every visible connector; one turned off more than [`DISABLED_SHOWN_DAYS`] ago is left out.
/// When it was turned off isn't recorded: its last update is the closest fact.
fn tools(snap: &Snapshot) -> Vec<Tool> {
    let cutoff = snap.windows.now - Duration::days(DISABLED_SHOWN_DAYS);
    snap.connectors
        .iter()
        .filter(|c| c.is_active || c.updated_at >= cutoff)
        .map(|c| Tool {
            id: c.id,
            name: c.display_name.clone().unwrap_or_else(|| c.name.clone()),
            brand: tool_brand(&c.name),
            connector_id: (!c.toolkit).then_some(c.id),
            disabled_at: (!c.is_active).then_some(c.updated_at),
        })
        .collect()
}

fn workflows(snap: &Snapshot) -> Vec<Workflow> {
    snap.workflows
        .iter()
        .map(|w| Workflow {
            id: w.id,
            name: w.name.clone(),
            owner_id: w.owner_id,
            runs: snap
                .workflow_runs
                .iter()
                .filter(|r| r.maf_id == w.id)
                .map(|r| r.runs)
                .sum(),
            steps: w
                .steps
                .iter()
                .map(|s| Step {
                    kind: "agent",
                    r#ref: Some(s.agent_id),
                    label: s.agent_name.clone(),
                })
                .collect(),
        })
        .collect()
}

fn people(snap: &Snapshot) -> Vec<Person> {
    struct Totals {
        runs: i64,
        sessions: i64,
        spend: f64,
        input: i64,
        checked: i64,
        failed: i64,
    }
    let mut totals: HashMap<Uuid, Totals> = HashMap::new();
    for u in &snap.person_usage {
        let t = totals.entry(u.user_id).or_insert(Totals {
            runs: 0,
            sessions: 0,
            spend: 0.0,
            input: 0,
            checked: 0,
            failed: 0,
        });
        t.runs += u.runs;
        t.sessions += u.sessions;
        t.spend += u.spend;
        t.input += u.input_tokens;
        t.checked += u.checked;
        t.failed += u.failed;
    }
    let per_session = |t: &Totals| t.spend / t.sessions.max(1) as f64;
    let typical = median(
        totals
            .values()
            .filter(|t| t.sessions >= LIGHT_SESSIONS)
            .map(per_session)
            .collect(),
    );
    let working: HashMap<Uuid, Uuid> = snap
        .working_on
        .iter()
        .map(|w| (w.user_id, w.agent_id))
        .collect();
    let on_map: HashSet<Uuid> = snap.agents.iter().map(|a| a.id).collect();
    snap.people
        .iter()
        .map(|p| {
            let t = totals.get(&p.id);
            let sessions = t.map_or(0, |t| t.sessions);
            let success = t.and_then(|t| success_pct(t.checked, t.failed));
            let segment = match t {
                _ if sessions < LIGHT_SESSIONS => "light",
                _ if success.is_some_and(|s| s < STUCK_SUCCESS_PCT) => "stuck",
                Some(t) if typical > 0.0 && per_session(t) > EXPENSIVE_FACTOR * typical => {
                    "expensive"
                }
                _ => "value",
            };
            Person {
                id: p.id,
                name: p.name.clone(),
                group_id: None,
                spend_usd: round2(t.map_or(0.0, |t| t.spend)),
                sessions,
                success_pct: success,
                avg_context_tokens: t
                    .filter(|t| t.runs > 0)
                    .map(|t| (t.input as f64 / t.runs as f64).round()),
                segment,
                working_on: working
                    .get(&p.id)
                    .filter(|id| on_map.contains(id))
                    .map(|id| WorkingOn {
                        kind: "agent",
                        id: *id,
                    }),
            }
        })
        .collect()
}

/// Traffic between the map's cards over the range. A flow with nothing on it is left out.
fn flows(snap: &Snapshot) -> Vec<Flow> {
    let harness: HashSet<Uuid> = snap
        .agents
        .iter()
        .filter(|a| a.is_harness())
        .map(|a| a.id)
        .collect();
    let is_harness = |id: &Uuid| harness.contains(id);
    let from = snap.windows.range.from.date_naive();

    let (agent_runs, agent_spend) = sum(snap
        .person_usage
        .iter()
        .filter(|u| !is_harness(&u.agent_id))
        .map(|u| (u.runs, u.spend)));
    let (harness_sessions, harness_spend) = sum(snap
        .person_usage
        .iter()
        .filter(|u| is_harness(&u.agent_id))
        .map(|u| (u.sessions, u.spend)));
    let (workflow_runs, workflow_spend) = sum(snap.workflow_runs.iter().map(|r| (r.runs, r.cost)));
    let workflow_steps: i64 = snap.steps_range.iter().map(|s| s.steps).sum();
    let (agent_calls, agent_model_spend) = sum(snap
        .agent_models
        .iter()
        .filter(|m| !is_harness(&m.agent_id))
        .map(|m| (m.calls, m.spend)));
    let (harness_calls, harness_model_spend) = sum(snap
        .agent_models
        .iter()
        .filter(|m| is_harness(&m.agent_id))
        .map(|m| (m.calls, m.spend)));
    let in_range = || snap.tool_calls.iter().filter(|c| c.day >= from);
    let agent_tools: i64 = in_range()
        .filter(|c| !is_harness(&c.agent_id))
        .map(|c| c.calls)
        .sum();
    let harness_tools: i64 = in_range()
        .filter(|c| is_harness(&c.agent_id))
        .map(|c| c.calls)
        .sum();

    [
        (
            "people-workflows",
            "people",
            "workflows",
            workflow_runs,
            "workflow runs",
            Some(workflow_spend),
            false,
        ),
        (
            "people-agents",
            "people",
            "agents",
            agent_runs,
            "agent runs",
            Some(agent_spend),
            false,
        ),
        (
            "people-harnesses",
            "people",
            "harnesses",
            harness_sessions,
            "harness sessions",
            Some(harness_spend),
            false,
        ),
        (
            "workflows-agents",
            "workflows",
            "agents",
            workflow_steps,
            "workflow steps",
            None,
            false,
        ),
        (
            "agents-models",
            "agents",
            "models",
            agent_calls,
            "LLM calls",
            Some(agent_model_spend),
            false,
        ),
        (
            "harnesses-models",
            "harnesses",
            "models",
            harness_calls,
            "LLM calls",
            Some(harness_model_spend),
            true,
        ),
        (
            "agents-tools",
            "agents",
            "tools",
            agent_tools,
            "tool calls",
            None,
            false,
        ),
        (
            "harnesses-tools",
            "harnesses",
            "tools",
            harness_tools,
            "tool calls",
            None,
            true,
        ),
    ]
    .into_iter()
    .filter(|f| f.3 > 0)
    .map(|(id, from, to, volume, unit, spend, direct)| Flow {
        id,
        from,
        to,
        volume,
        unit,
        spend_usd: spend.map(round2),
        direct,
    })
    .collect()
}

fn sum(rows: impl Iterator<Item = (i64, f64)>) -> (i64, f64) {
    rows.fold((0, 0.0), |(n, s), (n2, s2)| (n + n2, s + s2))
}

/// The meters' trend lines, one row per history day. Without budgets every dollar is
/// unbudgeted; what optimizations saved is measured.
fn daily(snap: &Snapshot) -> Vec<Daily> {
    let spent: HashMap<NaiveDate, f64> =
        snap.spend_daily.iter().map(|d| (d.day, d.amount)).collect();
    let saved: HashMap<NaiveDate, f64> =
        snap.saved_daily.iter().map(|d| (d.day, d.amount)).collect();
    snap.windows
        .days()
        .iter()
        .map(|day| Daily {
            budgeted_usd: 0.0,
            unbudgeted_usd: round2(spent.get(day).copied().unwrap_or(0.0)),
            uncapped_usd: 0.0,
            saved_usd: round2(saved.get(day).copied().unwrap_or(0.0)),
        })
        .collect()
}

/// What the viewer owns, and everything those touch: Mine ghosts the rest.
fn mine(
    snap: &Snapshot,
    agents: &[Agent],
    harnesses: &[Harness],
    models: &[Model],
) -> HashSet<String> {
    let me = snap.me;
    let provider_of: HashMap<&str, &str> = models
        .iter()
        .map(|m| (m.id.as_str(), m.provider_id.as_str()))
        .collect();
    let mut out: HashSet<String> = HashSet::from([me.to_string()]);
    let touch = |models: &[String], tools: &[Uuid], out: &mut HashSet<String>| {
        for m in models {
            out.insert(m.clone());
            if let Some(p) = provider_of.get(m.as_str()) {
                out.insert((*p).to_owned());
            }
        }
        out.extend(tools.iter().map(Uuid::to_string));
    };
    for a in agents.iter().filter(|a| a.owner_id == me) {
        out.insert(a.id.to_string());
        if let Some(r) = a.router_id {
            out.insert(r.to_string());
        }
        touch(&a.model_ids, &a.tool_ids, &mut out);
    }
    for h in harnesses.iter().filter(|h| h.owner_id == me) {
        out.insert(h.id.to_string());
        out.insert(DIRECT_ROUTER.to_owned());
        touch(&h.model_ids, &h.tool_ids, &mut out);
    }
    out.extend(
        snap.workflows
            .iter()
            .filter(|w| w.owner_id == me)
            .map(|w| w.id.to_string()),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_failed_before_it_is_costly_or_slow() {
        assert_eq!(session_outcome(true, 10.0, 10.0, 1.0, 1.0), "failed");
        assert_eq!(session_outcome(false, 3.0, 3.0, 1.0, 1.0), "cost");
        assert_eq!(session_outcome(false, 1.0, 3.0, 1.0, 1.0), "slow");
        assert_eq!(session_outcome(false, 1.5, 1.5, 1.0, 1.0), "ok");
        // No typical session yet (nothing priced or timed): nothing reads as costly or slow.
        assert_eq!(session_outcome(false, 5.0, 5.0, 0.0, 0.0), "ok");
    }

    #[test]
    fn providers_get_the_brand_the_ui_draws() {
        assert_eq!(
            provider_name("anthropic"),
            ("Anthropic".to_owned(), Some("anthropic"))
        );
        assert_eq!(provider_name("gemini").1, Some("gemini"));
        assert_eq!(provider_name("my-vllm"), ("my-vllm".to_owned(), None));
    }

    #[test]
    fn tool_brands_come_from_the_connector_name() {
        assert_eq!(tool_brand("jira-cloud"), Some("jira"));
        assert_eq!(tool_brand("GitHub"), Some("github"));
        assert_eq!(tool_brand("finance-ledger"), None);
    }
}
