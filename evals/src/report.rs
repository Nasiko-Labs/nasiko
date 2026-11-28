//! The numbers in a run report: resolved rate, match rate against the control model, cost and
//! saving, all split by difficulty, with a 95% range for each rate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cost::{PriceQuality, TokenUsage};
use crate::grade::{FailureClass, Grade};
use crate::instance::Difficulty;
use crate::screen::level;

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

/// A continuous metric (cost or saving ratio) with its bootstrap 95% confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetricRange {
    pub value: f64,
    pub low95: f64,
    pub high95: f64,
    pub n: u32,
}

/// Position on the mean-cost-per-attempt vs. conservative-confidence (`resolved.low95`) frontier
/// across models in the same run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParetoStatus {
    /// No other priced model in the run achieves both at least as high `resolved.low95` and
    /// strictly lower mean cost per attempt (or strictly higher `resolved.low95` at equal/lower
    /// cost).
    ParetoOptimal,
    /// Another priced model in the run has at least as high `resolved.low95` at lower cost, or
    /// strictly higher `resolved.low95` at no higher cost.
    Dominated,
    /// Cannot be placed on the frontier (e.g. zero attempts or no priced attempts).
    InsufficientData,
}

/// Cost uncertainty, multi-attempt consistency, and cost-vs-confidence efficiency for one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostConfidence {
    /// Mean cost per priced attempt with task-clustered bootstrap 95% CI.
    pub mean_cost_per_attempt: Option<MetricRange>,
    /// Cost per resolved attempt over priced attempts with task-clustered bootstrap 95% CI.
    pub cost_per_resolved: Option<MetricRange>,
    /// Paired bootstrap 95% CI for `saving_vs_control` across shared priced first attempts.
    pub saving_vs_control_ci: Option<MetricRange>,
    /// Paired bootstrap 95% CI for `saving_vs_original` across priced first attempts with
    /// `original_cost_usd`.
    pub saving_vs_original_ci: Option<MetricRange>,
    /// Resolved priced attempts per US dollar spent (`None` when priced cost is zero).
    pub resolved_per_dollar: Option<f64>,
    /// `pass^k` across tasks with at least 2 attempts: share where every attempt resolved.
    pub consistency: Option<Rate>,
    /// Whether this model lies on the `(mean_cost_per_attempt, resolved.low95)` Pareto frontier.
    pub pareto_status: ParetoStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSummary {
    pub model: String,
    pub is_control: bool,
    pub attempts: u32,
    pub resolved: Rate,
    /// Share of the control's resolved tasks this model also resolved.
    pub match_rate: Option<Rate>,
    pub cost_usd: f64,
    pub cost_per_resolved: Option<f64>,
    /// 1 − (this model's cost ÷ the control's cost), over the same tasks.
    pub saving_vs_control: Option<f64>,
    /// 1 − (this model's cost ÷ the original sessions' cost), over the same tasks.
    pub saving_vs_original: Option<f64>,
    pub by_level: BTreeMap<String, Rate>,
    pub failures: BTreeMap<String, u32>,
    pub unpriced_attempts: u32,
    /// Cost uncertainty, multi-attempt consistency, and cost-vs-confidence frontier status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_confidence: Option<CostConfidence>,
}

/// Bootstrap rounds for the 95% ranges.
pub const BOOTSTRAP_ROUNDS: u32 = 1000;
/// Fixed seed, so the same records always give the same report.
pub const BOOTSTRAP_SEED: u64 = 0x5eed_2026;

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
    let mut summaries: Vec<ModelSummary> = models
        .iter()
        .map(|m| summarise_model(records, m, m == control, &control_wins, &control_cost))
        .collect();
    assign_pareto_status(&mut summaries);
    summaries
}

fn first_attempts<'a>(records: &'a [AttemptRecord], model: &str) -> Vec<&'a AttemptRecord> {
    records
        .iter()
        .filter(|r| r.model == model && r.attempt == 1)
        .collect()
}

