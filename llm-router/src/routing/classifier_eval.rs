//! Evaluation I/O and metrics for the request classifier.
//!
//! The harness contract (participant brief, "How we run it"):
//!
//! ```sh
//! EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//! cargo run --release -p nasiko-llm-router --example classifier_eval
//! ```
//!
//! - `EVAL_SET` is a JSON file with an `examples` array; each case has `id`, `query` and
//!   an optional `context`. Any other field (`request_type`, `complexity`,
//!   `tier_hypothesis`, `tests`) is a **label** and is never read by the inference path:
//!   [`read_eval_cases`] does not even parse them.
//! - `OUT` gets one JSONL row per case, in input order, with exactly `id`,
//!   `request_type`, integer `complexity`, numeric `confidence` and measured `latency_us`.
//!   Nothing else goes in that file — the schema is the harness's, not ours.
//! - Diagnostics (which backend answered, fallback reason, hosted distributions, token
//!   usage, model version) go to a **sidecar** JSONL next to `OUT` (`<OUT>.diagnostics.jsonl`,
//!   or `DIAG_OUT`), and a run summary goes to stderr.
//! - Exit 0 means the eval ran. A missing/unreadable set, a malformed case, or an output
//!   that cannot be written is a fatal, actionable error.
//!
//! `latency_us` is the service's wall-clock for the whole decision (preparation, hosted
//! round trip, validation, fallback). One-time initialization is reported separately.
//!
//! [`metrics`] is the scorer side: given labels and predictions it computes the figures
//! the report prints. It lives in the library so its arithmetic is unit-tested on
//! hand-checkable fixtures.

use std::collections::BTreeMap;
use std::io::Write;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::classifier::{BackendDiagnostics, ClassifyInput, RequestType};
use super::classifier_service::{BackendStatus, ClassifierService, ClassifierStats, Disposition};

/// What the inference path is allowed to see from a case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalCase {
    pub id: String,
    pub query: String,
    pub context: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("EVAL_SET is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("EVAL_SET has no `examples` array at the top level")]
    NoExamples,
    #[error("case #{index}: {problem}")]
    BadCase { index: usize, problem: String },
    #[error("duplicate case id '{0}' (ids must be unique so rows can be matched to cases)")]
    DuplicateId(String),
    #[error("failed to write output: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse the harness file. Reads only `id`, `query` and `context`; labels are ignored by
/// construction. `context` may be absent, `null`, or a string (an empty string counts as
/// absent).
pub fn read_eval_cases(raw: &str) -> Result<Vec<EvalCase>, EvalError> {
    let data: Value = serde_json::from_str(raw)?;
    let examples = data
        .get("examples")
        .and_then(Value::as_array)
        .ok_or(EvalError::NoExamples)?;
    let mut seen = std::collections::HashSet::new();
    let mut cases = Vec::with_capacity(examples.len());
    for (index, ex) in examples.iter().enumerate() {
        let id = ex
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EvalError::BadCase {
                index,
                problem: "missing or empty string `id`".into(),
            })?;
        let query = ex
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| EvalError::BadCase {
                index,
                problem: format!("case '{id}': missing string `query`"),
            })?;
        let context = match ex.get("context") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.trim().is_empty() => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(EvalError::BadCase {
                    index,
                    problem: format!("case '{id}': `context` must be a string or null"),
                });
            }
        };
        if !seen.insert(id.to_string()) {
            return Err(EvalError::DuplicateId(id.to_string()));
        }
        cases.push(EvalCase {
            id: id.to_string(),
            query: query.to_string(),
            context,
        });
    }
    Ok(cases)
}

/// One `OUT` row — the harness schema, nothing more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalRow {
    pub id: String,
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
    pub latency_us: u64,
}

/// One sidecar row: everything about how the `OUT` row came to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagRow {
    pub id: String,
    pub configured_backend: String,
    pub answered_by: String,
    pub disposition: Disposition,
    pub input_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<BackendDiagnostics>,
}

