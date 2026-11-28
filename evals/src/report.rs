//! The numbers in a run report: resolved rate, match rate against the control model, cost and
//! saving, all split by difficulty, with a 95% range for each rate.
//!
//! The unit of every rate is the task, not the attempt: a task counts once, by its first
//! attempt, however many attempts the run made. Repeated attempts of one task are not
//! independent trials, so pooling them would narrow the range without adding evidence; what
//! they do show is in [`crate::consistency`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cost::{PriceQuality, TokenUsage};
use crate::grade::{FailureClass, Grade};
use crate::instance::Difficulty;
use crate::screen::level;
use crate::stats;

/// One graded attempt, as stored in `attempt.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub instance_id: String,
    pub model: String,
    pub attempt: u32,
    pub difficulty: Option<Difficulty>,
    pub grade: Grade,
    /// Router totals for the attempt's tag; `None` when the router reported nothing.
    pub usage: Option<TokenUsage>,
    pub cost_usd: Option<f64>,
    pub worst_price: Option<PriceQuality>,
    pub wall_secs: u64,
    /// The original session's cost for the same task, priced by the router.
    pub original_cost_usd: Option<f64>,
}

/// A rate with its 95% range.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    pub value: f64,
    pub low95: f64,
    pub high95: f64,
    pub n: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSummary {
    pub model: String,
    pub is_control: bool,
    pub attempts: u32,
    /// Share of tasks resolved, each task counted once by its first attempt.
    pub resolved: Rate,
    /// Share of the control's resolved tasks this model also resolved.
    pub match_rate: Option<Rate>,
    pub cost_usd: f64,
    pub cost_per_resolved: Option<f64>,
    /// 1 − (this model's cost ÷ the control's cost), over the same tasks.
    pub saving_vs_control: Option<f64>,
    /// 1 − (this model's cost ÷ the original sessions' cost), over the same tasks.
    pub saving_vs_original: Option<f64>,
    /// Resolved rate per difficulty level, each task counted once by its first attempt.
    pub by_level: BTreeMap<String, Rate>,
    /// Failure reasons, counted over every attempt.
    pub failures: BTreeMap<String, u32>,
    pub unpriced_attempts: u32,
}

/// One summary per model, the control first.
pub fn summarise(records: &[AttemptRecord], control: &str) -> Vec<ModelSummary> {
    let mut models: Vec<String> = records.iter().map(|r| r.model.clone()).collect();
    models.sort();
    models.dedup();
    models.sort_by_key(|m| m != control);
    let control_wins: BTreeMap<&str, bool> = first_attempts(records, control)
        .into_iter()
        .map(|r| (r.instance_id.as_str(), r.grade.resolved))
        .collect();
    let control_cost: BTreeMap<&str, f64> = first_attempts(records, control)
        .into_iter()
        .filter_map(|r| Some((r.instance_id.as_str(), r.cost_usd?)))
        .collect();
    models
        .iter()
        .map(|m| summarise_model(records, m, m == control, &control_wins, &control_cost))
        .collect()
}

/// One record per task for `model`: its first attempt (the lowest-numbered one on record, so a
/// task whose attempt 1 left no record still counts).
fn first_attempts<'a>(records: &'a [AttemptRecord], model: &str) -> Vec<&'a AttemptRecord> {
    let mut first: BTreeMap<&str, &AttemptRecord> = BTreeMap::new();
    for r in records.iter().filter(|r| r.model == model) {
        let kept = first.entry(r.instance_id.as_str()).or_insert(r);
        if r.attempt < kept.attempt {
            *kept = r;
        }
    }
    first.into_values().collect()
}