fn summarise_model(
    records: &[AttemptRecord],
    model: &str,
    is_control: bool,
    control_wins: &BTreeMap<&str, bool>,
    control_cost: &BTreeMap<&str, f64>,
) -> ModelSummary {
    let mine: Vec<&AttemptRecord> = records.iter().filter(|r| r.model == model).collect();
    let hits: Vec<bool> = mine.iter().map(|r| r.grade.resolved).collect();
    let first = first_attempts(records, model);
    let matched: Vec<bool> = first
        .iter()
        .filter(|r| control_wins.get(r.instance_id.as_str()) == Some(&true))
        .map(|r| r.grade.resolved)
        .collect();
    let mut by_level: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    for r in &mine {
        let key = r.difficulty.map(level).unwrap_or("unknown").to_string();
        by_level.entry(key).or_default().push(r.grade.resolved);
    }
    let mut failures = BTreeMap::new();
    for f in mine.iter().filter_map(|r| r.grade.failure) {
        *failures.entry(failure_name(f).to_string()).or_insert(0) += 1;
    }
    let cost_usd: f64 = mine.iter().filter_map(|r| r.cost_usd).sum();
    let resolved = hits.iter().filter(|h| **h).count();
    let orig_pairs: Vec<(f64, f64)> = first
        .iter()
        .filter_map(|r| Some((r.cost_usd?, r.original_cost_usd?)))
        .collect();
    let (mine_vs_orig, orig) = sum_pairs(&orig_pairs);
    let control_pairs: Vec<(f64, f64)> = first
        .iter()
        .filter_map(|r| {
            let base = control_cost.get(r.instance_id.as_str())?;
            Some((r.cost_usd?, *base))
        })
        .collect();
    let (mine_on_shared, control_on_shared) = sum_pairs(&control_pairs);
    let cost_confidence = build_cost_confidence(&mine, is_control, &control_pairs, &orig_pairs);
    ModelSummary {
        model: model.to_string(),
        is_control,
        attempts: mine.len() as u32,
        resolved: rate(&hits),
        match_rate: (!is_control && !matched.is_empty()).then(|| rate(&matched)),
        cost_usd,
        cost_per_resolved: (resolved > 0).then(|| cost_usd / resolved as f64),
        saving_vs_control: (!is_control && control_on_shared > 0.0)
            .then(|| 1.0 - mine_on_shared / control_on_shared),
        saving_vs_original: (orig > 0.0).then(|| 1.0 - mine_vs_orig / orig),
        by_level: by_level.iter().map(|(k, v)| (k.clone(), rate(v))).collect(),
        failures,
        unpriced_attempts: mine.iter().filter(|r| r.cost_usd.is_none()).count() as u32,
        cost_confidence: Some(cost_confidence),
    }
}