/// Aggregate facts about one run, for the stderr summary and the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalSummary {
    pub cases: usize,
    pub backend: BackendStatus,
    pub stats: ClassifierStats,
    pub init_time_us: u64,
    pub latency_us_p50: u64,
    pub latency_us_p95: u64,
    pub latency_us_total: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Model versions the hosted backend reported, with counts.
    pub model_versions: BTreeMap<String, u64>,
    pub rubric_version: String,
    pub truncated_inputs: u64,
}

/// Run every case through the shared service, writing rows as they complete (so a run
/// that dies mid-way still leaves a prefix of valid rows). Returns the summary.
pub async fn run_eval(
    service: &ClassifierService,
    cases: &[EvalCase],
    out: &mut dyn Write,
    mut diag: Option<&mut dyn Write>,
) -> Result<EvalSummary, EvalError> {
    let configured = service.status().configured.as_str().to_string();
    let mut latencies = Vec::with_capacity(cases.len());
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut model_versions = BTreeMap::new();
    let mut truncated_inputs = 0u64;
    for case in cases {
        let outcome = service
            .classify(&ClassifyInput {
                query: &case.query,
                context: case.context.as_deref(),
            })
            .await;
        let latency_us = outcome.latency.as_micros() as u64;
        latencies.push(latency_us);
        let row = EvalRow {
            id: case.id.clone(),
            request_type: outcome.classification.request_type,
            complexity: outcome.classification.complexity,
            confidence: outcome.classification.confidence,
            latency_us,
        };
        // Explicit key order so two runs diff cleanly on the semantic fields.
        writeln!(
            out,
            "{}",
            json!({
                "id": row.id,
                "request_type": row.request_type,
                "complexity": row.complexity,
                "confidence": row.confidence,
                "latency_us": row.latency_us,
            })
        )?;
        if let Some(d) = &outcome.diagnostics {
            input_tokens += d.input_tokens.unwrap_or(0);
            output_tokens += d.output_tokens.unwrap_or(0);
            if let Some(v) = &d.model_version {
                *model_versions.entry(v.clone()).or_insert(0) += 1;
            }
        }
        if outcome.input_truncated {
            truncated_inputs += 1;
        }
        if let Some(w) = diag.as_deref_mut() {
            let d = DiagRow {
                id: case.id.clone(),
                configured_backend: configured.clone(),
                answered_by: outcome.answered_by.clone(),
                disposition: outcome.disposition.clone(),
                input_truncated: outcome.input_truncated,
                diagnostics: outcome.diagnostics.clone(),
            };
            writeln!(w, "{}", serde_json::to_string(&d)?)?;
        }
    }
    out.flush()?;
    if let Some(w) = diag {
        w.flush()?;
    }
    Ok(EvalSummary {
        cases: cases.len(),
        backend: service.status(),
        stats: service.stats(),
        init_time_us: service.init_time().as_micros() as u64,
        latency_us_p50: metrics::percentile(&latencies, 50.0),
        latency_us_p95: metrics::percentile(&latencies, 95.0),
        latency_us_total: latencies.iter().sum(),
        input_tokens,
        output_tokens,
        model_versions,
        rubric_version: super::jev::RUBRIC_VERSION.to_string(),
        truncated_inputs,
    })
}