fn summarise_model(
    records: &[AttemptRecord],
    model: &str,
    is_control: bool,
    control_wins: &BTreeMap<&str, bool>,
    control_cost: &BTreeMap<&str, f64>,
) -> ModelSummary {
    let mine: Vec<&AttemptRecord> = records.iter().filter(|r| r.model == model).collect();
    let firsts = first_attempts(records, model);
    let hits: Vec<bool> = firsts.iter().map(|r| r.grade.resolved).collect();
    let matched: Vec<bool> = firsts
        .iter()
        .filter(|r| control_wins.get(r.instance_id.as_str()) == Some(&true))
        .map(|r| r.grade.resolved)
        .collect();
    let mut by_level: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    for r in &firsts {
        let key = r.difficulty.map(level).unwrap_or("unknown").to_string();
        by_level.entry(key).or_default().push(r.grade.resolved);
    }
    let mut failures = BTreeMap::new();
    for f in mine.iter().filter_map(|r| r.grade.failure) {
        *failures.entry(failure_name(f).to_string()).or_insert(0) += 1;
    }
    let cost_usd: f64 = mine.iter().filter_map(|r| r.cost_usd).sum();
    let resolved_attempts = mine.iter().filter(|r| r.grade.resolved).count();
    let (mut mine_vs_orig, mut orig) = (0.0, 0.0);
    for r in &firsts {
        if let (Some(c), Some(o)) = (r.cost_usd, r.original_cost_usd) {
            mine_vs_orig += c;
            orig += o;
        }
    }
    let (mut mine_on_shared, mut control_on_shared) = (0.0, 0.0);
    for r in &firsts {
        if let (Some(c), Some(base)) = (r.cost_usd, control_cost.get(r.instance_id.as_str())) {
            mine_on_shared += c;
            control_on_shared += base;
        }
    }
    ModelSummary {
        model: model.to_string(),
        is_control,
        attempts: mine.len() as u32,
        resolved: rate(&hits),
        match_rate: (!is_control && !matched.is_empty()).then(|| rate(&matched)),
        cost_usd,
        cost_per_resolved: (resolved_attempts > 0).then(|| cost_usd / resolved_attempts as f64),
        saving_vs_control: (!is_control && control_on_shared > 0.0)
            .then(|| 1.0 - mine_on_shared / control_on_shared),
        saving_vs_original: (orig > 0.0).then(|| 1.0 - mine_vs_orig / orig),
        by_level: by_level.iter().map(|(k, v)| (k.clone(), rate(v))).collect(),
        failures,
        unpriced_attempts: mine.iter().filter(|r| r.cost_usd.is_none()).count() as u32,
    }
}

/// Per model, the tasks whose first attempt failed on the sandbox, not on the model's work
/// ([`FailureClass::InfraError`]). The rates count a task by its first attempt, so each of
/// these reads as "not resolved" whatever later attempts did; a report names them so that a
/// flaky sandbox is not taken for a weak model. Models without such a task have no entry.
pub fn infra_failed_first_attempts(records: &[AttemptRecord]) -> BTreeMap<String, Vec<String>> {
    let mut models: Vec<&str> = records.iter().map(|r| r.model.as_str()).collect();
    models.sort();
    models.dedup();
    let failed_tasks = |model: &str| -> Vec<String> {
        first_attempts(records, model)
            .into_iter()
            .filter(|r| r.grade.failure == Some(FailureClass::InfraError))
            .map(|r| r.instance_id.clone())
            .collect()
    };
    models
        .into_iter()
        .map(|model| (model.to_string(), failed_tasks(model)))
        .filter(|(_, tasks)| !tasks.is_empty())
        .collect()
}

fn failure_name(f: FailureClass) -> &'static str {
    match f {
        FailureClass::InfraError => "infra_error",
        FailureClass::AgentCrashed => "agent_crashed",
        FailureClass::NoPatch => "no_patch",
        FailureClass::Budget => "budget",
        FailureClass::PatchFailed => "patch_failed",
        FailureClass::BuildFailed => "build_failed",
        FailureClass::Regression => "regression",
        FailureClass::TestsFailed => "tests_failed",
    }
}

/// The share of `true`s, with its 95% Wilson score range ([`stats::wilson95`]). The same input
/// always gives the same range, and all-true or all-false still gives a range with real width.
pub fn rate(hits: &[bool]) -> Rate {
    let wins = hits.iter().filter(|h| **h).count();
    rate_of(wins as u32, hits.len() as u32)
}

/// [`rate`] from counts: `hits` of `n`. More hits than `n` count as `n`: a share is never above
/// one.
pub fn rate_of(hits: u32, n: u32) -> Rate {
    let hits = hits.min(n);
    let (low95, high95) = stats::wilson95(hits, n);
    let value = if n == 0 {
        0.0
    } else {
        f64::from(hits) / f64::from(n)
    };
    Rate {
        value,
        low95,
        high95,
        n,
    }
}

/// One task's outcome for one model: how many of its attempts resolved the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskCell {
    pub resolved: u32,
    pub attempts: u32,
}

/// One row of the per-task table: a task and its outcome for every model that tried it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRow {
    pub instance_id: String,
    /// Model name → outcome; a model with no attempt on this task has no entry.
    pub cells: BTreeMap<String, TaskCell>,
}

/// The per-task view of a run: one row per task, sorted by task id.
pub fn task_table(records: &[AttemptRecord]) -> Vec<TaskRow> {
    let mut rows: BTreeMap<&str, BTreeMap<String, TaskCell>> = BTreeMap::new();
    for r in records {
        let cell = rows
            .entry(r.instance_id.as_str())
            .or_default()
            .entry(r.model.clone())
            .or_default();
        cell.attempts += 1;
        cell.resolved += u32::from(r.grade.resolved);
    }
    rows.into_iter()
        .map(|(id, cells)| TaskRow {
            instance_id: id.to_string(),
            cells,
        })
        .collect()
}