fn build_cost_confidence(
    mine: &[&AttemptRecord],
    is_control: bool,
    control_pairs: &[(f64, f64)],
    orig_pairs: &[(f64, f64)],
) -> CostConfidence {
    // Group priced attempts by task so bootstrap resamples independent tasks rather than
    // correlated attempts on the same task.
    let mut by_task_priced: BTreeMap<&str, Vec<(f64, bool)>> = BTreeMap::new();
    let mut priced_count = 0_u32;
    let mut priced_cost = 0.0_f64;
    let mut priced_resolved = 0_u32;
    for r in mine {
        if let Some(c) = r.cost_usd {
            priced_count += 1;
            priced_cost += c;
            if r.grade.resolved {
                priced_resolved += 1;
            }
            by_task_priced
                .entry(r.instance_id.as_str())
                .or_default()
                .push((c, r.grade.resolved));
        }
    }
    let task_clusters: Vec<Vec<(f64, bool)>> = by_task_priced.into_values().collect();
    let mean_cost_per_attempt = (priced_count > 0).then(|| {
        let value = priced_cost / f64::from(priced_count);
        cluster_bootstrap_ci(&task_clusters, priced_count, value, |cost, count, _| {
            (count > 0).then(|| cost / f64::from(count))
        })
    });
    let cost_per_resolved = (priced_resolved > 0).then(|| {
        let value = priced_cost / f64::from(priced_resolved);
        cluster_bootstrap_ci(&task_clusters, priced_count, value, |cost, _, resolved| {
            (resolved > 0).then(|| cost / f64::from(resolved))
        })
    });
    let saving_vs_control_ci = (!is_control)
        .then(|| paired_saving_ci(control_pairs))
        .flatten();
    let saving_vs_original_ci = paired_saving_ci(orig_pairs);
    let resolved_per_dollar = (priced_cost > 0.0).then(|| f64::from(priced_resolved) / priced_cost);

    let mut by_task_all: BTreeMap<&str, Vec<bool>> = BTreeMap::new();
    for r in mine {
        by_task_all
            .entry(r.instance_id.as_str())
            .or_default()
            .push(r.grade.resolved);
    }
    let multi_task_consistency: Vec<bool> = by_task_all
        .into_values()
        .filter(|outcomes| outcomes.len() >= 2)
        .map(|outcomes| outcomes.into_iter().all(|ok| ok))
        .collect();
    let consistency = (!multi_task_consistency.is_empty()).then(|| rate(&multi_task_consistency));

    CostConfidence {
        mean_cost_per_attempt,
        cost_per_resolved,
        saving_vs_control_ci,
        saving_vs_original_ci,
        resolved_per_dollar,
        consistency,
        pareto_status: ParetoStatus::InsufficientData,
    }
}

fn sum_pairs(pairs: &[(f64, f64)]) -> (f64, f64) {
    pairs
        .iter()
        .fold((0.0, 0.0), |(a, b), &(x, y)| (a + x, b + y))
}

fn cluster_bootstrap_ci(
    clusters: &[Vec<(f64, bool)>],
    obs_count: u32,
    value: f64,
    metric: impl Fn(f64, u32, u32) -> Option<f64>,
) -> MetricRange {
    let k = clusters.len();
    let mut rng = SplitMix64(BOOTSTRAP_SEED);
    let mut samples = Vec::with_capacity(BOOTSTRAP_ROUNDS as usize);
    for _ in 0..BOOTSTRAP_ROUNDS {
        let mut cost = 0.0_f64;
        let mut count = 0_u32;
        let mut resolved = 0_u32;
        for _ in 0..k {
            for &(c, ok) in &clusters[rng.below(k)] {
                cost += c;
                count += 1;
                if ok {
                    resolved += 1;
                }
            }
        }
        if let Some(sample) = metric(cost, count, resolved) {
            samples.push(sample);
        }
    }
    let (low95, high95) = ci_bounds(&mut samples, value);
    MetricRange {
        value,
        low95,
        high95,
        n: obs_count,
    }
}

fn paired_saving_ci(pairs: &[(f64, f64)]) -> Option<MetricRange> {
    let (mine_sum, base_sum) = sum_pairs(pairs);
    if pairs.is_empty() || base_sum <= 0.0 {
        return None;
    }
    let value = 1.0 - mine_sum / base_sum;
    let n = pairs.len();
    let mut rng = SplitMix64(BOOTSTRAP_SEED);
    let mut samples = Vec::with_capacity(BOOTSTRAP_ROUNDS as usize);
    for _ in 0..BOOTSTRAP_ROUNDS {
        let (mut m, mut b) = (0.0_f64, 0.0_f64);
        for _ in 0..n {
            let (mc, bc) = pairs[rng.below(n)];
            m += mc;
            b += bc;
        }
        if b > 0.0 {
            samples.push(1.0 - m / b);
        }
    }
    let (low95, high95) = ci_bounds(&mut samples, value);
    Some(MetricRange {
        value,
        low95,
        high95,
        n: n as u32,
    })
}