/// Human-readable run summary for stderr. Says plainly when a hosted run answered nothing
/// hosted (every row a fallback).
pub fn describe_summary(s: &EvalSummary) -> String {
    let mut text = String::new();
    let b = &s.backend;
    text.push_str(&format!(
        "classifier_eval: {} cases | configured backend = {} | effective = {}\n",
        s.cases,
        b.configured.as_str(),
        b.effective
    ));
    if let Some(e) = &b.init_error {
        text.push_str(&format!("  backend init error: {e}\n"));
    }
    if let (Some(m), Some(ep)) = (&b.model, &b.endpoint) {
        text.push_str(&format!(
            "  hosted model requested = {m} | endpoint = {ep} | timeout = {} ms | rubric = {}\n",
            b.timeout_ms, s.rubric_version
        ));
    }
    if !s.model_versions.is_empty() {
        let v: Vec<String> = s
            .model_versions
            .iter()
            .map(|(k, n)| format!("{k}×{n}"))
            .collect();
        text.push_str(&format!(
            "  model versions that answered: {}\n",
            v.join(", ")
        ));
    }
    text.push_str(&format!(
        "  init = {} µs (excluded from latency_us) | latency p50 = {} µs | p95 = {} µs | total = {} µs\n",
        s.init_time_us, s.latency_us_p50, s.latency_us_p95, s.latency_us_total
    ));
    let st = &s.stats;
    text.push_str(&format!(
        "  primary ok = {} | abstained = {} | fallbacks = {}",
        st.primary_ok,
        st.abstained,
        st.fallback_total()
    ));
    let by: Vec<String> = st
        .fallbacks
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(r, n)| format!("{}={n}", r.as_str()))
        .collect();
    if !by.is_empty() {
        text.push_str(&format!(" ({})", by.join(", ")));
    }
    text.push('\n');
    if b.configured != crate::config::ClassifierBackend::Regex
        && st.primary_ok + st.abstained == 0
        && s.cases > 0
    {
        text.push_str(
            "  WARNING: the hosted backend answered no case; every row above is a regex fallback.\n",
        );
    }
    if s.input_tokens > 0 || s.output_tokens > 0 {
        text.push_str(&format!(
            "  billed usage: input_tokens = {} | output_tokens = {} (output is free per vendor pricing)\n",
            s.input_tokens, s.output_tokens
        ));
    }
    if s.truncated_inputs > 0 {
        text.push_str(&format!(
            "  inputs truncated to the size caps: {}\n",
            s.truncated_inputs
        ));
    }
    text
}

/// Read a predictions JSONL (an `OUT` file) back. Rows must carry the harness fields;
/// `complexity`/`confidence` may be `null` (the pre-contribution baseline wrote nulls).
pub fn read_predictions(raw: &str) -> Result<Vec<metrics::Prediction>, String> {
    let mut rows = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("line {}: missing id", n + 1))?
            .to_string();
        let request_type = v
            .get("request_type")
            .and_then(Value::as_str)
            .and_then(RequestType::from_wire)
            .ok_or_else(|| format!("line {}: missing or unknown request_type", n + 1))?;
        let complexity = v.get("complexity").and_then(Value::as_u64).map(|c| c as u8);
        let confidence = v
            .get("confidence")
            .and_then(Value::as_f64)
            .map(|c| c as f32);
        let latency_us = v.get("latency_us").and_then(Value::as_u64).unwrap_or(0);
        rows.push(metrics::Prediction {
            id,
            request_type,
            complexity,
            confidence,
            latency_us,
        });
    }
    Ok(rows)
}

/// Read a diagnostics sidecar back.
pub fn read_diagnostics(raw: &str) -> Result<Vec<DiagRow>, String> {
    raw.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(n, l)| serde_json::from_str(l).map_err(|e| format!("line {}: {e}", n + 1)))
        .collect()
}

/// A labelled case as the scorer sees it (labels *are* read here — this is the scoring
/// side, never the inference side).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LabeledCase {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub context: Option<String>,
    pub request_type: RequestType,
    pub complexity: u8,
    #[serde(default)]
    pub tests: Vec<String>,
    /// Scenario family: paraphrases and near-duplicates share one, and a family never
    /// spans splits.
    #[serde(default)]
    pub family: Option<String>,
}

pub fn read_labeled_cases(raw: &str) -> Result<Vec<LabeledCase>, String> {
    let data: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let examples = data
        .get("examples")
        .and_then(Value::as_array)
        .ok_or("no `examples` array")?;
    examples
        .iter()
        .enumerate()
        .map(|(i, e)| serde_json::from_value(e.clone()).map_err(|err| format!("case #{i}: {err}")))
        .collect()
}

pub mod metrics {
    //! Scorer arithmetic on hand-checkable inputs.
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    pub struct Prediction {
        pub id: String,
        pub request_type: RequestType,
        pub complexity: Option<u8>,
        pub confidence: Option<f32>,
        pub latency_us: u64,
    }

