//! Deterministic reference-finding matching: a model finding matches a reference finding when
//! they name the same file and category and a line within `LINE_TOLERANCE` of each other.
//! Matching is greedy and one-to-one — a reference matches at most one model finding and vice
//! versa — so a model cannot inflate its own recall by repeating the same finding. This is the
//! only matching method this slice ships; the brief allows semantic/LLM-assisted matching only
//! as a *supplement* to a deterministic method, never as a replacement, and that supplement is
//! not built here (`oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`).

use super::api::{Metrics, ModelFinding, ReferenceFinding};

const LINE_TOLERANCE: i64 = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct GradeResult {
    /// One entry per reference finding: the model finding index it matched, if any.
    pub matches: Vec<Option<usize>>,
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub precision: f64,
    pub recall: f64,
    /// Among true positives, the fraction whose severity matched exactly; `None` with none.
    pub severity_accuracy: Option<f64>,
}

/// Grades one model's findings against a task's human-validated reference findings. Call only
/// when the task is graded (`reference` non-empty); an ungraded task's attempt must stay an
/// unscored preview instead.
pub fn grade(reference: &[ReferenceFinding], model: &[ModelFinding]) -> GradeResult {
    let mut claimed = vec![false; model.len()];
    let mut matches = vec![None; reference.len()];
    for (ri, r) in reference.iter().enumerate() {
        let mut best: Option<(usize, i64)> = None;
        for (mi, m) in model.iter().enumerate() {
            if claimed[mi] || !same_file(&m.file, &r.file) || m.category != r.category {
                continue;
            }
            let distance = (m.line - r.line).abs();
            if distance > LINE_TOLERANCE {
                continue;
            }
            if best
                .map(|(_, best_distance)| distance < best_distance)
                .unwrap_or(true)
            {
                best = Some((mi, distance));
            }
        }
        if let Some((mi, _)) = best {
            claimed[mi] = true;
            matches[ri] = Some(mi);
        }
    }
    let true_positives = matches.iter().filter(|m| m.is_some()).count();
    let false_negatives = reference.len() - true_positives;
    let false_positives = model.len() - true_positives;
    let severity_accuracy = (true_positives > 0).then(|| {
        let exact = matches
            .iter()
            .enumerate()
            .filter_map(|(ri, m)| m.map(|mi| (ri, mi)))
            .filter(|(ri, mi)| reference[*ri].severity == model[*mi].severity)
            .count();
        exact as f64 / true_positives as f64
    });
    GradeResult {
        matches,
        true_positives,
        false_positives,
        false_negatives,
        precision: ratio(true_positives, true_positives + false_positives),
        recall: ratio(true_positives, true_positives + false_negatives),
        severity_accuracy,
    }
}

/// A finding naming nothing (model omitted the category) never matches by category; a finding
/// naming the same path matches. Paths are compared as given — the prompt asks for the diff's own
/// paths, so no normalisation beyond an exact match is attempted at this scope.
fn same_file(a: &str, b: &str) -> bool {
    a == b
}

/// A ratio that reads as 0, not a misleading 1, when nothing was predicted or nothing existed to
/// find: "no positives" is not "perfect precision".
fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

impl From<&GradeResult> for Metrics {
    fn from(grade: &GradeResult) -> Self {
        Metrics {
            true_positives: grade.true_positives,
            false_positives: grade.false_positives,
            false_negatives: grade.false_negatives,
            precision: grade.precision,
            recall: grade.recall,
            severity_accuracy: grade.severity_accuracy,
        }
    }
}

