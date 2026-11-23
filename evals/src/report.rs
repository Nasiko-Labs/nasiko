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
    models
        .iter()
        .map(|m| summarise_model(records, m, m == control, &control_wins, &control_cost))
        .collect()
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
    let matched: Vec<bool> = first_attempts(records, model)
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
    let (mut mine_vs_orig, mut orig) = (0.0, 0.0);
    for r in first_attempts(records, model) {
        if let (Some(c), Some(o)) = (r.cost_usd, r.original_cost_usd) {
            mine_vs_orig += c;
            orig += o;
        }
    }
    let (mut mine_on_shared, mut control_on_shared) = (0.0, 0.0);
    for r in first_attempts(records, model) {
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
        cost_per_resolved: (resolved > 0).then(|| cost_usd / resolved as f64),
        saving_vs_control: (!is_control && control_on_shared > 0.0)
            .then(|| 1.0 - mine_on_shared / control_on_shared),
        saving_vs_original: (orig > 0.0).then(|| 1.0 - mine_vs_orig / orig),
        by_level: by_level.iter().map(|(k, v)| (k.clone(), rate(v))).collect(),
        failures,
        unpriced_attempts: mine.iter().filter(|r| r.cost_usd.is_none()).count() as u32,
    }
}

fn failure_name(f: FailureClass) -> &'static str {
    match f {
        FailureClass::InfraError => "infra_error",
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
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    Rate {
        value,
        low95: at(0.025),
        high95: at(0.975),
        n: n as u32,
    }
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
    format!(
        "{} {matched}{cost}, on {dataset} ({} attempts).",
        m.model, m.attempts
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grade::Resolution;

    fn rec(id: &str, model: &str, resolved: bool, cost: f64) -> AttemptRecord {
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
    fn ranges_are_repeatable_and_contain_the_value() {
        let hits = [
            true, true, false, true, false, true, true, false, true, true,
        ];
        let (a, b) = (rate(&hits), rate(&hits));
        assert_eq!(a, b);
        assert!(a.low95 <= a.value && a.value <= a.high95);
        assert_eq!(rate(&[]).n, 0);
    }
}