    /// Per-class precision/recall/F1 plus support.
    #[derive(Debug, Clone, PartialEq, Serialize)]
    pub struct ClassScore {
        pub request_type: RequestType,
        pub support: usize,
        pub predicted: usize,
        pub correct: usize,
        pub precision: f64,
        pub recall: f64,
        pub f1: f64,
    }

    /// One calibration bin: `[lo, hi)` on confidence.
    #[derive(Debug, Clone, PartialEq, Serialize)]
    pub struct CalibrationBin {
        pub lo: f32,
        pub hi: f32,
        pub count: usize,
        pub mean_confidence: f64,
        pub accuracy: f64,
    }

    /// Selective-prediction point: keep only predictions with confidence ≥ `threshold`.
    #[derive(Debug, Clone, PartialEq, Serialize)]
    pub struct SelectivePoint {
        pub threshold: f32,
        pub coverage: f64,
        pub accuracy_on_covered: f64,
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    pub struct Report {
        pub cases: usize,
        /// Cases with a prediction row.
        pub scored: usize,
        pub accuracy: f64,
        pub per_class: Vec<ClassScore>,
        pub macro_f1: f64,
        /// `confusion[true_idx][pred_idx]`, indices in [`RequestType::ALL`] order.
        pub confusion: Vec<Vec<usize>>,
        /// Expected calibration error over `ece_bins` equal-width bins on `[0,1]`; rows
        /// without a confidence are excluded and counted in `without_confidence`.
        pub ece: Option<f64>,
        pub ece_bins: usize,
        pub calibration: Vec<CalibrationBin>,
        pub without_confidence: usize,
        pub complexity_scored: usize,
        pub complexity_exact: Option<f64>,
        pub complexity_mae: Option<f64>,
        /// Within one level.
        pub complexity_within_one: Option<f64>,
        pub latency_us_p50: u64,
        pub latency_us_p95: u64,
        pub selective: Vec<SelectivePoint>,
    }

    /// Nearest-rank percentile (`p` in 0..=100) of a sample; 0 for an empty sample.
    pub fn percentile(values: &[u64], p: f64) -> u64 {
        if values.is_empty() {
            return 0;
        }
        let mut v = values.to_vec();
        v.sort_unstable();
        let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
        v[rank.min(v.len()) - 1]
    }

    fn idx(rt: RequestType) -> usize {
        RequestType::ALL.iter().position(|r| *r == rt).unwrap()
    }