/// "GLM matched the original on 82% of tasks at 71% lower cost." for one model. Without a
/// control run the comparison is with the original sessions, whose real fixes solved every task.
pub fn headline(m: &ModelSummary, control: Option<&str>, dataset: &str) -> String {
    headline_for(m, control, dataset, true)
}

/// The headline for a dataset whose tasks have no original session, such as one built from
/// commit history: there is no session to match and no original cost, so it states the
/// resolved rate and compares cost with the control only.
pub fn headline_without_sessions(m: &ModelSummary, control: Option<&str>, dataset: &str) -> String {
    headline_for(m, control, dataset, false)
}

fn headline_for(
    m: &ModelSummary,
    control: Option<&str>,
    dataset: &str,
    has_sessions: bool,
) -> String {
    let matched = match (control, m.match_rate) {
        (Some(c), Some(r)) => format!(
            "matched {c} on {:.0}% of the tasks {c} solved",
            r.value * 100.0
        ),
        _ if has_sessions => format!(
            "matched the original sessions on {:.0}% of tasks",
            m.resolved.value * 100.0
        ),
        _ => format!("resolved {:.0}% of tasks", m.resolved.value * 100.0),
    };
    let (saving, versus) = match control {
        Some(_) => (m.saving_vs_control, ""),
        None if has_sessions => (m.saving_vs_original, " than the original sessions"),
        None => (None, ""),
    };
    let cost = match saving {
        Some(s) if s >= 0.0 => format!(" at {:.0}% lower cost{versus}", s * 100.0),
        Some(s) => format!(" at {:.0}% higher cost{versus}", -s * 100.0),
        None => String::new(),
    };
    format!(
        "{} {matched}{cost}, on {dataset} ({} attempts).",
        m.model, m.attempts
    )
}