/// The least certain price source among an attempt's calls (mirrors
/// `evals::usage::worst_source`, duplicated here because that module is private to `evals` — see
/// the proposal doc's "known limitations" for the follow-up to share it instead).
pub fn worst_price_source(recorded: &[String]) -> String {
    const ORDER: [&str; 5] = ["Exact", "Family", "Static", "Ratio", "Default"];
    recorded
        .iter()
        .map(|name| {
            ORDER
                .iter()
                .position(|o| o == name)
                .unwrap_or(ORDER.len() - 1)
        })
        .max()
        .map(|i| ORDER[i.min(ORDER.len() - 1)].to_string())
        .unwrap_or_else(|| "Default".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::api::Category;
    use super::super::api::Severity;
    use super::super::api::Severity::{Critical, High};

    fn ref_finding(
        file: &str,
        line: i64,
        category: Category,
        severity: Severity,
    ) -> ReferenceFinding {
        ReferenceFinding {
            file: file.to_string(),
            line,
            severity,
            category,
            description: "ref".to_string(),
        }
    }

    fn model_finding(
        file: &str,
        line: i64,
        category: Category,
        severity: Severity,
    ) -> ModelFinding {
        ModelFinding {
            file: file.to_string(),
            line,
            severity,
            category,
            description: "found".to_string(),
            confidence: 0.9,
        }
    }

    #[test]
    fn an_exact_match_is_a_true_positive() {
        let reference = vec![ref_finding("a.rs", 10, Category::Bug, Critical)];
        let model = vec![model_finding("a.rs", 10, Category::Bug, Critical)];
        let result = grade(&reference, &model);
        assert_eq!(result.true_positives, 1);
        assert_eq!(result.false_positives, 0);
        assert_eq!(result.false_negatives, 0);
        assert_eq!(result.precision, 1.0);
        assert_eq!(result.recall, 1.0);
        assert_eq!(result.severity_accuracy, Some(1.0));
    }

    #[test]
    fn a_line_within_tolerance_still_matches() {
        let reference = vec![ref_finding("a.rs", 10, Category::Bug, Critical)];
        let model = vec![model_finding("a.rs", 12, Category::Bug, Critical)];
        assert_eq!(grade(&reference, &model).true_positives, 1);
    }

    #[test]
    fn a_line_past_tolerance_is_a_miss_and_a_false_positive() {
        let reference = vec![ref_finding("a.rs", 10, Category::Bug, Critical)];
        let model = vec![model_finding("a.rs", 20, Category::Bug, Critical)];
        let result = grade(&reference, &model);
        assert_eq!(result.true_positives, 0);
        assert_eq!(result.false_negatives, 1);
        assert_eq!(result.false_positives, 1);
        assert_eq!(result.precision, 0.0);
        assert_eq!(result.recall, 0.0);
    }

    #[test]
    fn a_category_mismatch_does_not_match() {
        let reference = vec![ref_finding("a.rs", 10, Category::Security, Critical)];
        let model = vec![model_finding("a.rs", 10, Category::Quality, Critical)];
        assert_eq!(grade(&reference, &model).true_positives, 0);
    }

    #[test]
    fn duplicate_model_findings_cannot_both_match_the_same_reference() {
        let reference = vec![ref_finding("a.rs", 10, Category::Bug, Critical)];
        let model = vec![
            model_finding("a.rs", 10, Category::Bug, Critical),
            model_finding("a.rs", 11, Category::Bug, Critical),
        ];
        let result = grade(&reference, &model);
        assert_eq!(result.true_positives, 1);
        assert_eq!(
            result.false_positives, 1,
            "the second duplicate is unmatched, not free"
        );
    }

    #[test]
    fn severity_accuracy_only_counts_true_positives() {
        let reference = vec![
            ref_finding("a.rs", 10, Category::Bug, Critical),
            ref_finding("b.rs", 1, Category::Bug, High),
        ];
        let model = vec![model_finding("a.rs", 10, Category::Bug, High)];
        let result = grade(&reference, &model);
        assert_eq!(result.true_positives, 1);
        assert_eq!(
            result.severity_accuracy,
            Some(0.0),
            "matched but wrong severity"
        );
    }

    #[test]
    fn no_reference_findings_means_every_model_finding_is_unmatched() {
        let model = vec![model_finding("a.rs", 1, Category::Bug, Critical)];
        let result = grade(&[], &model);
        assert_eq!(result.false_positives, 1);
        assert_eq!(
            result.precision, 0.0,
            "nothing was predicted correctly, by definition"
        );
        assert_eq!(
            result.recall, 0.0,
            "no denominator reads as 0, not a misleading 1"
        );
        assert_eq!(result.severity_accuracy, None);
    }

    #[test]
    fn empty_model_output_is_zero_recall_not_a_crash() {
        let reference = vec![ref_finding("a.rs", 10, Category::Bug, Critical)];
        let result = grade(&reference, &[]);
        assert_eq!(result.recall, 0.0);
        assert_eq!(result.false_negatives, 1);
    }

    #[test]
    fn worst_source_is_the_least_certain_one() {
        assert_eq!(worst_price_source(&["Exact".into()]), "Exact");
        assert_eq!(
            worst_price_source(&["Exact".into(), "Ratio".into()]),
            "Ratio"
        );
        assert_eq!(worst_price_source(&[]), "Default");
        assert_eq!(worst_price_source(&["Unheard".into()]), "Default");
    }
}