    /// Score predictions against labels. Predictions whose id is not in `labels` are
    /// ignored; labels without a prediction count as unscored (never as correct).
    pub fn score(labels: &[LabeledCase], preds: &[Prediction], ece_bins: usize) -> Report {
        let ece_bins = ece_bins.max(1);
        let by_id: BTreeMap<&str, &Prediction> = preds.iter().map(|p| (p.id.as_str(), p)).collect();
        let n = RequestType::ALL.len();
        let mut confusion = vec![vec![0usize; n]; n];
        let mut scored = 0usize;
        let mut correct_total = 0usize;
        let mut conf_pairs: Vec<(f32, bool)> = Vec::new();
        let mut without_confidence = 0usize;
        let mut cx_pairs: Vec<(u8, u8)> = Vec::new();
        let mut latencies = Vec::new();
        for l in labels {
            let Some(p) = by_id.get(l.id.as_str()) else {
                continue;
            };
            scored += 1;
            confusion[idx(l.request_type)][idx(p.request_type)] += 1;
            let correct = p.request_type == l.request_type;
            if correct {
                correct_total += 1;
            }
            match p.confidence {
                Some(c) if c.is_finite() => conf_pairs.push((c.clamp(0.0, 1.0), correct)),
                _ => without_confidence += 1,
            }
            if let Some(c) = p.complexity {
                cx_pairs.push((l.complexity, c));
            }
            latencies.push(p.latency_us);
        }
        let per_class: Vec<ClassScore> = RequestType::ALL
            .iter()
            .map(|rt| {
                let i = idx(*rt);
                let support: usize = confusion[i].iter().sum();
                let predicted: usize = confusion.iter().map(|row| row[i]).sum();
                let correct = confusion[i][i];
                let precision = if predicted > 0 {
                    correct as f64 / predicted as f64
                } else {
                    0.0
                };
                let recall = if support > 0 {
                    correct as f64 / support as f64
                } else {
                    0.0
                };
                let f1 = if precision + recall > 0.0 {
                    2.0 * precision * recall / (precision + recall)
                } else {
                    0.0
                };
                ClassScore {
                    request_type: *rt,
                    support,
                    predicted,
                    correct,
                    precision,
                    recall,
                    f1,
                }
            })
            .collect();
        // Macro F1 over classes that appear in the labels or predictions (an absent class
        // would otherwise drag the mean down with an undefined 0).
        let present: Vec<&ClassScore> = per_class
            .iter()
            .filter(|c| c.support > 0 || c.predicted > 0)
            .collect();
        let macro_f1 = if present.is_empty() {
            0.0
        } else {
            present.iter().map(|c| c.f1).sum::<f64>() / present.len() as f64
        };

        // ECE: equal-width bins [k/B, (k+1)/B), the last bin closed at 1.0.
        let mut calibration = Vec::with_capacity(ece_bins);
        let mut ece = 0.0;
        for k in 0..ece_bins {
            let lo = k as f32 / ece_bins as f32;
            let hi = (k + 1) as f32 / ece_bins as f32;
            let members: Vec<&(f32, bool)> = conf_pairs
                .iter()
                .filter(|(c, _)| *c >= lo && (*c < hi || (k + 1 == ece_bins && *c <= hi)))
                .collect();
            let count = members.len();
            let (mean_confidence, accuracy) = if count > 0 {
                (
                    members.iter().map(|(c, _)| *c as f64).sum::<f64>() / count as f64,
                    members.iter().filter(|(_, ok)| *ok).count() as f64 / count as f64,
                )
            } else {
                (0.0, 0.0)
            };
            if count > 0 && !conf_pairs.is_empty() {
                ece +=
                    (count as f64 / conf_pairs.len() as f64) * (accuracy - mean_confidence).abs();
            }
            calibration.push(CalibrationBin {
                lo,
                hi,
                count,
                mean_confidence,
                accuracy,
            });
        }
        let ece = (!conf_pairs.is_empty()).then_some(ece);

        let complexity_scored = cx_pairs.len();
        let (complexity_exact, complexity_mae, complexity_within_one) = if complexity_scored > 0 {
            let exact =
                cx_pairs.iter().filter(|(a, b)| a == b).count() as f64 / complexity_scored as f64;
            let mae = cx_pairs
                .iter()
                .map(|(a, b)| (*a as f64 - *b as f64).abs())
                .sum::<f64>()
                / complexity_scored as f64;
            let within = cx_pairs
                .iter()
                .filter(|(a, b)| (*a as i32 - *b as i32).abs() <= 1)
                .count() as f64
                / complexity_scored as f64;
            (Some(exact), Some(mae), Some(within))
        } else {
            (None, None, None)
        };

        let selective = [0.0f32, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]
            .iter()
            .map(|t| {
                let covered: Vec<&(f32, bool)> =
                    conf_pairs.iter().filter(|(c, _)| *c >= *t).collect();
                SelectivePoint {
                    threshold: *t,
                    coverage: if scored > 0 {
                        covered.len() as f64 / scored as f64
                    } else {
                        0.0
                    },
                    accuracy_on_covered: if covered.is_empty() {
                        0.0
                    } else {
                        covered.iter().filter(|(_, ok)| *ok).count() as f64 / covered.len() as f64
                    },
                }
            })
            .collect();

        Report {
            cases: labels.len(),
            scored,
            accuracy: if scored > 0 {
                correct_total as f64 / scored as f64
            } else {
                0.0
            },
            per_class,
            macro_f1,
            confusion,
            ece,
            ece_bins,
            calibration,
            without_confidence,
            complexity_scored,
            complexity_exact,
            complexity_mae,
            complexity_within_one,
            latency_us_p50: percentile(&latencies, 50.0),
            latency_us_p95: percentile(&latencies, 95.0),
            selective,
        }
    }

