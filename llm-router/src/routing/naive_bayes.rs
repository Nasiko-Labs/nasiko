//! Naive Bayes request-type classifier — the opt-in local backend (`CLASSIFIER_BACKEND=nb`).
//!
//! Pure Rust, no network, no model file: it **trains at construction** from the labelled
//! JSONL embedded in the binary (`data/classifier/train.jsonl`; labelling criteria in
//! `data/classifier/LABELLING.md`). Training on ~650 rows takes a few milliseconds at
//! startup; that is the backend's whole "load time".
//!
//! ## Model
//!
//! Multinomial naive Bayes over *binarized* features (each feature counts once per
//! document), Laplace smoothing [`ALPHA`]:
//!
//! - query unigrams and bigrams (lowercased alphanumeric tokens),
//! - context unigrams and bigrams, prefixed `c:` so "explain" in a pasted log is not the
//!   same evidence as "explain" in the ask,
//! - shape markers: a code fence / inline code in query or context, and no context.
//!
//! Features never seen in training are ignored. Iteration is over the document's own sorted
//! feature list, so the floating-point sums — and therefore the output — are identical for
//! identical input.
//!
//! ## Confidence
//!
//! `confidence` is the softmax probability of the top class over the per-class log
//! posteriors divided by [`TEMPERATURE`]. Raw naive Bayes posteriors are wildly
//! overconfident (features are not independent), so the temperature was chosen by 5-fold
//! cross-validation on the *training* split to minimise log loss (see the ignored
//! `tune_temperature` test); the validation split was not used.
//!
//! ## Low confidence
//!
//! Below the configured threshold the model does not trust itself: if the regex agrees it
//! keeps the label, otherwise it returns the **regex** label (today's router behaviour, the
//! safe default) with the model's own low confidence, and counts a low-confidence fallback.
//!
//! ## Complexity
//!
//! The model predicts only the request type; complexity comes from [`rubric_complexity`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
    classify_request_type,
};

/// The labelled training split, embedded so the backend needs no deployment step.
pub const TRAIN_JSONL: &str = include_str!("../../data/classifier/train.jsonl");

/// Laplace smoothing pseudo-count.
const ALPHA: f64 = 1.0;
/// Softmax temperature applied to log posteriors (see module docs).
pub const TEMPERATURE: f64 = 1.5;

/// Class order: index `i` of every per-class array is `CLASSES[i]`. On an exact score tie
/// the earlier class wins.
const CLASSES: [RequestType; 7] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];
const K: usize = CLASSES.len();

fn class_index(rt: RequestType) -> usize {
    CLASSES
        .iter()
        .position(|c| *c == rt)
        .expect("every RequestType is in CLASSES")
}

/// One labelled row of the JSONL dataset.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LabelledExample {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub context: String,
    pub request_type: String,
    pub complexity: u8,
}

