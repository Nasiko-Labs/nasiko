//! Is it safe to switch from the control model to a cheaper candidate?
//!
//! The run report gives each model's own resolved rate. With the few tasks a replay dataset
//! has, two such ranges almost always overlap, and a range around 5/5 says nothing about how
//! much worse the candidate might be. This module asks the question a switch needs answered,
//! on the same tasks:
//!
//! - Each task counts once: the first attempt of each model is paired on the same task, so
//!   repeated attempts are not counted as new evidence, and task difficulty cancels out.
//! - Tasks where either side hit an infrastructure error are left out (and counted), so a
//!   flaky sandbox is not mistaken for a worse model. Tasks whose attempts disagree with each
//!   other are counted as flaky.
//! - The difference `candidate − control` gets a 95% range that stays honest at small n:
//!   Newcombe's hybrid score interval for paired proportions (method 10, built from Wilson
//!   intervals), which never collapses to a point the way a bootstrap of 5/5 does.
//! - The verdict compares that range with a margin `δ` (how many points worse is acceptable):
//!   SAFE when the whole range is above `−δ`, NOT SAFE when it is all below, and otherwise NOT
//!   ENOUGH DATA, with the number of tasks it would take if the current rates held.
//!
//! The result is an exit code for CI (`nasiko eval verdict`), so a bad switch can be blocked.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::grade::FailureClass;
use crate::report::AttemptRecord;

/// z for a two-sided 95% range.
const Z: f64 = 1.959_963_984_540_054;

/// The largest dataset the "how many more tasks" estimate searches.
const MAX_TASKS: u32 = 5_000;

/// The default margin: the candidate may be at most 10 points worse than the control.
pub const DEFAULT_MARGIN: f64 = 0.10;

/// What the verdict needs from one attempt; built from a local [`AttemptRecord`] or from a
/// server run's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub task: String,
    pub model: String,
    pub attempt: u32,
    pub resolved: bool,
    /// The sandbox, not the model, failed: the attempt says nothing about the model.
    pub infra: bool,
    pub cost_usd: Option<f64>,
}

impl From<&AttemptRecord> for Outcome {
    fn from(r: &AttemptRecord) -> Self {
        Outcome {
            task: r.instance_id.clone(),
            model: r.model.clone(),
            attempt: r.attempt,
            resolved: r.grade.resolved,
            infra: r.grade.failure == Some(FailureClass::InfraError),
            cost_usd: r.cost_usd,
        }
    }
}

/// The paired first attempts, as a 2×2 table over tasks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Pairs {
    /// Both models resolved the task.
    pub both: u32,
    /// Only the control resolved it.
    pub control_only: u32,
    /// Only the candidate resolved it.
    pub candidate_only: u32,
    /// Neither did.
    pub neither: u32,
}

impl Pairs {
    pub fn n(&self) -> u32 {
        self.both + self.control_only + self.candidate_only + self.neither
    }
}

/// The decision, and the process exit code `nasiko eval verdict` uses for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    Safe,
    NotSafe,
    NotEnoughData,
}