    /// Compare two runs on the semantic fields only (`request_type`, `complexity`,
    /// `confidence` rounded to 1e-6); `latency_us` is measured and expected to differ.
    /// Returns `(compared, differing ids)`.
    pub fn semantic_diff(a: &[Prediction], b: &[Prediction]) -> (usize, Vec<String>) {
        let by_id: BTreeMap<&str, &Prediction> = b.iter().map(|p| (p.id.as_str(), p)).collect();
        let mut compared = 0;
        let mut differing = Vec::new();
        for p in a {
            let Some(q) = by_id.get(p.id.as_str()) else {
                differing.push(p.id.clone());
                continue;
            };
            compared += 1;
            let conf_eq = match (p.confidence, q.confidence) {
                (Some(x), Some(y)) => ((x - y).abs()) < 1e-6,
                (None, None) => true,
                _ => false,
            };
            if p.request_type != q.request_type || p.complexity != q.complexity || !conf_eq {
                differing.push(p.id.clone());
            }
        }
        (compared, differing)
    }
}

#[cfg(test)]
mod tests {
    use super::metrics::*;
    use super::*;
    use crate::routing::classifier::{Classification, ClassifyError};
    use crate::routing::classifier_service::test_support::{FakeClassifier, service_with};
    use std::sync::Arc;
    use std::time::Duration;

