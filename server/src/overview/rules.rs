//! Problems, cause chains and opportunities, found in a [`Snapshot`]. Pure.
//!
//! Every rule compares a thing with its own recent past (the last 7 days against the 7 before, or
//! its own 30-day baseline), never with a fleet-wide bar — the same stance as the Overview's fleet
//! ratings (`ui/common/src/features/overview/health.ts`), whose thresholds these mirror. A rule
//! only fires on enough data ([`MIN_BASE`]) to say so, and quotes the numbers it fired on.
//!
//! A problem's id is `<rule>:<entity id>`: stable across reads, so the decisions people record
//! against it (`overview_problem_state`) find it again while it lasts.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::load::{AgentDay, AgentRow, Snapshot, UsageStats};

/// Comparisons need this many runs (or calls) in the base window, so a jump on a handful of
/// runs never raises a problem (the fleet ratings' `MIN_BASE_OPS`).
const MIN_BASE: i64 = 20;
/// A week's spend this much above the week before is a cost problem.
const COST_RISE_PCT: f64 = 50.0;
/// Cost per run this many times the 30-day average is a cost problem.
const COST_PER_RUN_FACTOR: f64 = 2.0;
/// A day this many times the agent's typical (median) day, within the last week, is a spike.
const SPIKE_FACTOR: f64 = 3.0;
/// A typical day needs this many days with spend.
const MIN_SPIKE_DAYS: usize = 3;
/// p95 latency this much above the week before is a slow problem.
const P95_RISE_PCT: f64 = 50.0;
/// A deploy that hasn't finished in this long is stuck (the agents page's watch cap).
const STUCK_DEPLOY_MINUTES: i64 = 3;
/// A week with this share of runs failing is broken.
const FAILING_BROKEN_PCT: f64 = 20.0;
/// A week with this share failing, at least twice the week before, is getting worse.
const FAILING_WORSE_PCT: f64 = 5.0;
/// The same two bars for an MCP tool's calls: tools fail less often than agents do.
const TOOL_BROKEN_PCT: f64 = 10.0;
const TOOL_WORSE_PCT: f64 = 3.0;
/// Workflows run far less often than agents: fewer runs make a rate.
const MIN_WORKFLOW_RUNS: i64 = 5;
/// A coding harness with this share of its latest sessions failed is getting worse.
const HARNESS_FAILED_PCT: f64 = 20.0;
const MIN_SESSIONS: usize = 10;
/// Days in a problem's trend line.
const TREND_DAYS: usize = 10;
/// An opportunity worth less than this a month isn't shown.
const MIN_SAVING_USD: f64 = 0.5;
/// Prompt caching: input over 30 days below this is too little to tune.
const MIN_CACHE_TOKENS: i64 = 1_000_000;
/// Prompt caching: a share this far below the fleet's best quarter is an opportunity.
const CACHE_GAP: f64 = 0.2;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(super) enum Kind {
    Errors,
    Slow,
    Cost,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(super) enum Tier {
    Broken,
    Worse,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(super) enum Confidence {
    /// Measured: the evidence is the thing itself failing.
    High,
    /// Inferred: by configuration or timing.
    Medium,
}

/// A page the UI knows how to open (`panel/targets.tsx`), never a URL.
#[derive(Serialize, Clone, Debug)]
pub(super) struct Target {
    page: &'static str,
    id: Option<String>,
    query: Option<BTreeMap<&'static str, String>>,
}

impl Target {
    fn page(page: &'static str) -> Self {
        Target {
            page,
            id: None,
            query: None,
        }
    }
    fn of(page: &'static str, id: impl ToString) -> Self {
        Target {
            page,
            id: Some(id.to_string()),
            query: None,
        }
    }
    fn with(mut self, key: &'static str, value: &str) -> Self {
        self.query
            .get_or_insert_with(BTreeMap::new)
            .insert(key, value.to_owned());
        self
    }
}

#[derive(Serialize, Clone, Debug)]
pub(super) struct Action {
    label: String,
    target: Target,
}

fn action(label: &str, target: Target) -> Action {
    Action {
        label: label.to_owned(),
        target,
    }
}

#[derive(Serialize, Clone, Debug)]
pub(super) struct Metric {
    label: &'static str,
    current: f64,
    target: f64,
    direction: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
}

impl Metric {
    fn higher(
        label: &'static str,
        current: f64,
        target: f64,
        unit: &'static str,
        max: f64,
    ) -> Self {
        Metric {
            label,
            current,
            target,
            direction: "higher",
            unit,
            min: 0.0,
            max,
        }
    }
    fn lower(label: &'static str, current: f64, target: f64, unit: &'static str, max: f64) -> Self {
        Metric {
            label,
            current,
            target,
            direction: "lower",
            unit,
            min: 0.0,
            max,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub(super) struct Problem {
    pub id: String,
    pub kind: Kind,
    also: Option<Kind>,
    pub tier: Tier,
    pub title: String,
    why: String,
    metric: Metric,
    trend: Vec<f64>,
    since: NaiveDate,
    confidence: Confidence,
    evidence: Vec<String>,
    fixed_when: String,
    action: Action,
    /// The node the problem is about first, then everything it involves.
    pub entities: Vec<String>,
    flow_id: Option<&'static str>,
    count: i64,
    pub chain_id: Option<String>,
    owner_id: Option<Uuid>,
    snoozed_until: Option<DateTime<Utc>>,
    handled_at: Option<DateTime<Utc>>,
    pub score: f64,
    pub mine: bool,
}

#[derive(Serialize, Clone, Debug)]
pub(super) struct ChainStep {
    entity: String,
    what: String,
    detail: String,
}

#[derive(Serialize, Clone, Debug)]
pub(super) struct Chain {
    pub id: String,
    title: String,
    confidence: Confidence,
    steps: Vec<ChainStep>,
    fix: String,
    /// How many problems the fix clears.
    clears: i64,
    action: Action,
    /// The problems it links (each carries this chain's id).
    #[serde(skip)]
    members: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub(super) struct Opportunity {
    pub id: String,
    title: String,
    detail: String,
    /// A month's saving, in USD.
    pub saving_usd: f64,
    method: String,
    effort: &'static str,
    entities: Vec<String>,
    action: Action,
}

pub(super) struct Findings {
    pub problems: Vec<Problem>,
    pub chains: Vec<Chain>,
    pub opportunities: Vec<Opportunity>,
}

/// What someone decided about a problem (OV-2).
#[derive(sqlx::FromRow, Clone, Default, Debug)]
pub(super) struct Decision {
    pub problem_id: String,
    pub owner_id: Option<Uuid>,
    pub snoozed_until: Option<DateTime<Utc>>,
    pub handled_at: Option<DateTime<Utc>>,
}

/// One OV-2 change. `None` clears the owner or the snooze.
pub(super) enum Change {
    Owner(Option<Uuid>),
    Snooze(Option<DateTime<Utc>>),
    Handled(bool),
}

impl Decision {
    pub(super) fn new(problem_id: &str) -> Self {
        Decision {
            problem_id: problem_id.to_owned(),
            ..Default::default()
        }
    }

    pub(super) fn with(mut self, change: Change, now: DateTime<Utc>) -> Self {
        match change {
            Change::Owner(owner) => self.owner_id = owner,
            Change::Snooze(until) => self.snoozed_until = until,
            Change::Handled(true) => self.handled_at = Some(now),
            Change::Handled(false) => self.handled_at = None,
        }
        self
    }
}

/// Puts each problem's recorded decision on it. A snooze that has run out is gone, and "handled"
/// holds only for the UTC day it was marked: a problem still found the next day shows again.
pub(super) fn apply_decisions(
    problems: &mut [Problem],
    decisions: &[Decision],
    now: DateTime<Utc>,
) {
    let by_id: HashMap<&str, &Decision> = decisions
        .iter()
        .map(|d| (d.problem_id.as_str(), d))
        .collect();
    for p in problems {
        let Some(d) = by_id.get(p.id.as_str()) else {
            continue;
        };
        p.owner_id = d.owner_id;
        p.snoozed_until = d.snoozed_until.filter(|until| *until > now);
        p.handled_at = d
            .handled_at
            .filter(|at| at.date_naive() == now.date_naive());
    }
}

/// Every problem, cause chain and opportunity in `snap`, problems most urgent first.
pub(super) fn detect(snap: &Snapshot) -> Findings {
    let facts = Facts::new(snap);
    let mut problems = Vec::new();
    for agent in snap.agents.iter().filter(|a| !a.is_harness()) {
        problems.extend(agent_problems(&facts, agent));
    }
    for harness in snap.agents.iter().filter(|a| a.is_harness()) {
        problems.extend(harness_problems(&facts, harness));
    }
    problems.extend(tool_problems(&facts));
    problems.extend(workflow_problems(&facts));
    problems.extend(model_problems(&facts));
    let chains = link_chains(&facts, &mut problems);
    problems.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    Findings {
        problems,
        chains,
        opportunities: opportunities(&facts),
    }
}

// ── Lookups ──────────────────────────────────────────────────────────────────

/// `snap` indexed the ways the rules read it.
struct Facts<'a> {
    snap: &'a Snapshot,
    days: Vec<NaiveDate>,
    today: NaiveDate,
    week: HashMap<Uuid, &'a UsageStats>,
    prev_week: HashMap<Uuid, &'a UsageStats>,
    month: HashMap<Uuid, &'a UsageStats>,
    daily: HashMap<(Uuid, NaiveDate), &'a AgentDay>,
    agent_names: HashMap<Uuid, &'a str>,
}

impl<'a> Facts<'a> {
    fn new(snap: &'a Snapshot) -> Self {
        let index = |rows: &'a [super::load::AgentUsage]| {
            rows.iter()
                .map(|r| (r.agent_id, &r.stats))
                .collect::<HashMap<_, _>>()
        };
        Facts {
            snap,
            days: snap.windows.days(),
            today: snap.windows.now.date_naive(),
            week: index(&snap.usage_week),
            prev_week: index(&snap.usage_prev_week),
            month: index(&snap.usage_month),
            daily: snap
                .usage_daily
                .iter()
                .map(|d| ((d.agent_id, d.day), d))
                .collect(),
            agent_names: snap
                .agents
                .iter()
                .map(|a| (a.id, a.name.as_str()))
                .collect(),
        }
    }

    fn agent_name(&self, id: Uuid) -> &str {
        self.agent_names.get(&id).copied().unwrap_or("An agent")
    }

    /// The last [`TREND_DAYS`] days of one agent's daily figure, oldest first; a day without
    /// runs repeats the day before (or `quiet` before any).
    fn agent_trend(
        &self,
        id: Uuid,
        quiet: f64,
        value: impl Fn(&AgentDay) -> Option<f64>,
    ) -> Vec<f64> {
        trend(&self.days, quiet, |day| {
            self.daily.get(&(id, day)).and_then(|d| value(d))
        })
    }

    /// A week ago, as a date: where a problem with no clearer start began.
    fn week_ago(&self) -> NaiveDate {
        self.today - Duration::days(6)
    }
}

/// The last [`TREND_DAYS`] values of a daily series, oldest first; a day with no value repeats
/// the one before it (or `quiet` before any).
fn trend(days: &[NaiveDate], quiet: f64, value: impl Fn(NaiveDate) -> Option<f64>) -> Vec<f64> {
    let mut last = quiet;
    let all: Vec<f64> = days
        .iter()
        .map(|d| {
            if let Some(v) = value(*d) {
                last = v;
            }
            round1(last)
        })
        .collect();
    all[all.len().saturating_sub(TREND_DAYS)..].to_vec()
}

/// The first day of the run of bad days that reaches today. A day with no data neither starts
/// nor ends the run; with no bad day at all, `fallback`.
fn since(
    days: &[NaiveDate],
    fallback: NaiveDate,
    bad: impl Fn(NaiveDate) -> Option<bool>,
) -> NaiveDate {
    let mut start = None;
    for day in days.iter().rev() {
        match bad(*day) {
            Some(true) => start = Some(*day),
            Some(false) => break,
            None => {}
        }
    }
    start.unwrap_or(fallback)
}

fn pct_change(now: f64, before: f64) -> f64 {
    (now - before) / before * 100.0
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn short_date(d: NaiveDate) -> String {
    d.format("%b %-d").to_string()
}

fn money(x: f64) -> String {
    format!("${x:.2}")
}

/// Tier, impact, urgency and confidence in one number: the order of every list.
fn score(
    tier: Tier,
    count: i64,
    spend: f64,
    since: NaiveDate,
    today: NaiveDate,
    c: Confidence,
) -> f64 {
    let base = match tier {
        Tier::Broken => 100.0,
        Tier::Worse => 60.0,
    };
    let reach = (10.0 * (1.0 + count.max(0) as f64).log10()).min(30.0);
    let money = (5.0 * (1.0 + spend.max(0.0)).log10()).min(15.0);
    let fresh = if (today - since).num_days() <= 2 {
        5.0
    } else {
        0.0
    };
    let sure = match c {
        Confidence::High => 5.0,
        Confidence::Medium => 0.0,
    };
    round1(base + reach + money + fresh + sure)
}

/// A problem's fields that every rule fills the same way.
struct Draft {
    id: String,
    kind: Kind,
    tier: Tier,
    title: String,
    why: String,
    metric: Metric,
    trend: Vec<f64>,
    since: NaiveDate,
    confidence: Confidence,
    evidence: Vec<String>,
    fixed_when: String,
    action: Action,
    entities: Vec<String>,
    flow_id: &'static str,
    count: i64,
    spend: f64,
}

impl Draft {
    fn into_problem(self, today: NaiveDate) -> Problem {
        Problem {
            score: score(
                self.tier,
                self.count,
                self.spend,
                self.since,
                today,
                self.confidence,
            ),
            id: self.id,
            kind: self.kind,
            also: None,
            tier: self.tier,
            title: self.title,
            why: self.why,
            metric: self.metric,
            trend: self.trend,
            since: self.since,
            confidence: self.confidence,
            evidence: self.evidence,
            fixed_when: self.fixed_when,
            action: self.action,
            entities: self.entities,
            flow_id: Some(self.flow_id),
            count: self.count,
            chain_id: None,
            owner_id: None,
            snoozed_until: None,
            handled_at: None,
            mine: false,
        }
    }
}

// ── Agents ───────────────────────────────────────────────────────────────────

fn agent_problems(f: &Facts, a: &AgentRow) -> Vec<Problem> {
    let mut out = Vec::new();
    let down = agent_down(f, a);
    let down_now = down.is_some();
    out.extend(down);
    if !down_now {
        out.extend(agent_failed_build(f, a));
        out.extend(agent_stuck(f, a));
        out.extend(agent_failing(f, a));
    }
    out.extend(agent_slow(f, a));
    out.extend(agent_cost(f, a));
    out
}

/// The agent the agent's problems are about, then its models and tools: what the map lights.
fn agent_entities(f: &Facts, id: Uuid) -> Vec<String> {
    let mut models: Vec<_> = f
        .snap
        .agent_models
        .iter()
        .filter(|m| m.agent_id == id)
        .collect();
    models.sort_by(|x, y| y.spend.total_cmp(&x.spend));
    let mut out = vec![id.to_string()];
    out.extend(models.iter().take(2).map(|m| format!("model:{}", m.model)));
    out
}

fn runs_trend(f: &Facts, id: Uuid) -> Vec<f64> {
    f.agent_trend(id, 0.0, |d| Some(d.runs as f64))
}

/// Crashed or failed now (the catalog's own reconciled status).
fn agent_down(f: &Facts, a: &AgentRow) -> Option<Problem> {
    let crashed = a.status == "crashed";
    if !crashed && a.status != "failed" {
        return None;
    }
    let deployment = f.snap.deployments.iter().find(|d| d.agent_id == a.id);
    let build = f.snap.failed_builds.iter().find(|b| b.agent_id == a.id);
    let crashed_at = deployment.and_then(|d| d.crashed_at);
    let restarts = deployment.map_or(0, |d| d.restart_count);
    let since = crashed_at.unwrap_or(a.updated_at).date_naive();
    let title = match (crashed, restarts) {
        (true, n) if n > 1 => format!(
            "{} has crashed {n} times since {}",
            a.name,
            short_date(since)
        ),
        (true, _) => format!("{} has crashed", a.name),
        (false, _) => format!("{} failed to start", a.name),
    };
    let mut evidence = vec![format!("Status {} since {}", a.status, short_date(since))];
    if let Some(reason) = deployment.and_then(|d| d.crash_reason.as_deref()) {
        evidence.push(format!("Crash reason: {reason}"));
    }
    if restarts > 0 {
        evidence.push(format!(
            "Restarted {restarts} times on its current deployment"
        ));
    }
    if let Some(error) = build.and_then(|b| b.error_msg.as_deref()) {
        evidence.push(format!("Last build failed: {}", clip(error)));
    }
    let week = f.week.get(&a.id);
    Some(
        Draft {
            id: format!("agent-down:{}", a.id),
            kind: Kind::Errors,
            tier: Tier::Broken,
            title,
            why: "Nothing it serves gets an answer until it runs again.".into(),
            metric: Metric::higher("Running", 0.0, 1.0, "", 1.0),
            trend: runs_trend(f, a.id),
            since,
            confidence: Confidence::High,
            evidence,
            fixed_when: "Running and answering again".into(),
            action: action("Open the agent", Target::of("agent", a.id)),
            entities: agent_entities(f, a.id),
            flow_id: "people-agents",
            count: i64::from(restarts.max(1)),
            spend: week.map_or(0.0, |w| w.spend),
        }
        .into_problem(f.today),
    )
}

/// Its latest build failed: the newest version never deployed.
fn agent_failed_build(f: &Facts, a: &AgentRow) -> Option<Problem> {
    let build = f.snap.failed_builds.iter().find(|b| b.agent_id == a.id)?;
    let since = build.failed_at.date_naive();
    Some(
        Draft {
            id: format!("agent-build:{}", a.id),
            kind: Kind::Errors,
            tier: Tier::Broken,
            title: format!("{}'s latest build failed", a.name),
            why: "Its newest version never deployed: it still runs the one before, if any.".into(),
            metric: Metric::lower("Failed builds", 1.0, 0.0, "", 1.0),
            trend: runs_trend(f, a.id),
            since,
            confidence: Confidence::High,
            evidence: vec![format!(
                "Build failed on {}: {}",
                short_date(since),
                clip(build.error_msg.as_deref().unwrap_or("no error recorded"))
            )],
            fixed_when: "A build of it succeeds".into(),
            action: action(
                "Open failed builds",
                Target::page("builds").with("status", "failed"),
            ),
            entities: agent_entities(f, a.id),
            flow_id: "people-agents",
            count: 1,
            spend: 0.0,
        }
        .into_problem(f.today),
    )
}

fn agent_stuck(f: &Facts, a: &AgentRow) -> Option<Problem> {
    let waited = f.snap.windows.now - a.updated_at;
    if a.status != "deploying" || waited < Duration::minutes(STUCK_DEPLOY_MINUTES) {
        return None;
    }
    let minutes = waited.num_minutes();
    Some(
        Draft {
            id: format!("agent-stuck:{}", a.id),
            kind: Kind::Errors,
            tier: Tier::Broken,
            title: format!("{} has been deploying for {minutes} min", a.name),
            why: "A deploy normally finishes in a minute or two; this one may never start.".into(),
            metric: Metric::lower(
                "Minutes deploying",
                minutes as f64,
                3.0,
                "min",
                (minutes as f64).max(10.0),
            ),
            trend: runs_trend(f, a.id),
            since: a.updated_at.date_naive(),
            confidence: Confidence::High,
            evidence: vec![format!(
                "Deploying since {}",
                a.updated_at.format("%b %-d %H:%M UTC")
            )],
            fixed_when: "Running, or failed with a reason".into(),
            action: action("Open the agent", Target::of("agent", a.id)),
            entities: agent_entities(f, a.id),
            flow_id: "people-agents",
            count: 1,
            spend: 0.0,
        }
        .into_problem(f.today),
    )
}

/// The tier a failure rate earns: broken over the bar, worse when it at least doubled.
fn failing_tier(pct: f64, before: Option<f64>, broken: f64, worse: f64) -> Option<Tier> {
    if pct >= broken {
        Some(Tier::Broken)
    } else if pct >= worse && before.is_some_and(|b| pct >= 2.0 * b) {
        Some(Tier::Worse)
    } else {
        None
    }
}

fn agent_failing(f: &Facts, a: &AgentRow) -> Option<Problem> {
    let week = f.week.get(&a.id).filter(|w| w.checked >= MIN_BASE)?;
    let pct = week.failed_pct()?;
    let before = f
        .prev_week
        .get(&a.id)
        .filter(|p| p.checked >= MIN_BASE)
        .and_then(|p| p.failed_pct());
    let tier = failing_tier(pct, before, FAILING_BROKEN_PCT, FAILING_WORSE_PCT)?;
    let day_pct =
        |d: &AgentDay| (d.checked > 0).then(|| d.failed as f64 * 100.0 / d.checked as f64);
    let since = since(&f.days, f.week_ago(), |day| {
        f.daily
            .get(&(a.id, day))
            .and_then(|d| day_pct(d))
            .map(|p| p >= FAILING_WORSE_PCT)
    });
    let mut evidence = vec![format!(
        "{} of {} checked runs failed in the last 7 days",
        week.failed, week.checked
    )];
    if let Some(b) = before {
        evidence.push(format!("{b:.0}% failed the week before"));
    }
    if week.failed_spend > 0.0 {
        evidence.push(format!(
            "{} spent on the failed runs",
            money(week.failed_spend)
        ));
    }
    Some(
        Draft {
            id: format!("agent-failing:{}", a.id),
            kind: Kind::Errors,
            tier,
            title: format!("{} fails {pct:.0}% of runs", a.name),
            why: format!(
                "{} of its runs this week ended in an error; whoever called it got no answer.",
                week.failed
            ),
            metric: Metric::higher("Success rate", round1(100.0 - pct), 95.0, "%", 100.0),
            trend: f.agent_trend(a.id, 100.0, |d| day_pct(d).map(|p| 100.0 - p)),
            since,
            confidence: Confidence::High,
            evidence,
            fixed_when: format!("Under {FAILING_WORSE_PCT:.0}% of runs failing for a day"),
            action: action("Open the agent", Target::of("agent", a.id)),
            entities: agent_entities(f, a.id),
            flow_id: "people-agents",
            count: week.failed,
            spend: week.failed_spend,
        }
        .into_problem(f.today),
    )
}

fn agent_slow(f: &Facts, a: &AgentRow) -> Option<Problem> {
    let week = f.week.get(&a.id)?;
    let prev = f.prev_week.get(&a.id).filter(|p| p.runs >= MIN_BASE)?;
    let (now_ms, before_ms) = (week.p95_ms?, prev.p95_ms?);
    if before_ms <= 0.0 || week.runs < MIN_BASE {
        return None;
    }
    let rise = pct_change(now_ms, before_ms);
    if rise <= P95_RISE_PCT {
        return None;
    }
    let bar = before_ms * (1.0 + P95_RISE_PCT / 100.0);
    let since = since(&f.days, f.week_ago(), |day| {
        f.daily
            .get(&(a.id, day))
            .and_then(|d| d.p95_ms)
            .map(|p| p > bar)
    });
    let secs = |ms: f64| round1(ms / 1000.0);
    Some(
        Draft {
            id: format!("agent-slow:{}", a.id),
            kind: Kind::Slow,
            tier: Tier::Worse,
            title: format!("{} got {rise:.0}% slower", a.name),
            why: format!(
                "Its slowest runs take {}s now, {}s the week before.",
                secs(now_ms),
                secs(before_ms)
            ),
            metric: Metric::lower(
                "p95 latency",
                secs(now_ms),
                secs(before_ms),
                "s",
                secs(now_ms * 1.5),
            ),
            trend: f.agent_trend(a.id, secs(before_ms), |d| d.p95_ms.map(secs)),
            since,
            confidence: Confidence::High,
            evidence: vec![format!(
                "p95 {}s over {} runs in the last 7 days, {}s over {} the week before",
                secs(now_ms),
                week.runs,
                secs(before_ms),
                prev.runs
            )],
            fixed_when: format!("p95 back under {}s for a day", secs(bar)),
            action: action("Open the agent", Target::of("agent", a.id)),
            entities: agent_entities(f, a.id),
            flow_id: "agents-models",
            count: week.runs,
            spend: week.spend,
        }
        .into_problem(f.today),
    )
}

/// One cost problem per agent: a spike day, else a costlier week, else costlier runs.
fn agent_cost(f: &Facts, a: &AgentRow) -> Option<Problem> {
    spike(f, a)
        .or_else(|| cost_rise(f, a.id, &a.name, "people-agents", Target::of("agent", a.id)))
        .or_else(|| cost_per_run(f, a))
}

fn spike(f: &Facts, a: &AgentRow) -> Option<Problem> {
    let spends: Vec<(NaiveDate, f64)> = f
        .days
        .iter()
        .filter_map(|day| f.daily.get(&(a.id, *day)).map(|d| (*day, d.spend)))
        .filter(|(_, s)| *s > 0.0)
        .collect();
    if spends.len() < MIN_SPIKE_DAYS {
        return None;
    }
    let typical = median(spends.iter().map(|(_, s)| *s).collect());
    let (day, spend) = spends
        .iter()
        .filter(|(day, _)| *day >= f.week_ago())
        .copied()
        .max_by(|x, y| x.1.total_cmp(&y.1))?;
    let factor = spend / typical;
    if typical <= 0.0 || factor < SPIKE_FACTOR {
        return None;
    }
    Some(
        Draft {
            id: format!("agent-cost:{}", a.id),
            kind: Kind::Cost,
            tier: Tier::Broken,
            title: format!(
                "{} spent {factor:.1}× a typical day on {}",
                a.name,
                short_date(day)
            ),
            why: format!(
                "{} that day against a typical {}.",
                money(spend),
                money(typical)
            ),
            metric: Metric::lower(
                "Spend that day",
                round2(spend),
                round2(typical),
                "$",
                round2(spend * 1.2),
            ),
            trend: f.agent_trend(a.id, 0.0, |d| Some(round2(d.spend))),
            since: day,
            confidence: Confidence::High,
            evidence: vec![format!(
                "Median day over the last {} days with spend: {}",
                spends.len(),
                money(typical)
            )],
            fixed_when: "No day above 3× a typical one for a week".into(),
            action: action(
                "Open that day in TokenOps",
                Target::page("tokenops").with("day", &day.to_string()),
            ),
            entities: agent_entities(f, a.id),
            flow_id: "agents-models",
            count: f.daily.get(&(a.id, day)).map_or(0, |d| d.runs),
            spend,
        }
        .into_problem(f.today),
    )
}

/// A week's spend well above the week before, on enough runs to compare.
fn cost_rise(
    f: &Facts,
    id: Uuid,
    name: &str,
    flow: &'static str,
    target: Target,
) -> Option<Problem> {
    let week = f.week.get(&id)?;
    let prev = f
        .prev_week
        .get(&id)
        .filter(|p| p.runs >= MIN_BASE && p.spend > 0.0)?;
    let rise = pct_change(week.spend, prev.spend);
    if rise <= COST_RISE_PCT {
        return None;
    }
    Some(
        Draft {
            id: format!("agent-cost:{id}"),
            kind: Kind::Cost,
            tier: Tier::Worse,
            title: format!("{name} costs {rise:.0}% more than last week"),
            why: format!(
                "{} in the last 7 days against {} the week before.",
                money(week.spend),
                money(prev.spend)
            ),
            metric: Metric::lower(
                "Spend this week",
                round2(week.spend),
                round2(prev.spend),
                "$",
                round2(week.spend * 1.2),
            ),
            trend: f.agent_trend(id, 0.0, |d| Some(round2(d.spend))),
            since: f.week_ago(),
            confidence: Confidence::High,
            evidence: vec![format!(
                "{} runs this week, {} the week before",
                week.runs, prev.runs
            )],
            fixed_when: "A week within 50% of the one before".into(),
            action: action("Open the spend", target),
            entities: agent_entities(f, id),
            flow_id: flow,
            count: week.runs,
            spend: week.spend - prev.spend,
        }
        .into_problem(f.today),
    )
}

fn cost_per_run(f: &Facts, a: &AgentRow) -> Option<Problem> {
    let week = f.week.get(&a.id).filter(|w| w.runs >= MIN_BASE)?;
    let month = f.month.get(&a.id).filter(|m| m.runs > 0 && m.spend > 0.0)?;
    let (now, usual) = (
        week.spend / week.runs as f64,
        month.spend / month.runs as f64,
    );
    let factor = now / usual;
    if factor <= COST_PER_RUN_FACTOR {
        return None;
    }
    Some(
        Draft {
            id: format!("agent-cost:{}", a.id),
            kind: Kind::Cost,
            tier: Tier::Worse,
            title: format!("{}'s runs cost {factor:.1}× their usual", a.name),
            why: format!(
                "{} a run this week against {} over 30 days.",
                money(now),
                money(usual)
            ),
            metric: Metric::lower(
                "Cost per run",
                round2(now),
                round2(usual),
                "$",
                round2(now * 1.2),
            ),
            trend: f.agent_trend(a.id, round2(usual), |d| {
                (d.runs > 0).then(|| round2(d.spend / d.runs as f64))
            }),
            since: f.week_ago(),
            confidence: Confidence::High,
            evidence: vec![format!(
                "{} runs this week cost {}",
                week.runs,
                money(week.spend)
            )],
            fixed_when: format!(
                "Runs back under {} each",
                money(usual * COST_PER_RUN_FACTOR)
            ),
            action: action("Open the agent", Target::of("agent", a.id)),
            entities: agent_entities(f, a.id),
            flow_id: "agents-models",
            count: week.runs,
            spend: week.spend,
        }
        .into_problem(f.today),
    )
}

pub(super) fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(f64::total_cmp);
    let mid = xs.len() / 2;
    if xs.len().is_multiple_of(2) {
        (xs[mid - 1] + xs[mid]) / 2.0
    } else {
        xs[mid]
    }
}

fn clip(s: &str) -> String {
    const MAX: usize = 200;
    if s.chars().count() <= MAX {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(MAX).collect::<String>())
    }
}

// ── Coding harnesses ─────────────────────────────────────────────────────────

fn harness_problems(f: &Facts, h: &AgentRow) -> Vec<Problem> {
    let mut out: Vec<Problem> = harness_failing(f, h).into_iter().collect();
    out.extend(cost_rise(
        f,
        h.id,
        &h.name,
        "people-harnesses",
        Target::page("harnesses"),
    ));
    out
}

fn harness_failing(f: &Facts, h: &AgentRow) -> Option<Problem> {
    let sessions: Vec<_> = f
        .snap
        .sessions
        .iter()
        .filter(|s| s.agent_id == h.id)
        .collect();
    if sessions.len() < MIN_SESSIONS {
        return None;
    }
    let failed = sessions.iter().filter(|s| s.failed).count();
    let pct = failed as f64 * 100.0 / sessions.len() as f64;
    if pct < HARNESS_FAILED_PCT {
        return None;
    }
    let first_failed = sessions
        .iter()
        .find(|s| s.failed)
        .map(|s| s.last_at.date_naive());
    Some(
        Draft {
            id: format!("harness-failing:{}", h.id),
            kind: Kind::Errors,
            tier: Tier::Worse,
            title: format!(
                "{failed} of {}'s last {} sessions failed",
                h.name,
                sessions.len()
            ),
            why: "A failed session is work its developer has to redo.".into(),
            metric: Metric::higher(
                "Sessions without errors",
                round1(100.0 - pct),
                90.0,
                "%",
                100.0,
            ),
            trend: f.agent_trend(h.id, 100.0, |d| {
                (d.checked > 0).then(|| 100.0 - d.failed as f64 * 100.0 / d.checked as f64)
            }),
            since: first_failed.unwrap_or_else(|| f.week_ago()),
            confidence: Confidence::High,
            evidence: vec![format!(
                "Sessions with an error span: {failed} of the last {}",
                sessions.len()
            )],
            fixed_when: format!("Under {HARNESS_FAILED_PCT:.0}% of its latest sessions failing"),
            action: action("Open coding harnesses", Target::page("harnesses")),
            entities: vec![h.id.to_string(), h.owner_id.to_string()],
            flow_id: "people-harnesses",
            count: failed as i64,
            spend: 0.0,
        }
        .into_problem(f.today),
    )
}

// ── MCP tools ────────────────────────────────────────────────────────────────

fn tool_problems(f: &Facts) -> Vec<Problem> {
    let mut out = Vec::new();
    for c in &f.snap.connectors {
        if let Some(error) = &c.setup_failed {
            out.push(tool_setup(f, c, error));
        }
        if f.snap.expired_connections.contains(&c.id) {
            out.push(tool_signin(f, c));
        }
        out.extend(tool_failing(f, c));
    }
    out
}

fn tool_name(c: &super::load::ConnectorRow) -> &str {
    c.display_name.as_deref().unwrap_or(&c.name)
}

fn tool_target(c: &super::load::ConnectorRow) -> Target {
    if c.toolkit {
        Target::page("mcp")
    } else {
        Target::of("connector", c.id)
    }
}

/// Calls and errors to one connector in a window, per agent.
fn tool_calls_in(
    f: &Facts,
    connector: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> HashMap<Uuid, (i64, i64)> {
    let mut by_agent: HashMap<Uuid, (i64, i64)> = HashMap::new();
    for c in f
        .snap
        .tool_calls
        .iter()
        .filter(|c| c.connector_id == connector && c.day >= from && c.day <= to)
    {
        let e = by_agent.entry(c.agent_id).or_default();
        e.0 += c.calls;
        e.1 += c.errors;
    }
    by_agent
}

fn totals(by_agent: &HashMap<Uuid, (i64, i64)>) -> (i64, i64) {
    by_agent
        .values()
        .fold((0, 0), |(c, e), (c2, e2)| (c + c2, e + e2))
}

fn tool_failing(f: &Facts, c: &super::load::ConnectorRow) -> Option<Problem> {
    let week = tool_calls_in(f, c.id, f.week_ago(), f.today);
    let (calls, errors) = totals(&week);
    if calls < MIN_BASE {
        return None;
    }
    let pct = errors as f64 * 100.0 / calls as f64;
    let prev = totals(&tool_calls_in(
        f,
        c.id,
        f.week_ago() - Duration::days(7),
        f.week_ago() - Duration::days(1),
    ));
    let before = (prev.0 >= MIN_BASE).then(|| prev.1 as f64 * 100.0 / prev.0 as f64);
    let tier = failing_tier(pct, before, TOOL_BROKEN_PCT, TOOL_WORSE_PCT)?;
    let mut callers: Vec<(Uuid, (i64, i64))> =
        week.into_iter().filter(|(_, (_, e))| *e > 0).collect();
    callers.sort_by_key(|(_, (_, errors))| std::cmp::Reverse(*errors));
    let names: Vec<&str> = callers
        .iter()
        .take(3)
        .map(|(id, _)| f.agent_name(*id))
        .collect();
    let day_pct = |day: NaiveDate| {
        let (c2, e2) = totals(&tool_calls_in(f, c.id, day, day));
        (c2 > 0).then(|| e2 as f64 * 100.0 / c2 as f64)
    };
    let mut entities = vec![c.id.to_string()];
    entities.extend(callers.iter().map(|(id, _)| id.to_string()));
    let mut evidence = vec![format!(
        "{errors} of {calls} calls failed in the last 7 days"
    )];
    if let Some((id, (_, e))) = callers.first() {
        evidence.push(format!(
            "Most failing calls come from {} ({e})",
            f.agent_name(*id)
        ));
    }
    let why = match names.as_slice() {
        [] => "Calls to it fail.".to_owned(),
        [one] => format!("{one} depends on it, so its runs fail with it."),
        many => format!(
            "{} depend on it, so their runs fail with it.",
            many.join(", ")
        ),
    };
    Some(
        Draft {
            id: format!("tool-failing:{}", c.id),
            kind: Kind::Errors,
            tier,
            title: format!("The {} tool fails {pct:.0}% of calls", tool_name(c)),
            why,
            metric: Metric::lower(
                "Tool call errors",
                round1(pct),
                1.0,
                "%",
                round1((pct * 1.4).max(5.0)),
            ),
            trend: trend(&f.days, 0.0, day_pct),
            since: since(&f.days, f.week_ago(), |day| {
                day_pct(day).map(|p| p >= TOOL_WORSE_PCT)
            }),
            confidence: Confidence::High,
            evidence,
            fixed_when: "Under 1% of calls failing for a day".into(),
            action: action("Open the MCP server", tool_target(c)),
            entities,
            flow_id: "agents-tools",
            count: errors,
            spend: 0.0,
        }
        .into_problem(f.today),
    )
}

fn tool_setup(f: &Facts, c: &super::load::ConnectorRow, error: &str) -> Problem {
    Draft {
        id: format!("tool-setup:{}", c.id),
        kind: Kind::Errors,
        tier: Tier::Broken,
        title: format!("{} isn't set up", tool_name(c)),
        why: "Its setup failed, so no agent can call its tools.".into(),
        metric: Metric::higher("Set up", 0.0, 1.0, "", 1.0),
        trend: vec![0.0; TREND_DAYS],
        since: c.updated_at.date_naive(),
        confidence: Confidence::High,
        evidence: vec![if error.is_empty() {
            "Setup failed with no error recorded".into()
        } else {
            clip(error)
        }],
        fixed_when: "Setup finishes".into(),
        action: action("Open the MCP server", tool_target(c)),
        entities: vec![c.id.to_string()],
        flow_id: "agents-tools",
        count: 1,
        spend: 0.0,
    }
    .into_problem(f.today)
}

/// The caller's own sign-in to a connector expired: only they can fix it.
fn tool_signin(f: &Facts, c: &super::load::ConnectorRow) -> Problem {
    Draft {
        id: format!("tool-signin:{}:{}", c.id, f.snap.me),
        kind: Kind::Errors,
        tier: Tier::Broken,
        title: format!("Your {} sign-in expired", tool_name(c)),
        why: "Tools that act as you stop working until you sign in again.".into(),
        metric: Metric::higher("Signed in", 0.0, 1.0, "", 1.0),
        trend: vec![0.0; TREND_DAYS],
        since: f.today,
        confidence: Confidence::High,
        evidence: vec!["The connection's token was refused and marked expired".into()],
        fixed_when: "You sign in again".into(),
        action: action("Sign in again", tool_target(c)),
        entities: vec![c.id.to_string(), f.snap.me.to_string()],
        flow_id: "agents-tools",
        count: 1,
        spend: 0.0,
    }
    .into_problem(f.today)
}

// ── Workflows ────────────────────────────────────────────────────────────────

/// Runs and failures of one workflow between two days, inclusive.
fn runs_between(f: &Facts, maf: Uuid, from: NaiveDate, to: NaiveDate) -> (i64, i64) {
    f.snap
        .runs_daily
        .iter()
        .filter(|r| r.maf_id == maf && r.day >= from && r.day <= to)
        .fold((0, 0), |(r, x), d| (r + d.runs, x + d.failed))
}

/// The agent whose steps failed most in a workflow this week, with its count.
fn failing_step(f: &Facts, maf: Uuid) -> Option<(Uuid, i64)> {
    f.snap
        .steps_week
        .iter()
        .filter(|s| s.maf_id == maf && s.failed > 0)
        .max_by_key(|s| s.failed)
        .map(|s| (s.agent_id, s.failed))
}

fn workflow_problems(f: &Facts) -> Vec<Problem> {
    f.snap
        .workflows
        .iter()
        .filter_map(|w| workflow_failing(f, w))
        .collect()
}

fn workflow_failing(f: &Facts, w: &super::load::WorkflowRow) -> Option<Problem> {
    let (runs, failed) = runs_between(f, w.id, f.week_ago(), f.today);
    if runs < MIN_WORKFLOW_RUNS {
        return None;
    }
    let pct = failed as f64 * 100.0 / runs as f64;
    let (prev_runs, prev_failed) = runs_between(
        f,
        w.id,
        f.week_ago() - Duration::days(7),
        f.week_ago() - Duration::days(1),
    );
    let before =
        (prev_runs >= MIN_WORKFLOW_RUNS).then(|| prev_failed as f64 * 100.0 / prev_runs as f64);
    let tier = failing_tier(pct, before, FAILING_BROKEN_PCT, FAILING_BROKEN_PCT / 2.0)?;
    let day_pct = |day: NaiveDate| {
        let (r, x) = runs_between(f, w.id, day, day);
        (r > 0).then(|| x as f64 * 100.0 / r as f64)
    };
    let mut evidence = vec![format!("{failed} of {runs} runs failed in the last 7 days")];
    let mut entities = vec![w.id.to_string()];
    if let Some((agent, n)) = failing_step(f, w.id) {
        evidence.push(format!("Its {} step failed {n} times", f.agent_name(agent)));
        entities.push(agent.to_string());
    }
    entities.push(w.owner_id.to_string());
    Some(
        Draft {
            id: format!("workflow-failing:{}", w.id),
            kind: Kind::Errors,
            tier,
            title: format!("{} fails {pct:.0}% of runs", w.name),
            why: "A failed run stops partway: its later steps never happen.".into(),
            metric: Metric::higher("Successful runs", round1(100.0 - pct), 95.0, "%", 100.0),
            trend: trend(&f.days, 100.0, |day| day_pct(day).map(|p| 100.0 - p)),
            since: since(&f.days, f.week_ago(), |day| {
                day_pct(day).map(|p| p >= FAILING_BROKEN_PCT / 2.0)
            }),
            confidence: Confidence::High,
            evidence,
            fixed_when: format!(
                "Under {:.0}% of runs failing for a day",
                FAILING_BROKEN_PCT / 2.0
            ),
            action: action("Open the workflow", Target::of("workflow", w.id)),
            entities,
            flow_id: "people-workflows",
            count: failed,
            spend: 0.0,
        }
        .into_problem(f.today),
    )
}

// ── Routers, providers and models ────────────────────────────────────────────

fn model_problems(f: &Facts) -> Vec<Problem> {
    let mut out: Vec<Problem> = f
        .snap
        .provider_failures
        .iter()
        .map(|p| provider_sync(f, p))
        .collect();
    out.extend(f.snap.models_week.iter().filter_map(|m| model_slow(f, m)));
    out
}

/// The routers whose primary or fallback model is `model`.
fn routers_using_model(f: &Facts, model: &str) -> Vec<String> {
    f.snap
        .routers
        .iter()
        .filter(|r| {
            r.model.as_deref() == Some(model)
                || r.fallback_models
                    .0
                    .iter()
                    .any(|m| m.as_str() == Some(model))
        })
        .map(|r| r.id.to_string())
        .collect()
}

/// A custom provider OpenRuntime can't reach to list its models: calls routed to it are likely
/// failing the same way, so it is inferred (medium), not measured.
fn provider_sync(f: &Facts, p: &super::load::ProviderFailure) -> Problem {
    let routers: Vec<_> = f
        .snap
        .routers
        .iter()
        .filter(|r| r.provider == p.label)
        .collect();
    let mut entities = vec![format!("provider:{}", p.label)];
    entities.extend(routers.iter().map(|r| r.id.to_string()));
    entities.extend(
        f.snap
            .agents
            .iter()
            .filter(|a| {
                a.router_id
                    .is_some_and(|id| routers.iter().any(|r| r.id == id))
            })
            .map(|a| a.id.to_string()),
    );
    Draft {
        id: format!("provider-sync:{}", p.label),
        kind: Kind::Errors,
        tier: Tier::Worse,
        title: format!("OpenRuntime can't reach {}", p.display_name),
        why: "Its model list stopped refreshing; calls routed to it may fail the same way.".into(),
        metric: Metric::higher("Reachable", 0.0, 1.0, "", 1.0),
        trend: vec![0.0; TREND_DAYS],
        since: f.today,
        confidence: Confidence::Medium,
        evidence: vec![format!(
            "Last model sync failed: {}",
            clip(p.error.as_deref().unwrap_or("no error recorded"))
        )],
        fixed_when: "A model sync succeeds".into(),
        action: action("Open the LLM router", Target::page("router")),
        entities,
        flow_id: "agents-models",
        count: routers.len() as i64,
        spend: 0.0,
    }
    .into_problem(f.today)
}

fn model_slow(f: &Facts, m: &super::load::ModelUsage) -> Option<Problem> {
    let prev = f
        .snap
        .models_prev_week
        .iter()
        .find(|p| p.model == m.model && p.timed_calls >= MIN_BASE)?;
    let (now_ms, before_ms) = (m.p95_ms?, prev.p95_ms?);
    if m.timed_calls < MIN_BASE || before_ms <= 0.0 {
        return None;
    }
    let rise = pct_change(now_ms, before_ms);
    if rise <= P95_RISE_PCT {
        return None;
    }
    let secs = |ms: f64| round1(ms / 1000.0);
    let mut entities = vec![
        format!("model:{}", m.model),
        format!("provider:{}", m.provider),
    ];
    entities.extend(routers_using_model(f, &m.model));
    entities.extend(
        f.snap
            .agent_models
            .iter()
            .filter(|am| am.model == m.model)
            .map(|am| am.agent_id.to_string()),
    );
    Some(
        Draft {
            id: format!("model-slow:{}", m.model),
            kind: Kind::Slow,
            tier: Tier::Worse,
            title: format!("{} got {rise:.0}% slower", m.model),
            why: "Every agent and harness calling it waits longer for each answer.".into(),
            metric: Metric::lower(
                "p95 latency",
                secs(now_ms),
                secs(before_ms),
                "s",
                secs(now_ms * 1.5),
            ),
            trend: vec![secs(before_ms); TREND_DAYS - 1]
                .into_iter()
                .chain([secs(now_ms)])
                .collect(),
            since: f.week_ago(),
            confidence: Confidence::High,
            evidence: vec![format!(
                "p95 {}s over {} agent calls in the last 7 days, {}s over {} the week before",
                secs(now_ms),
                m.timed_calls,
                secs(before_ms),
                prev.timed_calls
            )],
            fixed_when: format!(
                "p95 back under {}s",
                secs(before_ms * (1.0 + P95_RISE_PCT / 100.0))
            ),
            action: action("Open the LLM router", Target::page("router")),
            entities,
            flow_id: "agents-models",
            count: m.calls,
            spend: m.spend,
        }
        .into_problem(f.today),
    )
}

// ── Cause chains ─────────────────────────────────────────────────────────────

/// Links problems that share a cause, and marks each with its chain. A chain needs evidence for
/// every link: an agent's own calls to the failing tool failed, or a workflow's own steps failed
/// on that agent. Configuration alone (a provider and the agents routed to it) is medium.
fn link_chains(f: &Facts, problems: &mut [Problem]) -> Vec<Chain> {
    let mut chains = Vec::new();
    let mut chained: HashSet<String> = HashSet::new();
    let ids: HashMap<String, usize> = problems
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), i))
        .collect();
    let agent_problem = |agent: Uuid, chained: &HashSet<String>| {
        ["agent-failing", "agent-down"]
            .iter()
            .map(|rule| format!("{rule}:{agent}"))
            .find(|id| ids.contains_key(id) && !chained.contains(id))
    };
    let workflow_failing_on = |agent: Uuid, chained: &HashSet<String>| -> Vec<String> {
        f.snap
            .workflows
            .iter()
            .filter(|w| failing_step(f, w.id).is_some_and(|(a, _)| a == agent))
            .map(|w| format!("workflow-failing:{}", w.id))
            .filter(|id| ids.contains_key(id) && !chained.contains(id))
            .collect()
    };

    // A failing tool, the agents whose calls to it failed, and the workflows failing on those agents.
    for c in &f.snap.connectors {
        let root = format!("tool-failing:{}", c.id);
        let Some(&r) = ids.get(&root) else { continue };
        let week = tool_calls_in(f, c.id, f.week_ago(), f.today);
        let mut links = Vec::new();
        let mut steps = vec![step(&problems[r], c.id.to_string())];
        for (agent, (_, errors)) in &week {
            if *errors == 0 {
                continue;
            }
            let Some(id) = agent_problem(*agent, &chained) else {
                continue;
            };
            let mut s = step(&problems[ids[&id]], agent.to_string());
            s.detail = format!("{errors} of its calls to {} failed this week", tool_name(c));
            steps.push(s);
            links.push(id);
            for wf in workflow_failing_on(*agent, &chained) {
                steps.push(step(
                    &problems[ids[&wf]],
                    problems[ids[&wf]].entities[0].clone(),
                ));
                links.push(wf);
            }
        }
        if links.is_empty() {
            continue;
        }
        links.push(root.clone());
        chains.push(chain(
            format!("chain:tool:{}", c.id),
            format!(
                "The {} tool is failing the agents that call it",
                tool_name(c)
            ),
            Confidence::High,
            steps,
            format!(
                "Fix {}: the agents and workflows that call it recover with it.",
                tool_name(c)
            ),
            problems[r].action.clone(),
            &links,
        ));
        chained.extend(links);
    }

    // An agent failing the workflows whose steps run on it.
    for a in f.snap.agents.iter().filter(|a| !a.is_harness()) {
        let Some(root) = agent_problem(a.id, &chained) else {
            continue;
        };
        let workflows = workflow_failing_on(a.id, &chained);
        if workflows.is_empty() {
            continue;
        }
        let mut steps = vec![step(&problems[ids[&root]], a.id.to_string())];
        for wf in &workflows {
            let mut s = step(&problems[ids[wf]], problems[ids[wf]].entities[0].clone());
            s.detail = format!("its {} step fails", a.name);
            steps.push(s);
        }
        let mut links = workflows;
        links.push(root.clone());
        chains.push(chain(
            format!("chain:agent:{}", a.id),
            format!("{} is failing the workflows that run it", a.name),
            Confidence::High,
            steps,
            format!("Fix {}: the workflows that run it recover with it.", a.name),
            problems[ids[&root]].action.clone(),
            &links,
        ));
        chained.extend(links);
    }

    // A provider OpenRuntime can't reach, and the failing agents routed to it.
    for p in &f.snap.provider_failures {
        let root = format!("provider-sync:{}", p.label);
        let Some(&r) = ids.get(&root) else { continue };
        let routed: Vec<Uuid> = f
            .snap
            .agents
            .iter()
            .filter(|a| {
                a.router_id.is_some_and(|id| {
                    f.snap
                        .routers
                        .iter()
                        .any(|x| x.id == id && x.provider == p.label)
                })
            })
            .map(|a| a.id)
            .collect();
        let links: Vec<String> = routed
            .iter()
            .filter_map(|a| agent_problem(*a, &chained))
            .collect();
        if links.is_empty() {
            continue;
        }
        let mut steps = vec![step(&problems[r], format!("provider:{}", p.label))];
        steps.extend(
            links
                .iter()
                .map(|id| step(&problems[ids[id]], problems[ids[id]].entities[0].clone())),
        );
        let mut all = links;
        all.push(root.clone());
        chains.push(chain(
            format!("chain:provider:{}", p.label),
            format!("Agents routed to {} are failing", p.display_name),
            Confidence::Medium,
            steps,
            format!(
                "Restore {}, or route these agents to another provider.",
                p.display_name
            ),
            problems[r].action.clone(),
            &all,
        ));
        chained.extend(all);
    }

    mark_chained(problems, &chains);
    chains
}

fn step(p: &Problem, entity: String) -> ChainStep {
    ChainStep {
        entity,
        what: p.title.clone(),
        detail: p.evidence.first().cloned().unwrap_or_default(),
    }
}

fn chain(
    id: String,
    title: String,
    confidence: Confidence,
    steps: Vec<ChainStep>,
    fix: String,
    action: Action,
    members: &[String],
) -> Chain {
    Chain {
        id,
        title,
        confidence,
        steps,
        fix,
        clears: members.len() as i64,
        action,
        members: members.to_vec(),
    }
}

fn mark_chained(problems: &mut [Problem], chains: &[Chain]) {
    for c in chains {
        for p in problems.iter_mut().filter(|p| c.members.contains(&p.id)) {
            p.chain_id = Some(c.id.clone());
        }
    }
}

// ── Opportunities ────────────────────────────────────────────────────────────

fn opportunities(f: &Facts) -> Vec<Opportunity> {
    let mut out = compression_opportunities(f);
    out.extend(cache_opportunities(f));
    out.sort_by(|a, b| b.saving_usd.total_cmp(&a.saving_usd));
    out
}

/// Agents without payload compression, priced at what it measurably saves the agents that have it.
fn compression_opportunities(f: &Facts) -> Vec<Opportunity> {
    let rate = &f.snap.compression;
    if rate.saved <= 0.0 || rate.spend <= 0.0 {
        return Vec::new();
    }
    // The share of the would-be bill compression removed: saved / (spent + saved).
    let share = rate.saved / (rate.spend + rate.saved);
    f.snap
        .routed_spend
        .iter()
        .filter_map(|s| {
            let agent = f.snap.agents.iter().find(|a| a.id == s.agent_id && !a.compress_enabled && !a.is_harness())?;
            let saving = round2(s.amount * share);
            (saving >= MIN_SAVING_USD).then(|| Opportunity {
                id: format!("opp-compress:{}", agent.id),
                title: format!("Turn on token optimization for {}", agent.name),
                detail: format!(
                    "Agents with it on paid {:.0}% less for their model calls over the last 30 days; {} spent {} without it.",
                    share * 100.0,
                    agent.name,
                    money(s.amount)
                ),
                saving_usd: saving,
                method: format!(
                    "Its 30-day spend through the router × the {:.0}% the agents with it on measurably saved.",
                    share * 100.0
                ),
                effort: "One setting",
                entities: vec![agent.id.to_string()],
                action: action("Open its settings", Target::of("agent", agent.id).with("tab", "settings")),
            })
        })
        .collect()
}

/// Agents that read far less of their input from the prompt cache than the fleet's best quarter.
fn cache_opportunities(f: &Facts) -> Vec<Opportunity> {
    let share = |input: i64, cached: i64| cached as f64 / (input + cached) as f64;
    let priced: Vec<_> = f
        .snap
        .cache
        .iter()
        .filter(|c| c.input_tokens + c.cache_read_tokens >= MIN_CACHE_TOKENS)
        .filter(|c| matches!((c.input_price_per_1m, c.cache_read_price_per_1m), (Some(i), Some(r)) if i > r))
        .collect();
    if priced.len() < 3 {
        return Vec::new();
    }
    let mut shares: Vec<f64> = priced
        .iter()
        .map(|c| share(c.input_tokens, c.cache_read_tokens))
        .collect();
    shares.sort_by(f64::total_cmp);
    let target = shares[(shares.len() * 3) / 4];
    let mut best: HashMap<Uuid, Opportunity> = HashMap::new();
    for c in priced {
        let now = share(c.input_tokens, c.cache_read_tokens);
        if now > target - CACHE_GAP {
            continue;
        }
        let (Some(input), Some(read)) = (c.input_price_per_1m, c.cache_read_price_per_1m) else {
            continue;
        };
        let tokens = (c.input_tokens + c.cache_read_tokens) as f64;
        let saving = round2((target - now) * tokens * (input - read) / 1_000_000.0);
        if saving < MIN_SAVING_USD
            || best
                .get(&c.agent_id)
                .is_some_and(|o| o.saving_usd >= saving)
        {
            continue;
        }
        let name = f.agent_name(c.agent_id);
        best.insert(
            c.agent_id,
            Opportunity {
                id: format!("opp-cache:{}", c.agent_id),
                title: format!("Cache more of {name}'s prompts"),
                detail: format!(
                    "{:.0}% of its {} input is read from the cache; the fleet's best quarter reads {:.0}%.",
                    now * 100.0,
                    c.model,
                    target * 100.0
                ),
                saving_usd: saving,
                method: "(The best quarter's cached share − its own) × its 30-day input tokens × (input − cache-read price)."
                    .into(),
                effort: "Prompt edit",
                entities: vec![c.agent_id.to_string(), format!("model:{}", c.model)],
                action: action("Open the agent", Target::of("agent", c.agent_id)),
            },
        );
    }
    best.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overview::load::{AgentAmount, RunDay, StepOutcome};
    use crate::overview::load::{
        AgentUsage, CompressionRate, ConnectorRow, ToolCall, WorkflowRow, WorkflowStep,
    };

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-21T12:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn agent(id: u128, name: &str, status: &str) -> AgentRow {
        AgentRow {
            id: Uuid::from_u128(id),
            name: name.into(),
            owner_id: Uuid::from_u128(1),
            status: status.into(),
            updated_at: now() - Duration::days(2),
            router_id: None,
            harness: None,
            compress_enabled: false,
        }
    }

    fn usage(id: u128, runs: i64, failed: i64, spend: f64) -> AgentUsage {
        AgentUsage {
            agent_id: Uuid::from_u128(id),
            stats: UsageStats {
                runs,
                spend,
                priced: runs,
                checked: runs,
                failed,
                failed_spend: 0.0,
                p95_ms: Some(1000.0),
            },
        }
    }

    fn connector(id: u128, name: &str) -> ConnectorRow {
        ConnectorRow {
            id: Uuid::from_u128(id),
            name: name.into(),
            display_name: None,
            toolkit: false,
            is_active: true,
            updated_at: now(),
            setup_failed: None,
        }
    }

    fn ids(problems: &[Problem]) -> Vec<&str> {
        problems.iter().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn an_agent_failing_a_fifth_of_its_runs_is_broken() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Pricing Engine", "running"));
        snap.usage_week.push(usage(10, 100, 28, 5.0));
        let found = detect(&snap).problems;
        let p = found
            .iter()
            .find(|p| p.id.starts_with("agent-failing:"))
            .expect("a failing problem");
        assert_eq!(p.tier, Tier::Broken);
        assert_eq!(p.title, "Pricing Engine fails 28% of runs");
    }

    #[test]
    fn a_lower_rate_is_worse_only_once_it_doubled_on_enough_runs() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Doc Writer", "running"));
        snap.usage_week.push(usage(10, 100, 8, 5.0));
        // No week before to compare with: 8% alone raises nothing.
        assert!(detect(&snap).problems.is_empty());
        snap.usage_prev_week.push(usage(10, 100, 3, 5.0));
        let found = detect(&snap).problems;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tier, Tier::Worse);
        // Too few runs to say: nothing.
        let mut few = Snapshot::empty(now());
        few.agents.push(agent(10, "Doc Writer", "running"));
        few.usage_week.push(usage(10, 10, 9, 5.0));
        assert!(detect(&few).problems.is_empty());
    }

    #[test]
    fn a_crashed_agent_is_one_problem_however_else_it_fails() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Code Reviewer", "crashed"));
        snap.usage_week.push(usage(10, 100, 60, 5.0));
        let found = detect(&snap).problems;
        assert_eq!(
            ids(&found),
            vec![format!("agent-down:{}", Uuid::from_u128(10))]
        );
        assert_eq!(found[0].title, "Code Reviewer has crashed");
    }

    #[test]
    fn a_failing_tool_chains_to_the_agents_whose_calls_to_it_failed() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Support Bot", "running"));
        snap.agents.push(agent(11, "Triage Router", "running"));
        snap.connectors.push(connector(20, "jira-cloud"));
        snap.usage_week.push(usage(10, 100, 30, 5.0));
        snap.usage_week.push(usage(11, 100, 0, 5.0));
        for (agent, errors) in [(10, 15), (11, 0)] {
            snap.tool_calls.push(ToolCall {
                agent_id: Uuid::from_u128(agent),
                connector_id: Uuid::from_u128(20),
                day: now().date_naive(),
                calls: 50,
                errors,
            });
        }
        let found = detect(&snap);
        assert_eq!(found.chains.len(), 1);
        let chain = &found.chains[0];
        assert_eq!(chain.confidence, Confidence::High);
        assert_eq!(chain.clears, 2);
        for id in [
            format!("tool-failing:{}", Uuid::from_u128(20)),
            format!("agent-failing:{}", Uuid::from_u128(10)),
        ] {
            let p = found
                .problems
                .iter()
                .find(|p| p.id == id)
                .expect("problem present");
            assert_eq!(p.chain_id.as_deref(), Some(chain.id.as_str()));
        }
    }

    #[test]
    fn a_workflow_failing_on_one_agent_chains_to_that_agent() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Invoice Parser", "running"));
        snap.usage_week.push(usage(10, 100, 25, 5.0));
        let maf = Uuid::from_u128(30);
        snap.workflows.push(WorkflowRow {
            id: maf,
            name: "Invoice triage".into(),
            owner_id: Uuid::from_u128(1),
            steps: vec![WorkflowStep {
                step_index: 0,
                agent_id: Uuid::from_u128(10),
                agent_name: "Invoice Parser".into(),
            }],
        });
        snap.runs_daily.push(RunDay {
            maf_id: maf,
            day: now().date_naive(),
            runs: 10,
            failed: 4,
        });
        snap.steps_week.push(StepOutcome {
            maf_id: maf,
            agent_id: Uuid::from_u128(10),
            steps: 10,
            failed: 4,
        });
        let found = detect(&snap);
        assert_eq!(found.chains.len(), 1);
        assert_eq!(
            found.chains[0].id,
            format!("chain:agent:{}", Uuid::from_u128(10))
        );
        assert!(found.problems.iter().all(|p| p.chain_id.is_some()));
    }

    #[test]
    fn handled_holds_for_its_day_and_an_expired_snooze_is_gone() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Pricing Engine", "crashed"));
        let mut problems = detect(&snap).problems;
        let id = problems[0].id.clone();
        let yesterday = Decision {
            problem_id: id.clone(),
            owner_id: Some(Uuid::from_u128(1)),
            snoozed_until: Some(now() - Duration::hours(1)),
            handled_at: Some(now() - Duration::days(1)),
        };
        apply_decisions(&mut problems, &[yesterday], now());
        assert_eq!(problems[0].owner_id, Some(Uuid::from_u128(1)));
        assert_eq!(problems[0].snoozed_until, None);
        assert_eq!(
            problems[0].handled_at, None,
            "a problem still found the next day reopens"
        );
        let today = Decision::new(&id).with(Change::Handled(true), now());
        apply_decisions(&mut problems, &[today], now());
        assert_eq!(problems[0].handled_at, Some(now()));
    }

    #[test]
    fn compression_is_priced_at_the_rate_it_measurably_saves() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Sales Assistant", "running"));
        let mut on = agent(11, "Support Bot", "running");
        on.compress_enabled = true;
        snap.agents.push(on);
        // Agents with it on spent $80 and saved $20: a fifth of the would-be bill.
        snap.compression = CompressionRate {
            saved: 20.0,
            spend: 80.0,
        };
        snap.routed_spend.push(AgentAmount {
            agent_id: Uuid::from_u128(10),
            amount: 50.0,
        });
        snap.routed_spend.push(AgentAmount {
            agent_id: Uuid::from_u128(11),
            amount: 50.0,
        });
        let found = detect(&snap).opportunities;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, format!("opp-compress:{}", Uuid::from_u128(10)));
        assert_eq!(found[0].saving_usd, 10.0);
    }

    #[test]
    fn no_measured_savings_means_no_compression_opportunity() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Sales Assistant", "running"));
        snap.routed_spend.push(AgentAmount {
            agent_id: Uuid::from_u128(10),
            amount: 500.0,
        });
        assert!(detect(&snap).opportunities.is_empty());
    }

    #[test]
    fn a_day_three_times_the_typical_one_is_a_spike() {
        let mut snap = Snapshot::empty(now());
        snap.agents.push(agent(10, "Research Agent", "running"));
        let days = snap.windows.days();
        for (i, day) in days.iter().enumerate() {
            let spend = if i == days.len() - 2 { 40.0 } else { 10.0 };
            snap.usage_daily.push(crate::overview::load::AgentDay {
                agent_id: Uuid::from_u128(10),
                day: *day,
                spend,
                runs: 10,
                checked: 10,
                failed: 0,
                p95_ms: None,
            });
        }
        let found = detect(&snap).problems;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::Cost);
        assert_eq!(found[0].tier, Tier::Broken);
        assert!(
            found[0].title.contains("4.0× a typical day"),
            "{}",
            found[0].title
        );
    }
}