fn assign_pareto_status(summaries: &mut [ModelSummary]) {
    const EPS: f64 = 1e-12;
    let points: Vec<Option<(f64, f64)>> = summaries
        .iter()
        .map(|s| {
            let cost = s
                .cost_confidence
                .as_ref()
                .and_then(|cc| cc.mean_cost_per_attempt)
                .map(|m| m.value)?;
            (s.attempts > 0).then_some((cost, s.resolved.low95))
        })
        .collect();

    for (i, summary) in summaries.iter_mut().enumerate() {
        let Some(cc) = summary.cost_confidence.as_mut() else {
            continue;
        };
        let Some((cost_i, low_i)) = points[i] else {
            cc.pareto_status = ParetoStatus::InsufficientData;
            continue;
        };
        let dominated = points.iter().enumerate().any(|(j, other)| {
            if j == i {
                return false;
            }
            let Some((cost_j, low_j)) = *other else {
                return false;
            };
            cost_j <= cost_i + EPS
                && low_j >= low_i - EPS
                && (cost_j < cost_i - EPS || low_j > low_i + EPS)
        });
        cc.pareto_status = if dominated {
            ParetoStatus::Dominated
        } else {
            ParetoStatus::ParetoOptimal
        };
    }
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

/// The share of `true`s, with a bootstrap 95% range.
pub fn rate(hits: &[bool]) -> Rate {
    let n = hits.len();
    if n == 0 {
        return Rate {
            value: 0.0,
            low95: 0.0,
            high95: 0.0,
            n: 0,
        };
    }
    let value = hits.iter().filter(|h| **h).count() as f64 / n as f64;
    let mut rng = SplitMix64(BOOTSTRAP_SEED);
    let mut samples: Vec<f64> = (0..BOOTSTRAP_ROUNDS)
        .map(|_| {
            let wins = (0..n).filter(|_| hits[rng.below(n)]).count();
            wins as f64 / n as f64
        })
        .collect();
    let (low95, high95) = ci_bounds(&mut samples, value);
    Rate {
        value,
        low95,
        high95,
        n: n as u32,
    }
}

fn ci_bounds(samples: &mut [f64], fallback: f64) -> (f64, f64) {
    if samples.is_empty() {
        return (fallback, fallback);
    }
    samples.sort_by(f64::total_cmp);
    let last = samples.len() - 1;
    let at = |q: f64| samples[(last as f64 * q).round() as usize];
    (at(0.025), at(0.975))
}

/// A tiny seeded generator, so reports need no `rand` crate and are repeatable.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// "GLM matched the original on 82% of tasks at 71% lower cost." for one model. Without a
/// control run the comparison is with the original sessions, whose real fixes solved every task.
pub fn headline(m: &ModelSummary, control: Option<&str>, dataset: &str) -> String {
    let matched = match (control, m.match_rate) {
        (Some(c), Some(r)) => format!(
            "matched {c} on {:.0}% of the tasks {c} solved",
            r.value * 100.0
        ),
        _ => format!(
            "matched the original sessions on {:.0}% of tasks",
            m.resolved.value * 100.0
        ),
    };
    let (saving, versus) = match control {
        Some(_) => (m.saving_vs_control, ""),
        None => (m.saving_vs_original, " than the original sessions"),
    };
    let cost = match saving {
        Some(s) if s >= 0.0 => format!(" at {:.0}% lower cost{versus}", s * 100.0),
        Some(s) => format!(" at {:.0}% higher cost{versus}", -s * 100.0),
        None => String::new(),
    };
    let confidence = m
        .cost_confidence
        .as_ref()
        .map(|cc| {
            let mut parts = Vec::new();
            let saving_ci = match control {
                Some(_) => cc.saving_vs_control_ci,
                None => cc.saving_vs_original_ci,
            };
            if let Some(ci) = saving_ci {
                parts.push(format!(
                    "saving 95% CI {:.0}%–{:.0}%",
                    ci.low95 * 100.0,
                    ci.high95 * 100.0
                ));
            }
            if let Some(cons) = cc.consistency {
                parts.push(format!("consistency {:.0}%", cons.value * 100.0));
            }
            if cc.pareto_status == ParetoStatus::ParetoOptimal {
                parts.push("pareto-optimal".to_string());
            }
            if parts.is_empty() {
                String::new()
            } else {
                format!(" [{}]", parts.join(", "))
            }
        })
        .unwrap_or_default();
    format!(
        "{} {matched}{cost}, on {dataset} ({} attempts; resolved 95% CI {:.0}%–{:.0}%){confidence}.",
        m.model,
        m.attempts,
        m.resolved.low95 * 100.0,
        m.resolved.high95 * 100.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grade::Resolution;

    fn rec(id: &str, model: &str, resolved: bool, cost: f64) -> AttemptRecord {
        rec_attempt(id, model, 1, resolved, Some(cost), None)
    }

    fn rec_attempt(
        id: &str,
        model: &str,
        attempt: u32,
        resolved: bool,
        cost: Option<f64>,
        original_cost_usd: Option<f64>,
    ) -> AttemptRecord {
        AttemptRecord {
            instance_id: id.into(),
            model: model.into(),
            attempt,
            difficulty: Some(Difficulty::Under15Min),
            grade: Grade {
                resolution: if resolved {
                    Resolution::Full
                } else {
                    Resolution::No
                },
                resolved,
                f2p_passed: u32::from(resolved),
                f2p_total: 1,
                p2p_passed: 0,
                p2p_total: 0,
                failure: (!resolved).then_some(FailureClass::TestsFailed),
            },
            usage: None,
            cost_usd: cost,
            worst_price: cost.map(|_| PriceQuality::Exact),
            wall_secs: 1,
            original_cost_usd,
        }
    }

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
            h.contains("matched claude on 50%")
                && h.contains("100% lower cost")
                && h.contains("resolved 95% CI")
                && h.contains("saving 95% CI 100%–100%"),
            "{h}"
        );
        let mut alone = rec("a", "flint", true, 0.0);
        alone.original_cost_usd = Some(4.0);
        let s = summarise(&[alone], "none");
        assert_eq!(s[0].saving_vs_original, Some(1.0));
        let h = headline(&s[0], None, "ds@1");
        assert!(
            h.contains("original sessions on 100%")
                && h.contains("lower cost than the original")
                && h.contains("pareto-optimal"),
            "{h}"
        );
    }

    #[test]
    fn ranges_are_repeatable_and_contain_the_value() {
        let hits = [
            true, true, false, true, false, true, true, false, true, true,
        ];
        let (a, b) = (rate(&hits), rate(&hits));
        assert_eq!(a, b);
        assert!(a.low95 <= a.value && a.value <= a.high95);
        assert_eq!(rate(&[]).n, 0);
    }

    #[test]
    fn cost_confidence_computes_deterministic_bootstrap_cis() {
        let records = vec![
            rec_attempt("t1", "flagship", 1, true, Some(2.0), Some(3.0)),
            rec_attempt("t2", "flagship", 1, true, Some(4.0), Some(5.0)),
            rec_attempt("t3", "flagship", 1, false, Some(2.0), Some(4.0)),
            rec_attempt("t4", "flagship", 1, true, Some(4.0), Some(4.0)),
            rec_attempt("t1", "flash", 1, true, Some(0.2), Some(3.0)),
            rec_attempt("t2", "flash", 1, true, Some(0.4), Some(5.0)),
            rec_attempt("t3", "flash", 1, false, Some(0.2), Some(4.0)),
            rec_attempt("t4", "flash", 1, false, Some(0.4), Some(4.0)),
        ];
        let a = summarise(&records, "flagship");
        let b = summarise(&records, "flagship");
        assert_eq!(a, b);

        let flash = &a[1];
        let cc = flash.cost_confidence.as_ref().expect("cost confidence");
        let mean_cost = cc.mean_cost_per_attempt.expect("mean cost");
        assert!((mean_cost.value - 0.3).abs() < 1e-9);
        assert_eq!(mean_cost.n, 4);
        assert!(mean_cost.low95 <= mean_cost.value && mean_cost.value <= mean_cost.high95);

        let cpr = cc.cost_per_resolved.expect("cost per resolved");
        assert!((cpr.value - 0.6).abs() < 1e-9);
        assert!(cpr.low95 <= cpr.value && cpr.value <= cpr.high95);

        let svc = cc.saving_vs_control_ci.expect("saving vs control");
        assert!((svc.value - 0.9).abs() < 1e-9);
        assert_eq!(svc.n, 4);
        assert!(svc.low95 <= svc.value && svc.value <= svc.high95);

        let svo = cc.saving_vs_original_ci.expect("saving vs original");
        assert!((svo.value - (1.0 - 1.2 / 16.0)).abs() < 1e-9);
        assert!(svo.low95 <= svo.value && svo.value <= svo.high95);

        assert!((cc.resolved_per_dollar.expect("efficiency") - (2.0 / 1.2)).abs() < 1e-9);
        assert_eq!(cc.consistency, None);
        assert!(
            a[0].cost_confidence
                .as_ref()
                .unwrap()
                .saving_vs_control_ci
                .is_none()
        );
    }

    #[test]
    fn multi_attempt_consistency_tracks_unanimous_tasks() {
        let records = vec![
            rec_attempt("t1", "flash", 1, true, Some(0.1), None),
            rec_attempt("t1", "flash", 2, true, Some(0.1), None),
            rec_attempt("t2", "flash", 1, true, Some(0.2), None),
            rec_attempt("t2", "flash", 2, false, Some(0.2), None),
            rec_attempt("t3", "flash", 1, false, Some(0.3), None),
        ];
        let s = summarise(&records, "none");
        let cc = s[0].cost_confidence.as_ref().expect("cost confidence");
        let cons = cc.consistency.expect("two multi-attempt tasks");
        assert_eq!(cons.n, 2);
        assert!((cons.value - 0.5).abs() < 1e-9);
        assert_eq!(cc.mean_cost_per_attempt.expect("mean").n, 5);
    }

    #[test]
    fn pareto_frontier_ranks_cost_vs_conservative_resolved_bound() {
        let mut records = Vec::new();
        for i in 0..8 {
            let id = format!("t{i}");
            // cheap_good: $0.10/attempt, 6/8 resolved
            records.push(rec(&id, "cheap_good", i < 6, 0.10));
            // expensive_best: $1.00/attempt, 8/8 resolved
            records.push(rec(&id, "expensive_best", true, 1.00));
            // dominated: $0.50/attempt, 4/8 resolved (worse & pricier than cheap_good)
            records.push(rec(&id, "dominated", i < 4, 0.50));
            // unpriced: no cost reported
            records.push(rec_attempt(&id, "unpriced", 1, true, None, None));
        }
        let s = summarise(&records, "expensive_best");
        let status_of = |name: &str| {
            s.iter()
                .find(|m| m.model == name)
                .and_then(|m| m.cost_confidence.as_ref())
                .map(|cc| cc.pareto_status)
                .unwrap()
        };
        assert_eq!(status_of("cheap_good"), ParetoStatus::ParetoOptimal);
        assert_eq!(status_of("expensive_best"), ParetoStatus::ParetoOptimal);
        assert_eq!(status_of("dominated"), ParetoStatus::Dominated);
        assert_eq!(status_of("unpriced"), ParetoStatus::InsufficientData);
    }

    #[test]
    fn model_summary_deserializes_legacy_json_without_cost_confidence() {
        let legacy = serde_json::json!({
            "model": "legacy",
            "is_control": false,
            "attempts": 1,
            "resolved": { "value": 1.0, "low95": 1.0, "high95": 1.0, "n": 1 },
            "match_rate": null,
            "cost_usd": 0.5,
            "cost_per_resolved": 0.5,
            "saving_vs_control": null,
            "saving_vs_original": null,
            "by_level": {},
            "failures": {},
            "unpriced_attempts": 0
        });
        let parsed: ModelSummary = serde_json::from_value(legacy).expect("legacy json");
        assert_eq!(parsed.cost_confidence, None);
    }
}