    const SET: &str = r#"{"examples":[
        {"id":"a","query":"write a python function","context":"no ctx","request_type":"code_generation","complexity":1,"tier_hypothesis":"tier_3","tests":["x"]},
        {"id":"b","query":"what is the capital of France?","context":null,"request_type":"factual_lookup","complexity":1},
        {"id":"c","query":"hello there","request_type":"general","complexity":1}
    ]}"#;

    #[test]
    fn read_eval_cases_reads_only_inference_fields_and_rejects_bad_input() {
        let cases = read_eval_cases(SET).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].context.as_deref(), Some("no ctx"));
        assert_eq!(cases[1].context, None);
        assert_eq!(cases[2].context, None);
        assert!(matches!(read_eval_cases("{}"), Err(EvalError::NoExamples)));
        assert!(matches!(
            read_eval_cases("not json"),
            Err(EvalError::Json(_))
        ));
        assert!(matches!(
            read_eval_cases(r#"{"examples":[{"query":"q"}]}"#),
            Err(EvalError::BadCase { index: 0, .. })
        ));
        assert!(matches!(
            read_eval_cases(r#"{"examples":[{"id":"x"}]}"#),
            Err(EvalError::BadCase { .. })
        ));
        assert!(matches!(
            read_eval_cases(r#"{"examples":[{"id":"x","query":"q","context":5}]}"#),
            Err(EvalError::BadCase { .. })
        ));
        assert!(matches!(
            read_eval_cases(r#"{"examples":[{"id":"x","query":"q"},{"id":"x","query":"q"}]}"#),
            Err(EvalError::DuplicateId(_))
        ));
    }

    #[tokio::test]
    async fn run_eval_writes_one_schema_row_per_case_in_order_with_real_timing() {
        let cases = read_eval_cases(SET).unwrap();
        let service = ClassifierService::regex_only();
        let mut out = Vec::new();
        let mut diag = Vec::new();
        let summary = run_eval(&service, &cases, &mut out, Some(&mut diag))
            .await
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        let rows: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter()
                .map(|r| r["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        for r in &rows {
            let keys: Vec<&String> = r.as_object().unwrap().keys().collect();
            assert_eq!(
                keys,
                [
                    "complexity",
                    "confidence",
                    "id",
                    "latency_us",
                    "request_type"
                ]
            );
            assert!(r["complexity"].is_u64());
            assert!(r["confidence"].is_f64() || r["confidence"].is_u64());
            assert!(r["latency_us"].is_u64());
        }
        assert_eq!(rows[0]["request_type"], "code_generation");
        assert_eq!(rows[1]["request_type"], "factual_lookup");
        assert_eq!(rows[2]["request_type"], "general");
        // The sidecar names the backend and disposition per row.
        let d = read_diagnostics(std::str::from_utf8(&diag).unwrap()).unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].answered_by, "regex");
        assert_eq!(d[0].disposition, Disposition::Regex);
        assert_eq!(summary.cases, 3);
        assert_eq!(summary.stats.calls, 3);
        assert!(summary.stats.fallback_total() == 0);
        // Rows read back through the scorer loader.
        let preds = read_predictions(&text).unwrap();
        assert_eq!(preds.len(), 3);
        assert_eq!(preds[0].complexity, Some(3));
    }

    #[tokio::test]
    async fn run_eval_makes_fallbacks_visible_and_honest() {
        let cases = read_eval_cases(SET).unwrap();
        let fake = Arc::new(FakeClassifier::failing(ClassifyError::Network(
            "down".into(),
        )));
        let service = service_with(fake, 0.0, Duration::from_secs(1));
        let mut out = Vec::new();
        let mut diag = Vec::new();
        let summary = run_eval(&service, &cases, &mut out, Some(&mut diag))
            .await
            .unwrap();
        assert_eq!(summary.stats.fallback_total(), 3);
        let d = read_diagnostics(std::str::from_utf8(&diag).unwrap()).unwrap();
        assert!(
            d.iter()
                .all(|r| matches!(r.disposition, Disposition::Fallback { .. }))
        );
        assert!(
            d.iter()
                .all(|r| r.answered_by == "regex" && r.configured_backend == "jev")
        );
        let text = describe_summary(&summary);
        assert!(text.contains("WARNING"), "{text}");
        assert!(text.contains("network=3"), "{text}");
    }

    #[tokio::test]
    async fn eval_never_shows_labels_to_the_backend() {
        let cases = read_eval_cases(SET).unwrap();
        let fake = Arc::new(FakeClassifier::answering(Classification {
            request_type: RequestType::Writing,
            complexity: 2,
            confidence: 0.9,
        }));
        let service = service_with(fake.clone(), 0.0, Duration::from_secs(1));
        let mut out = Vec::new();
        run_eval(&service, &cases, &mut out, None).await.unwrap();
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        for (q, c) in seen.iter() {
            for forbidden in ["code_generation", "factual_lookup", "tier_3", "complexity"] {
                assert!(!q.contains(forbidden) && !c.as_deref().unwrap_or("").contains(forbidden));
            }
        }
        assert_eq!(
            seen[0],
            ("write a python function".into(), Some("no ctx".into()))
        );
    }

    // ── scorer arithmetic on hand-checkable fixtures ───────────────────────────────

    fn label(id: &str, rt: RequestType, cx: u8) -> LabeledCase {
        LabeledCase {
            id: id.into(),
            query: String::new(),
            context: None,
            request_type: rt,
            complexity: cx,
            tests: vec![],
            family: None,
        }
    }
    fn pred(id: &str, rt: RequestType, cx: Option<u8>, conf: Option<f32>, lat: u64) -> Prediction {
        Prediction {
            id: id.into(),
            request_type: rt,
            complexity: cx,
            confidence: conf,
            latency_us: lat,
        }
    }

    #[test]
    fn score_computes_accuracy_f1_confusion_ece_complexity_and_latency_by_hand() {
        use RequestType::*;
        let labels = vec![
            label("1", CodeGeneration, 1),
            label("2", CodeGeneration, 3),
            label("3", Writing, 2),
            label("4", Writing, 5),
            label("5", General, 1), // no prediction ⇒ unscored
        ];
        let preds = vec![
            pred("1", CodeGeneration, Some(1), Some(0.9), 100),
            pred("2", Writing, Some(2), Some(0.8), 200),
            pred("3", Writing, Some(2), Some(0.7), 300),
            pred("4", CodeGeneration, None, Some(0.2), 400),
            pred("zzz", General, Some(1), Some(1.0), 999), // unknown id ⇒ ignored
        ];
        let r = score(&labels, &preds, 10);
        assert_eq!(r.cases, 5);
        assert_eq!(r.scored, 4);
        assert!((r.accuracy - 0.5).abs() < 1e-9);
        // code_generation: support 2, predicted 2, correct 1 ⇒ P=R=F1=0.5; writing same.
        let cg = r
            .per_class
            .iter()
            .find(|c| c.request_type == CodeGeneration)
            .unwrap();
        assert_eq!((cg.support, cg.predicted, cg.correct), (2, 2, 1));
        assert!((cg.f1 - 0.5).abs() < 1e-9);
        // Macro F1 over the two present classes = 0.5.
        assert!((r.macro_f1 - 0.5).abs() < 1e-9);
        let i = |rt| RequestType::ALL.iter().position(|x| *x == rt).unwrap();
        assert_eq!(r.confusion[i(CodeGeneration)][i(Writing)], 1);
        assert_eq!(r.confusion[i(Writing)][i(CodeGeneration)], 1);
        // ECE with 10 bins: bins [0.9,1.0]: conf .9 acc 1 ⇒ |0.1|; [0.8,0.9): .8 acc 0 ⇒ .8;
        // [0.7,0.8): .7 acc 1 ⇒ .3; [0.2,0.3): .2 acc 0 ⇒ .2. Each weight 1/4 ⇒ 0.35.
        assert!((r.ece.unwrap() - 0.35).abs() < 1e-6, "ece = {:?}", r.ece);
        assert_eq!(r.without_confidence, 0);
        // Complexity: pairs (1,1),(3,2),(2,2) ⇒ exact 2/3, MAE 1/3, within-one 1.0.
        assert_eq!(r.complexity_scored, 3);
        assert!((r.complexity_exact.unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert!((r.complexity_mae.unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert!((r.complexity_within_one.unwrap() - 1.0).abs() < 1e-9);
        // Latency nearest-rank: p50 of [100,200,300,400] = 200, p95 = 400.
        assert_eq!(r.latency_us_p50, 200);
        assert_eq!(r.latency_us_p95, 400);
        // Selective: at ≥0.7 coverage 3/4, accuracy on covered 2/3.
        let s = r
            .selective
            .iter()
            .find(|s| (s.threshold - 0.7).abs() < 1e-6)
            .unwrap();
        assert!((s.coverage - 0.75).abs() < 1e-9);
        assert!((s.accuracy_on_covered - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn score_handles_missing_confidence_and_empty_inputs() {
        let labels = vec![label("1", RequestType::General, 1)];
        let preds = vec![pred("1", RequestType::General, None, None, 5)];
        let r = score(&labels, &preds, 10);
        assert_eq!(r.accuracy, 1.0);
        assert_eq!(r.ece, None);
        assert_eq!(r.without_confidence, 1);
        assert_eq!(r.complexity_exact, None);
        let empty = score(&[], &[], 10);
        assert_eq!(empty.scored, 0);
        assert_eq!(empty.accuracy, 0.0);
        assert_eq!(empty.latency_us_p50, 0);
    }

    #[test]
    fn percentile_is_nearest_rank() {
        assert_eq!(percentile(&[], 50.0), 0);
        assert_eq!(percentile(&[7], 95.0), 7);
        assert_eq!(percentile(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 50.0), 5);
        assert_eq!(percentile(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 95.0), 10);
        assert_eq!(percentile(&[10, 1, 5], 0.0), 1);
    }

    #[test]
    fn semantic_diff_ignores_latency_and_flags_label_changes() {
        let a = vec![
            pred("1", RequestType::General, Some(1), Some(0.5), 10),
            pred("2", RequestType::Writing, Some(2), Some(0.6), 20),
        ];
        let b = vec![
            pred("1", RequestType::General, Some(1), Some(0.5), 9999),
            pred("2", RequestType::Writing, Some(3), Some(0.6), 20),
        ];
        let (compared, diff) = semantic_diff(&a, &b);
        assert_eq!(compared, 2);
        assert_eq!(diff, vec!["2".to_string()]);
    }

    #[test]
    fn labeled_cases_parse_with_optional_family_and_tests() {
        let cases = read_labeled_cases(SET).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].tests, vec!["x".to_string()]);
        assert_eq!(cases[0].family, None);
        assert_eq!(cases[1].request_type, RequestType::FactualLookup);
    }
}