/// Attempt records for this crate's tests.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::grade::Resolution;

    /// Attempt 1 of `id` by `model`, costing `cost`.
    pub(crate) fn rec(id: &str, model: &str, resolved: bool, cost: f64) -> AttemptRecord {
        AttemptRecord {
            instance_id: id.into(),
            model: model.into(),
            attempt: 1,
            difficulty: Some(Difficulty::Under15Min),
            grade: Grade {
                resolution: if resolved {
                    Resolution::Full
                } else {
                    Resolution::No
                },
                resolved,
                f2p_passed: 0,
                f2p_total: 1,
                p2p_passed: 0,
                p2p_total: 0,
                failure: (!resolved).then_some(FailureClass::TestsFailed),
            },
            usage: None,
            cost_usd: Some(cost),
            worst_price: Some(PriceQuality::Exact),
            wall_secs: 1,
            original_cost_usd: None,
        }
    }

    /// Attempt `attempt` of `id` by `model`, costing 1.
    pub(crate) fn nth(id: &str, model: &str, attempt: u32, resolved: bool) -> AttemptRecord {
        AttemptRecord {
            attempt,
            ..rec(id, model, resolved, 1.0)
        }
    }

    /// Attempt `attempt` of `id` by `model` that failed on the sandbox.
    pub(crate) fn infra_failed(id: &str, model: &str, attempt: u32) -> AttemptRecord {
        let mut record = nth(id, model, attempt, false);
        record.grade.failure = Some(FailureClass::InfraError);
        record
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{infra_failed, nth, rec};
    use super::*;

    #[test]
    fn match_rate_and_saving_are_against_the_control() {
        let records = vec![
            rec("a", "claude", true, 1.0),
            rec("b", "claude", true, 1.0),
            rec("c", "claude", false, 1.0),
            rec("a", "flint", true, 0.0),
            rec("b", "flint", false, 0.0),
            rec("c", "flint", true, 0.0),
        ];
        let s = summarise(&records, "claude");
        assert_eq!(s[0].model, "claude");
        let flint = &s[1];
        assert!((flint.match_rate.expect("set").value - 0.5).abs() < 1e-9);
        assert!((flint.resolved.value - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(flint.saving_vs_control, Some(1.0));
        assert_eq!(flint.failures.get("tests_failed"), Some(&1));
        let h = headline(flint, Some("claude"), "ds@1");
        assert!(
            h.contains("matched claude on 50%") && h.contains("100% lower cost"),
            "{h}"
        );
        let mut alone = rec("a", "flint", true, 0.0);
        alone.original_cost_usd = Some(4.0);
        let s = summarise(&[alone], "none");
        assert_eq!(s[0].saving_vs_original, Some(1.0));
        let h = headline(&s[0], None, "ds@1");
        assert!(
            h.contains("original sessions on 100%") && h.contains("lower cost than the original"),
            "{h}"
        );
    }

    #[test]
    fn a_dataset_without_sessions_never_claims_to_match_them() {
        let mut alone = rec("a", "flint", true, 1.0);
        alone.original_cost_usd = Some(0.0);
        let s = summarise(&[alone], "none");
        let h = headline_without_sessions(&s[0], None, "ds@1");
        assert_eq!(h, "flint resolved 100% of tasks, on ds@1 (1 attempts).");

        let records = [rec("a", "claude", true, 4.0), rec("a", "flint", true, 1.0)];
        let s = summarise(&records, "claude");
        let flint = s.iter().find(|m| m.model == "flint").unwrap();
        let h = headline_without_sessions(flint, Some("claude"), "ds@1");
        assert!(
            h.contains("matched claude on 100%") && h.contains("75% lower cost"),
            "{h}"
        );
        assert!(!h.contains("original sessions"), "{h}");
    }

    #[test]
    fn ranges_are_repeatable_and_contain_the_value() {
        let hits = [
            true, true, false, true, false, true, true, false, true, true,
        ];
        let (a, b) = (rate(&hits), rate(&hits));
        assert_eq!(a, b);
        assert!(a.low95 <= a.value && a.value <= a.high95);
        let nothing = Rate {
            value: 0.0,
            low95: 0.0,
            high95: 0.0,
            n: 0,
        };
        assert_eq!(rate(&[]), nothing);
    }

    #[test]
    fn all_resolved_and_none_resolved_keep_a_real_range() {
        let all = rate(&[true; 10]);
        let (low, high) = stats::wilson95(10, 10);
        assert_eq!((all.value, all.low95, all.high95), (1.0, low, high));
        assert!(all.low95 < 0.75 && all.high95 == 1.0, "{all:?}");
        let none = rate(&[false; 10]);
        assert!(none.low95 == 0.0 && none.high95 > 0.25, "{none:?}");
        assert_eq!(rate_of(5, 10), rate(&[true, false].repeat(5)));
        assert_eq!(rate_of(3, 2), rate_of(2, 2), "a share is never above one");
    }

    #[test]
    fn first_attempts_lost_to_the_sandbox_are_named_per_model() {
        let records = vec![
            infra_failed("a", "flint", 1),
            nth("a", "flint", 2, true),
            nth("b", "flint", 1, true),
            infra_failed("b", "flint", 2),
            nth("a", "claude", 1, false),
        ];
        let lost = infra_failed_first_attempts(&records);
        assert_eq!(lost.len(), 1, "{lost:?}");
        assert_eq!(lost["flint"], vec!["a"]);
        // The rate still counts the task by that first attempt.
        assert_eq!(
            summarise(&records, "none")[1].resolved,
            rate(&[false, true])
        );
    }

    #[test]
    fn repeated_attempts_do_not_add_tasks_to_the_resolved_rate() {
        // Two tasks, three attempts each: the rate is over 2 tasks, by first attempt.
        let records = vec![
            nth("a", "flint", 1, true),
            nth("a", "flint", 2, false),
            nth("a", "flint", 3, false),
            nth("b", "flint", 1, false),
            nth("b", "flint", 2, true),
            nth("b", "flint", 3, true),
        ];
        let flint = &summarise(&records, "none")[0];
        assert_eq!(flint.attempts, 6);
        assert_eq!(flint.resolved, rate(&[true, false]));
        assert_eq!(flint.by_level["easy"].n, 2);
        assert_eq!(flint.failures.get("tests_failed"), Some(&3));
        // Cost stays every attempt's, shared over every resolved attempt.
        assert_eq!(flint.cost_usd, 6.0);
        assert_eq!(flint.cost_per_resolved, Some(2.0));
    }

    #[test]
    fn a_task_without_attempt_one_counts_by_its_earliest_attempt() {
        let records = vec![nth("a", "flint", 3, false), nth("a", "flint", 2, true)];
        let flint = &summarise(&records, "none")[0];
        assert_eq!(flint.resolved, rate(&[true]));
    }

    #[test]
    fn the_task_table_counts_resolved_attempts_per_model() {
        let records = vec![
            nth("b", "flint", 1, true),
            nth("b", "flint", 2, false),
            nth("a", "flint", 1, false),
            nth("a", "claude", 1, true),
        ];
        let table = task_table(&records);
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].instance_id, "a");
        let b_flint = TaskCell {
            resolved: 1,
            attempts: 2,
        };
        assert_eq!(table[1].cells["flint"], b_flint);
        assert!(!table[1].cells.contains_key("claude"));
    }
}