impl Decision {
    /// 0 lets the switch through; 2 and 3 block it (1 is left for tool errors).
    pub fn exit_code(self) -> i32 {
        match self {
            Decision::Safe => 0,
            Decision::NotSafe => 2,
            Decision::NotEnoughData => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Decision::Safe => "SAFE",
            Decision::NotSafe => "NOT SAFE",
            Decision::NotEnoughData => "NOT ENOUGH DATA",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub control: String,
    pub candidate: String,
    /// How much worse (as a share of tasks) the candidate may be.
    pub margin: f64,
    pub pairs: Pairs,
    /// Resolved-rate difference `candidate − control` on the paired tasks.
    pub diff: f64,
    pub low95: f64,
    pub high95: f64,
    pub decision: Decision,
    /// For NOT ENOUGH DATA: how many more paired tasks would settle it if the rates held.
    /// `None` when no dataset up to [`MAX_TASKS`] would (the difference sits on the margin).
    pub more_tasks: Option<u32>,
    /// Tasks left out because either side's first attempt was an infrastructure error.
    pub excluded_infra: u32,
    /// Tasks only one of the two models attempted.
    pub unpaired: u32,
    /// Paired tasks where some model's repeated attempts disagreed.
    pub flaky_tasks: u32,
    /// Total cost on the paired tasks; `None` when any of their attempts is unpriced.
    pub control_cost_usd: Option<f64>,
    pub candidate_cost_usd: Option<f64>,
}

impl Verdict {
    /// The share saved by switching, on the paired tasks.
    pub fn saving(&self) -> Option<f64> {
        match (self.control_cost_usd, self.candidate_cost_usd) {
            (Some(c), Some(k)) if c > 0.0 => Some(1.0 - k / c),
            _ => None,
        }
    }
}

/// Wilson score interval for `k` successes out of `n`; `(0, 1)` when `n` is 0.
pub fn wilson(k: u32, n: u32) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let (k, n) = (f64::from(k.min(n)), f64::from(n));
    let p = k / n;
    let z2 = Z * Z;
    let centre = (p + z2 / (2.0 * n)) / (1.0 + z2 / n);
    let half = Z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / (1.0 + z2 / n);
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// Newcombe's 95% range for the paired difference `candidate − control`, with the difference.
pub fn paired_difference(p: Pairs) -> (f64, f64, f64) {
    let n = p.n();
    if n == 0 {
        return (0.0, -1.0, 1.0);
    }
    let nf = f64::from(n);
    let candidate = p.both + p.candidate_only;
    let control = p.both + p.control_only;
    let (p1, p2) = (f64::from(candidate) / nf, f64::from(control) / nf);
    let (l1, u1) = wilson(candidate, n);
    let (l2, u2) = wilson(control, n);
    let d = p1 - p2;
    let phi = correlation(p);
    let below = (p1 - l1).powi(2) - 2.0 * phi * (p1 - l1) * (u2 - p2) + (u2 - p2).powi(2);
    let above = (u1 - p1).powi(2) - 2.0 * phi * (u1 - p1) * (p2 - l2) + (p2 - l2).powi(2);
    (
        d,
        (d - below.max(0.0).sqrt()).max(-1.0),
        (d + above.max(0.0).sqrt()).min(1.0),
    )
}

/// The continuity-corrected phi correlation between the two models' outcomes; 0 when a
/// margin of the table is empty (e.g. both resolved every task).
fn correlation(p: Pairs) -> f64 {
    let (a, b, c, d) = (
        f64::from(p.both),
        f64::from(p.candidate_only),
        f64::from(p.control_only),
        f64::from(p.neither),
    );
    let denominator = ((a + b) * (c + d) * (a + c) * (b + d)).sqrt();
    if denominator == 0.0 {
        return 0.0;
    }
    let cross = a * d - b * c;
    let n = a + b + c + d;
    let numerator = if cross > 0.0 {
        (cross - n / 2.0).max(0.0)
    } else {
        cross
    };
    numerator / denominator
}

fn decide(low: f64, high: f64, margin: f64) -> Decision {
    if low > -margin {
        Decision::Safe
    } else if high < -margin {
        Decision::NotSafe
    } else {
        Decision::NotEnoughData
    }
}

/// The smallest number of extra paired tasks after which the decision is no longer NOT ENOUGH
/// DATA, if each cell of the table kept its share.
fn more_tasks_needed(p: Pairs, margin: f64) -> Option<u32> {
    let n = p.n();
    if n == 0 {
        return None;
    }
    let scale = |count: u32, total: u32| {
        (f64::from(count) * f64::from(total) / f64::from(n)).round() as u32
    };
    (n + 1..=MAX_TASKS)
        .find(|&total| {
            let grown = Pairs {
                both: scale(p.both, total),
                control_only: scale(p.control_only, total),
                candidate_only: scale(p.candidate_only, total),
                neither: scale(p.neither, total),
            };
            let (_, low, high) = paired_difference(grown);
            decide(low, high, margin) != Decision::NotEnoughData
        })
        .map(|total| total - n)
}

/// The verdict for switching from `control` to `candidate`, from all attempts of a run.
pub fn verdict(outcomes: &[Outcome], control: &str, candidate: &str, margin: f64) -> Verdict {
    let margin = margin.clamp(0.0, 1.0);
    let attempts = |model: &str| -> BTreeMap<&str, Vec<&Outcome>> {
        let mut by_task: BTreeMap<&str, Vec<&Outcome>> = BTreeMap::new();
        for o in outcomes.iter().filter(|o| o.model == model) {
            by_task.entry(o.task.as_str()).or_default().push(o);
        }
        for list in by_task.values_mut() {
            list.sort_by_key(|o| o.attempt);
        }
        by_task
    };
    let (ctrl, cand) = (attempts(control), attempts(candidate));
    let tasks: BTreeSet<&str> = ctrl.keys().chain(cand.keys()).copied().collect();
    let mut v = Verdict {
        control: control.to_string(),
        candidate: candidate.to_string(),
        margin,
        pairs: Pairs::default(),
        diff: 0.0,
        low95: -1.0,
        high95: 1.0,
        decision: Decision::NotEnoughData,
        more_tasks: None,
        excluded_infra: 0,
        unpaired: 0,
        flaky_tasks: 0,
        control_cost_usd: Some(0.0),
        candidate_cost_usd: Some(0.0),
    };
    let add_cost = |total: &mut Option<f64>, o: &Outcome| {
        *total = total.zip(o.cost_usd).map(|(t, c)| t + c);
    };
    for task in tasks {
        let (Some(c), Some(k)) = (ctrl.get(task), cand.get(task)) else {
            v.unpaired += 1;
            continue;
        };
        let (c1, k1) = (c[0], k[0]);
        if c1.infra || k1.infra {
            v.excluded_infra += 1;
            continue;
        }
        let disagrees = |list: &[&Outcome]| {
            let graded: Vec<bool> = list
                .iter()
                .filter(|o| !o.infra)
                .map(|o| o.resolved)
                .collect();
            graded.iter().any(|&r| r != graded[0])
        };
        if disagrees(c) || disagrees(k) {
            v.flaky_tasks += 1;
        }
        match (k1.resolved, c1.resolved) {
            (true, true) => v.pairs.both += 1,
            (false, true) => v.pairs.control_only += 1,
            (true, false) => v.pairs.candidate_only += 1,
            (false, false) => v.pairs.neither += 1,
        }
        add_cost(&mut v.control_cost_usd, c1);
        add_cost(&mut v.candidate_cost_usd, k1);
    }
    if v.pairs.n() == 0 {
        v.control_cost_usd = None;
        v.candidate_cost_usd = None;
        return v;
    }
    (v.diff, v.low95, v.high95) = paired_difference(v.pairs);
    v.decision = decide(v.low95, v.high95, margin);
    if v.decision == Decision::NotEnoughData {
        v.more_tasks = more_tasks_needed(v.pairs, margin);
    }
    v
}

fn points(share: f64) -> String {
    let p = (share * 100.0).round();
    if p > 0.0 {
        format!("+{p:.0}")
    } else {
        format!("{p:.0}")
    }
}

/// The verdict as one line for a terminal, a report or a CI log.
pub fn sentence(v: &Verdict) -> String {
    let n = v.pairs.n();
    if n == 0 {
        return format!(
            "NOT ENOUGH DATA: {} and {} have no task in common to compare.",
            v.candidate, v.control
        );
    }
    let range = format!(
        "{} vs {}: {} points (95% range {} to {}) on {n} paired task{}",
        v.candidate,
        v.control,
        points(v.diff),
        points(v.low95),
        points(v.high95),
        if n == 1 { "" } else { "s" }
    );
    let allowed = format!("{:.0} points", v.margin * 100.0);
    let mut s = match v.decision {
        Decision::Safe => format!("SAFE to switch: {range}; at most {allowed} worse is allowed."),
        Decision::NotSafe => {
            format!("NOT SAFE to switch: {range}; it is more than {allowed} worse.")
        }
        Decision::NotEnoughData => {
            let more = match v.more_tasks {
                Some(m) => format!("about {m} more paired tasks would settle it if the rates hold"),
                None => format!("the difference sits on the {allowed} margin"),
            };
            format!("NOT ENOUGH DATA: {range}; {more}.")
        }
    };
    if let Some(saving) = v.saving() {
        let pct = (saving * 100.0).round();
        s.push_str(&if pct >= 0.0 {
            format!(" Switching saves {pct:.0}% of the cost.")
        } else {
            format!(" Switching costs {:.0}% more.", -pct)
        });
    }
    let mut notes = Vec::new();
    if v.excluded_infra > 0 {
        notes.push(format!("{} left out for infra errors", v.excluded_infra));
    }
    if v.unpaired > 0 {
        notes.push(format!("{} attempted by only one model", v.unpaired));
    }
    if v.flaky_tasks > 0 {
        notes.push(format!("{} flaky across attempts", v.flaky_tasks));
    }
    if !notes.is_empty() {
        s.push_str(&format!(" Tasks: {}.", notes.join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(task: &str, model: &str, attempt: u32, resolved: bool) -> Outcome {
        Outcome {
            task: task.into(),
            model: model.into(),
            attempt,
            resolved,
            infra: false,
            cost_usd: Some(if model == "big" { 1.0 } else { 0.25 }),
        }
    }

    /// `n` tasks; the control resolves the first `c` and the candidate the first `k`.
    fn run(n: u32, c: u32, k: u32) -> Vec<Outcome> {
        (0..n)
            .flat_map(|i| {
                let t = format!("t{i}");
                [o(&t, "big", 1, i < c), o(&t, "small", 1, i < k)]
            })
            .collect()
    }

    #[test]
    fn wilson_is_honest_at_the_edges() {
        let (low, high) = wilson(5, 5);
        assert!(low > 0.5 && low < 0.6 && high == 1.0, "{low} {high}");
        let (low, high) = wilson(0, 5);
        assert!(low == 0.0 && high > 0.4 && high < 0.5, "{low} {high}");
        assert_eq!(wilson(0, 0), (0.0, 1.0));
        let (low, high) = wilson(50, 100);
        assert!((low + high - 1.0).abs() < 1e-12 && low > 0.39 && low < 0.41);
    }

    #[test]
    fn the_range_contains_the_difference_and_flips_with_the_models() {
        for pairs in [
            Pairs {
                both: 3,
                control_only: 2,
                candidate_only: 1,
                neither: 4,
            },
            Pairs {
                both: 10,
                control_only: 0,
                candidate_only: 0,
                neither: 0,
            },
            Pairs {
                both: 0,
                control_only: 7,
                candidate_only: 0,
                neither: 1,
            },
            Pairs {
                both: 1,
                control_only: 0,
                candidate_only: 0,
                neither: 0,
            },
        ] {
            let (d, low, high) = paired_difference(pairs);
            assert!(
                low <= d && d <= high && low >= -1.0 && high <= 1.0,
                "{pairs:?}"
            );
            let swapped = Pairs {
                control_only: pairs.candidate_only,
                candidate_only: pairs.control_only,
                ..pairs
            };
            let (d2, low2, high2) = paired_difference(swapped);
            assert!((d + d2).abs() < 1e-12);
            assert!((low + high2).abs() < 1e-12 && (high + low2).abs() < 1e-12);
        }
    }

    #[test]
    fn five_perfect_tasks_are_not_enough() {
        let v = verdict(&run(5, 5, 5), "big", "small", DEFAULT_MARGIN);
        assert_eq!(v.decision, Decision::NotEnoughData);
        assert_eq!(v.pairs.both, 5);
        assert!(v.low95 < -DEFAULT_MARGIN, "{}", v.low95);
        let more = v.more_tasks.expect("a size settles it");
        assert!((20..=60).contains(&more), "{more}");
        assert_eq!(v.saving(), Some(0.75));
        let s = sentence(&v);
        assert!(
            s.starts_with("NOT ENOUGH DATA") && s.contains(&format!("about {more} more")),
            "{s}"
        );
        assert_eq!(v.decision.exit_code(), 3);
    }

    #[test]
    fn the_estimate_is_reached_at_that_size() {
        let v = verdict(&run(5, 5, 5), "big", "small", DEFAULT_MARGIN);
        let total = 5 + v.more_tasks.expect("set");
        assert_eq!(
            verdict(&run(total, total, total), "big", "small", DEFAULT_MARGIN).decision,
            Decision::Safe
        );
        let short = total - 1;
        assert_eq!(
            verdict(&run(short, short, short), "big", "small", DEFAULT_MARGIN).decision,
            Decision::NotEnoughData
        );
    }

    #[test]
    fn many_equal_tasks_are_safe_and_a_clearly_worse_model_is_not() {
        let v = verdict(&run(200, 160, 158), "big", "small", DEFAULT_MARGIN);
        assert_eq!(v.decision, Decision::Safe, "{}", sentence(&v));
        assert_eq!(v.decision.exit_code(), 0);
        let v = verdict(&run(60, 54, 24), "big", "small", DEFAULT_MARGIN);
        assert_eq!(v.decision, Decision::NotSafe, "{}", sentence(&v));
        assert!(sentence(&v).starts_with("NOT SAFE") && v.decision.exit_code() == 2);
    }

    #[test]
    fn infra_errors_and_unpaired_tasks_are_left_out() {
        let mut runs = run(4, 4, 4);
        runs[1].infra = true;
        runs[1].resolved = false;
        runs.push(o("only-big", "big", 1, true));
        let v = verdict(&runs, "big", "small", DEFAULT_MARGIN);
        assert_eq!((v.pairs.n(), v.excluded_infra, v.unpaired), (3, 1, 1));
        let s = sentence(&v);
        assert!(
            s.contains("1 left out for infra errors")
                && s.contains("1 attempted by only one model"),
            "{s}"
        );
    }

    #[test]
    fn only_first_attempts_count_and_disagreeing_attempts_are_flaky() {
        let mut runs = run(3, 3, 3);
        runs.push(o("t0", "small", 2, false));
        runs.push(o("t0", "small", 3, false));
        let v = verdict(&runs, "big", "small", DEFAULT_MARGIN);
        assert_eq!(v.pairs.both, 3);
        assert_eq!(v.flaky_tasks, 1);
        assert_eq!(v.candidate_cost_usd, Some(0.75));
    }

    #[test]
    fn unpriced_attempts_leave_the_saving_unknown_and_no_overlap_is_handled() {
        let mut runs = run(3, 3, 3);
        runs[0].cost_usd = None;
        assert_eq!(
            verdict(&runs, "big", "small", DEFAULT_MARGIN).saving(),
            None
        );
        let v = verdict(&run(3, 3, 3), "big", "nobody", DEFAULT_MARGIN);
        assert_eq!(v.decision, Decision::NotEnoughData);
        assert_eq!(v.unpaired, 3);
        assert!(sentence(&v).contains("no task in common"));
    }

    #[test]
    fn a_dearer_candidate_is_said_to_cost_more() {
        // "big" costs 1.0 a task and "small" 0.25: switching from small to big costs 300% more.
        let v = verdict(&run(3, 3, 3), "small", "big", DEFAULT_MARGIN);
        assert!(
            sentence(&v).contains("Switching costs 300% more."),
            "{}",
            sentence(&v)
        );
        let v = verdict(&run(3, 3, 3), "big", "small", DEFAULT_MARGIN);
        assert!(sentence(&v).contains("Switching saves 75% of the cost."));
    }
}