/// Parse a JSONL dataset; blank lines are skipped, any malformed row or unknown label is
/// an error (a half-loaded model must not silently ship).
pub fn parse_jsonl(jsonl: &str) -> Result<Vec<LabelledExample>, ClassifyError> {
    let mut rows = Vec::new();
    for (n, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: LabelledExample = serde_json::from_str(line)
            .map_err(|e| ClassifyError::Load(format!("line {}: {e}", n + 1)))?;
        if RequestType::from_wire(&row.request_type).is_none() {
            return Err(ClassifyError::Load(format!(
                "line {}: unknown request_type {:?}",
                n + 1,
                row.request_type
            )));
        }
        rows.push(row);
    }
    Ok(rows)
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn push_ngrams(out: &mut Vec<String>, text: &str, prefix: &str) {
    let ws = words(text);
    for w in &ws {
        out.push(format!("{prefix}{w}"));
    }
    for pair in ws.windows(2) {
        out.push(format!("{prefix}{} {}", pair[0], pair[1]));
    }
    if text.contains("```") {
        out.push(format!("{prefix}#fence"));
    } else if text.contains('`') {
        out.push(format!("{prefix}#tick"));
    }
}

/// The document's binarized feature set, sorted (deterministic summation order).
fn features(input: &ClassifyInput<'_>) -> Vec<String> {
    let mut out = Vec::new();
    push_ngrams(&mut out, input.query, "");
    match input.context.map(str::trim).filter(|c| !c.is_empty()) {
        Some(ctx) => push_ngrams(&mut out, ctx, "c:"),
        None => out.push("#noctx".to_string()),
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A trained model: log priors plus per-feature, per-class log likelihoods.
#[derive(Debug, Clone)]
struct Model {
    log_prior: [f64; K],
    log_lik: HashMap<String, [f64; K]>,
}

impl Model {
    fn fit(rows: &[LabelledExample]) -> Result<Self, ClassifyError> {
        if rows.is_empty() {
            return Err(ClassifyError::Load("empty training set".into()));
        }
        let mut docs = [0usize; K];
        let mut counts: HashMap<String, [f64; K]> = HashMap::new();
        for row in rows {
            let rt = RequestType::from_wire(&row.request_type)
                .ok_or_else(|| ClassifyError::Load(format!("unknown label in {}", row.id)))?;
            let k = class_index(rt);
            docs[k] += 1;
            let input = ClassifyInput {
                query: &row.query,
                context: Some(&row.context),
            };
            for f in features(&input) {
                counts.entry(f).or_insert([0.0; K])[k] += 1.0;
            }
        }
        let vocab = counts.len() as f64;
        let mut totals = [0.0f64; K];
        for c in counts.values() {
            for k in 0..K {
                totals[k] += c[k];
            }
        }
        let n = rows.len() as f64;
        let mut log_prior = [0.0; K];
        for k in 0..K {
            // Smoothed so a class absent from a CV fold is unlikely, not impossible.
            log_prior[k] = ((docs[k] as f64 + 1.0) / (n + K as f64)).ln();
        }
        let log_lik = counts
            .into_iter()
            .map(|(f, c)| {
                let mut l = [0.0; K];
                for k in 0..K {
                    l[k] = ((c[k] + ALPHA) / (totals[k] + ALPHA * vocab)).ln();
                }
                (f, l)
            })
            .collect();
        Ok(Self { log_prior, log_lik })
    }

    /// Per-class probabilities (softmax of tempered log posteriors), in `CLASSES` order.
    fn predict_proba(&self, input: &ClassifyInput<'_>, temperature: f64) -> [f64; K] {
        let mut score = self.log_prior;
        for f in features(input) {
            if let Some(l) = self.log_lik.get(&f) {
                for k in 0..K {
                    score[k] += l[k];
                }
            }
        }
        let max = score.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mut p = [0.0; K];
        let mut sum = 0.0;
        for k in 0..K {
            p[k] = ((score[k] - max) / temperature).exp();
            sum += p[k];
        }
        for v in &mut p {
            *v /= sum;
        }
        p
    }
}

fn argmax(p: &[f64; K]) -> usize {
    let mut best = 0;
    for k in 1..K {
        if p[k] > p[best] {
            best = k;
        }
    }
    best
}

/// The naive Bayes backend. Deterministic; never errors once constructed.
#[derive(Debug)]
pub struct NaiveBayesClassifier {
    model: Model,
    temperature: f64,
    threshold: f32,
    low_confidence_fallbacks: AtomicU64,
}

impl NaiveBayesClassifier {
    /// Train on the embedded dataset. `threshold` is the confidence below which the regex
    /// label is preferred (see module docs).
    pub fn embedded(threshold: f32) -> Result<Self, ClassifyError> {
        Self::from_jsonl(TRAIN_JSONL, threshold)
    }

    /// Train on an arbitrary JSONL dataset (same schema as the embedded one).
    pub fn from_jsonl(jsonl: &str, threshold: f32) -> Result<Self, ClassifyError> {
        Self::from_examples(&parse_jsonl(jsonl)?, threshold, TEMPERATURE)
    }

    pub fn from_examples(
        rows: &[LabelledExample],
        threshold: f32,
        temperature: f64,
    ) -> Result<Self, ClassifyError> {
        Ok(Self {
            model: Model::fit(rows)?,
            temperature,
            threshold: threshold.clamp(0.0, 1.0),
            low_confidence_fallbacks: AtomicU64::new(0),
        })
    }

    /// Number of distinct features learned.
    pub fn vocab_size(&self) -> usize {
        self.model.log_lik.len()
    }

    /// Times the regex label was used because the model's confidence was below threshold.
    pub fn low_confidence_fallbacks(&self) -> u64 {
        self.low_confidence_fallbacks.load(Ordering::Relaxed)
    }

    /// The model's raw top label and probability, before the low-confidence rule.
    pub fn predict(&self, input: &ClassifyInput<'_>) -> (RequestType, f32) {
        let p = self.model.predict_proba(input, self.temperature);
        let k = argmax(&p);
        (CLASSES[k], p[k] as f32)
    }

    /// Mean negative log-likelihood of the true labels — used to tune [`TEMPERATURE`].
    pub fn log_loss(&self, rows: &[LabelledExample]) -> f64 {
        let total: f64 = rows
            .iter()
            .map(|r| {
                let input = ClassifyInput {
                    query: &r.query,
                    context: Some(&r.context),
                };
                let p = self.model.predict_proba(&input, self.temperature);
                let k = class_index(RequestType::from_wire(&r.request_type).unwrap());
                -p[k].max(1e-12).ln()
            })
            .sum();
        total / rows.len().max(1) as f64
    }

    /// Synchronous classification (the trait method just wraps this).
    pub fn classify_sync(&self, input: &ClassifyInput<'_>) -> Classification {
        let (predicted, confidence) = self.predict(input);
        let request_type = if confidence >= self.threshold {
            predicted
        } else {
            let regex = classify_request_type(input.query);
            if regex != predicted {
                self.low_confidence_fallbacks
                    .fetch_add(1, Ordering::Relaxed);
            }
            regex
        };
        Classification {
            request_type,
            complexity: rubric_complexity(input, request_type),
            confidence,
        }
    }
}

#[async_trait]
impl RequestClassifier for NaiveBayesClassifier {
    fn name(&self) -> &str {
        "nb"
    }
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(self.classify_sync(input))
    }
}

// --------------------------------------------------------------------------
// Complexity rubric
// --------------------------------------------------------------------------

/// Words that signal rigour beyond doing the task: validation, failure handling,
/// concurrency, migration safety, or explicit uncertainty.
const RIGOUR_MARKERS: &[&str] = &[
    "test",
    "tests",
    "invariant",
    "invariants",
    "rollback",
    "roll back",
    "without downtime",
    "concurren",
    "race",
    "deadlock",
    "interleaving",
    "memory ordering",
    "failure",
    "failover",
    "prove",
    "uncertain",
    "ambiguous",
    "edge case",
    "migrat",
    "idempoten",
    "consisten",
    "serializab",
    "conflict",
    "rollout",
    "clock",
    "atomic",
    "recovery",
    "multi-region",
    "trade",
];

/// Words that signal a deliberately small ask.
const MINIMAL_MARKERS: &[&str] = &[
    "just ",
    "only",
    "typo",
    "rename",
    "one sentence",
    "one-liner",
    "short answer",
    "quick",
    "single",
    "a sentence",
    "no essay",
    "don't rewrite",
];

/// Complexity 1–5 from a fixed, documented rubric (no learning):
///
/// | points | signal |
/// |---|---|
/// | base 1 | `factual_lookup`, `general` |
/// | base 2 | every other request type |
/// | +1 | query ≥ 25 words; another +1 at ≥ 50 words |
/// | +1 | context ≥ 30 words |
/// | +1 | ≥ 3 enumerated requirements (`,` `;` ` and ` ` plus `); another +1 at ≥ 6 |
/// | +1 | any rigour marker ([`RIGOUR_MARKERS`]); another +1 at ≥ 3 distinct markers |
/// | −1 | any minimal-edit marker ([`MINIMAL_MARKERS`]) |
///
/// The sum is clamped to `1..=5`. Levels follow the eval's rubric: 1 trivial single
/// operation; 2 straightforward; 3 multi-step with limited constraints; 4 substantial
/// reasoning or design; 5 intricate cross-component reasoning and validation.
pub fn rubric_complexity(input: &ClassifyInput<'_>, request_type: RequestType) -> u8 {
    let query = input.query.to_lowercase();
    let context = input.context.unwrap_or("");
    let mut score: i32 = match request_type {
        RequestType::FactualLookup | RequestType::General => 1,
        _ => 2,
    };
    let qwords = query.split_whitespace().count();
    score += (qwords >= 25) as i32 + (qwords >= 50) as i32;
    score += (context.split_whitespace().count() >= 30) as i32;
    let reqs = query.matches(", ").count()
        + query.matches("; ").count()
        + query.matches(" and ").count()
        + query.matches(" plus ").count();
    score += (reqs >= 3) as i32 + (reqs >= 6) as i32;
    let rigour = RIGOUR_MARKERS.iter().filter(|m| query.contains(*m)).count();
    score += (rigour >= 1) as i32 + (rigour >= 3) as i32;
    score -= MINIMAL_MARKERS.iter().any(|m| query.contains(m)) as i32;
    score.clamp(1, 5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const VAL_JSONL: &str = include_str!("../../data/classifier/val.jsonl");

    fn input(r: &LabelledExample) -> ClassifyInput<'_> {
        ClassifyInput {
            query: &r.query,
            context: Some(&r.context),
        }
    }

    #[test]
    fn embedded_dataset_parses_and_covers_every_type() {
        let rows = parse_jsonl(TRAIN_JSONL).unwrap();
        assert!(rows.len() >= 200, "train split too small: {}", rows.len());
        for rt in CLASSES {
            assert!(
                rows.iter().any(|r| r.request_type == rt.as_str()),
                "no training rows for {}",
                rt.as_str()
            );
        }
        assert!(rows.iter().all(|r| (1..=5).contains(&r.complexity)));
    }

    #[test]
    fn malformed_dataset_is_a_load_error() {
        assert!(matches!(
            NaiveBayesClassifier::from_jsonl("{not json", 0.5),
            Err(ClassifyError::Load(_))
        ));
        let bad_label =
            r#"{"id":"x","query":"q","context":"","request_type":"poetry","complexity":1}"#;
        assert!(matches!(
            NaiveBayesClassifier::from_jsonl(bad_label, 0.5),
            Err(ClassifyError::Load(_))
        ));
        assert!(matches!(
            NaiveBayesClassifier::from_jsonl("", 0.5),
            Err(ClassifyError::Load(_))
        ));
    }

    /// Near-duplicates (query token Jaccard ≥ 0.6) must not appear within or across splits.
    #[test]
    fn no_near_duplicates_within_or_across_splits() {
        use std::collections::HashSet;
        let mut rows = parse_jsonl(TRAIN_JSONL).unwrap();
        rows.extend(parse_jsonl(VAL_JSONL).unwrap());
        let sets: Vec<HashSet<String>> = rows
            .iter()
            .map(|r| words(&r.query).into_iter().collect())
            .collect();
        for i in 0..rows.len() {
            for j in i + 1..rows.len() {
                let inter = sets[i].intersection(&sets[j]).count() as f64;
                let union = sets[i].union(&sets[j]).count().max(1) as f64;
                assert!(
                    inter / union < 0.6,
                    "near-duplicate: {} vs {} (jaccard {:.2})",
                    rows[i].id,
                    rows[j].id,
                    inter / union
                );
            }
        }
    }

    #[test]
    fn same_input_same_output() {
        let a = NaiveBayesClassifier::embedded(0.4).unwrap();
        let b = NaiveBayesClassifier::embedded(0.4).unwrap();
        for r in parse_jsonl(VAL_JSONL).unwrap() {
            let x = a.classify_sync(&input(&r));
            assert_eq!(x, a.classify_sync(&input(&r)), "unstable on {}", r.id);
            assert_eq!(
                x,
                b.classify_sync(&input(&r)),
                "differs across instances on {}",
                r.id
            );
            assert!((0.0..=1.0).contains(&x.confidence));
            assert!((1..=5).contains(&x.complexity));
        }
    }

    #[test]
    fn low_confidence_defers_to_the_regex_label() {
        // A threshold above any reachable probability forces the low-confidence path on
        // every query: the label must then equal the regex's, and the model's own (low)
        // confidence is still reported.
        let nb = NaiveBayesClassifier::embedded(1.01).unwrap();
        let q = "Draft a Python script that deletes empty folders.";
        let c = nb.classify_sync(&ClassifyInput {
            query: q,
            context: None,
        });
        assert_eq!(c.request_type, classify_request_type(q));
        assert!(c.confidence < 1.0);
        // The model alone says code_generation; the regex says writing ("draft").
        assert_eq!(
            nb.predict(&ClassifyInput {
                query: q,
                context: None
            })
            .0,
            RequestType::CodeGeneration
        );
        assert_eq!(nb.low_confidence_fallbacks(), 1);

        // With threshold 0 the model's own label is always used.
        let nb = NaiveBayesClassifier::embedded(0.0).unwrap();
        let c = nb.classify_sync(&ClassifyInput {
            query: q,
            context: None,
        });
        assert_eq!(c.request_type, RequestType::CodeGeneration);
        assert_eq!(nb.low_confidence_fallbacks(), 0);
    }

    #[test]
    fn rubric_orders_trivial_below_intricate() {
        let trivial = ClassifyInput {
            query: "What port does SSH use?",
            context: None,
        };
        let hard = ClassifyInput {
            query: "Design a multi-region failover plan for the ledger service, covering replication, conflict resolution, rollback, data consistency checks, and tests that simulate region loss and clock skew.",
            context: None,
        };
        assert_eq!(rubric_complexity(&trivial, RequestType::FactualLookup), 1);
        assert!(rubric_complexity(&hard, RequestType::TechnicalDesign) >= 4);
    }

    /// Held-out accuracy on `val.jsonl` versus the regex baseline. NB must not be worse.
    #[test]
    fn validation_accuracy_beats_regex() {
        let nb = NaiveBayesClassifier::embedded(DEFAULT_TEST_THRESHOLD).unwrap();
        let val = parse_jsonl(VAL_JSONL).unwrap();
        let (mut nb_ok, mut rx_ok, mut cx_exact, mut cx_within1) = (0, 0, 0, 0);
        for r in &val {
            let c = nb.classify_sync(&input(r));
            nb_ok += (c.request_type.as_str() == r.request_type) as usize;
            rx_ok += (classify_request_type(&r.query).as_str() == r.request_type) as usize;
            cx_exact += (c.complexity == r.complexity) as usize;
            cx_within1 += ((c.complexity as i32 - r.complexity as i32).abs() <= 1) as usize;
        }
        let n = val.len() as f64;
        eprintln!(
            "val n={} nb_acc={:.3} regex_acc={:.3} complexity_exact={:.3} complexity_within1={:.3}",
            val.len(),
            nb_ok as f64 / n,
            rx_ok as f64 / n,
            cx_exact as f64 / n,
            cx_within1 as f64 / n
        );
        assert!(
            nb_ok > rx_ok,
            "nb {nb_ok} vs regex {rx_ok} correct of {}",
            val.len()
        );
    }

    const DEFAULT_TEST_THRESHOLD: f32 = 0.35;

    /// 5-fold CV on the training split only; prints accuracy and log loss per temperature.
    /// Run: cargo test -p nasiko-llm-router tune_temperature -- --ignored --nocapture
    #[test]
    #[ignore]
    fn tune_temperature() {
        let rows = parse_jsonl(TRAIN_JSONL).unwrap();
        for t in [1.0, 1.25, 1.5, 1.75, 2.0, 3.0, 4.0, 8.0] {
            let (mut loss, mut ok) = (0.0, 0usize);
            for fold in 0..5 {
                let train: Vec<_> = rows
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % 5 != fold)
                    .map(|(_, r)| r.clone())
                    .collect();
                let test: Vec<_> = rows
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % 5 == fold)
                    .map(|(_, r)| r.clone())
                    .collect();
                let nb = NaiveBayesClassifier::from_examples(&train, 0.0, t).unwrap();
                loss += nb.log_loss(&test) * test.len() as f64;
                ok += test
                    .iter()
                    .filter(|r| nb.predict(&input(r)).0.as_str() == r.request_type)
                    .count();
            }
            eprintln!(
                "T={t:>4} cv_acc={:.3} cv_logloss={:.3}",
                ok as f64 / rows.len() as f64,
                loss / rows.len() as f64
            );
        }
        for th in [0.0f32, 0.2, 0.3, 0.35, 0.4, 0.5, 0.6] {
            let mut ok = 0usize;
            for fold in 0..5 {
                let train: Vec<_> = rows
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % 5 != fold)
                    .map(|(_, r)| r.clone())
                    .collect();
                let nb = NaiveBayesClassifier::from_examples(&train, th, TEMPERATURE).unwrap();
                ok += rows
                    .iter()
                    .enumerate()
                    .filter(|(i, r)| {
                        i % 5 == fold
                            && nb.classify_sync(&input(r)).request_type.as_str() == r.request_type
                    })
                    .count();
            }
            eprintln!(
                "threshold={th} cv_acc_with_regex_fallback={:.3}",
                ok as f64 / rows.len() as f64
            );
        }
        let mut exact = 0;
        let mut within = 0;
        for r in &rows {
            let rt = RequestType::from_wire(&r.request_type).unwrap();
            let c = rubric_complexity(&input(r), rt);
            exact += (c == r.complexity) as usize;
            within += ((c as i32 - r.complexity as i32).abs() <= 1) as usize;
        }
        eprintln!(
            "train rubric exact={:.3} within1={:.3}",
            exact as f64 / rows.len() as f64,
            within as f64 / rows.len() as f64
        );
    }
}
