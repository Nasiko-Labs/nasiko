//! Local request-type classifier — a multinomial Naive Bayes over word unigrams/bigrams, 5-char stems, shape
//! tokens and the regex category votes, trained **once at load** from labelled JSONL.
//!
//! * **No network, no model file, no new dependency.** The default training set
//!   (`data/classifier/train.jsonl`) is embedded in the binary; `CLASSIFIER_MODEL_PATH` may point
//!   at another JSONL (`{"request_type": ..., "query": ...}` per line) to train on instead.
//! * **Deterministic.** Counting is order-independent and scoring iterates tokens in input order,
//!   so identical `(query, context)` always yields identical output.
//! * **Confidence** is the posterior of the winning class after length-normalised, temperature
//!   scaled logits (`TEMPERATURE`, chosen by cross-validation on the training split only).
//! * **Regex votes as features.** `rx_<type>` tokens carry the existing keyword classifier's
//!   evidence, so the model starts from the baseline and corrects it rather than replacing it.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::Deserialize;

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
    estimate_complexity,
};
use super::patterns::CATEGORY_PATTERNS;

const TRAIN_JSONL: &str = include_str!("../../data/classifier/train.jsonl");
const CLASSES: [RequestType; 7] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];
/// Laplace smoothing (1.0: the training set is small, so lean on the prior).
const ALPHA: f64 = 1.0;
/// Logit scaling: `logit / (sqrt(n_tokens) * TEMPERATURE)`. Raw NB posteriors are saturated for
/// long inputs and wrong in the other direction after length normalisation; 0.25 was chosen by
/// 5-fold cross-validation on the training split only (ECE ~0.08; the validation split was
/// never used for tuning). See `calibration_cv_on_train_only`.
const TEMPERATURE: f64 = 0.25;
/// Logit scaling: `logit / (sqrt(n_tokens) * TEMPERATURE)`. Raw NB posteriors are saturated
/// (~1.0) for long inputs; this brings confidence back to a usable, calibrated range.

/// Context tokens count for less than query tokens.
const CONTEXT_WEIGHT: f64 = 0.3;

#[derive(Deserialize)]
struct Row {
    request_type: String,
    query: String,
}

/// Trained model: per-class token log-likelihoods plus log priors.
pub struct LocalClassifier {
    /// token -> per-class log P(token | class)
    loglik: HashMap<String, [f64; 7]>,
    /// log P(token | class) for a token never seen in training (smoothed).
    unseen: [f64; 7],
    log_prior: [f64; 7],
    examples: usize,
}

impl LocalClassifier {
    /// Train on the embedded default set.
    pub fn embedded() -> Self {
        Self::from_jsonl(TRAIN_JSONL).expect("embedded classifier training set is valid")
    }

    /// Train on a JSONL string. Errors on a malformed line, an unknown label or an empty set.
    pub fn from_jsonl(jsonl: &str) -> Result<Self, String> {
        let mut counts: HashMap<String, [f64; 7]> = HashMap::new();
        let mut class_tokens = [0f64; 7];
        let mut class_docs = [0f64; 7];
        let mut examples = 0usize;
        for (i, line) in jsonl
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let row: Row =
                serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
            let rt = RequestType::from_wire(&row.request_type).ok_or_else(|| {
                format!("line {}: unknown request_type {}", i + 1, row.request_type)
            })?;
            let c = CLASSES.iter().position(|x| *x == rt).expect("class listed");
            class_docs[c] += 1.0;
            examples += 1;
            for (tok, _) in features(&row.query, None) {
                counts.entry(tok).or_insert([0.0; 7])[c] += 1.0;
                class_tokens[c] += 1.0;
            }
        }
        if examples == 0 {
            return Err("empty training set".into());
        }
        let vocab = counts.len() as f64;
        let denom: Vec<f64> = class_tokens
            .iter()
            .map(|t| t + ALPHA * (vocab + 1.0))
            .collect();
        let mut unseen = [0.0; 7];
        for c in 0..7 {
            unseen[c] = (ALPHA / denom[c]).ln();
        }
        let loglik = counts
            .into_iter()
            .map(|(t, cs)| {
                let mut ll = [0.0; 7];
                for c in 0..7 {
                    ll[c] = ((cs[c] + ALPHA) / denom[c]).ln();
                }
                (t, ll)
            })
            .collect();
        let total: f64 = class_docs.iter().sum();
        let mut log_prior = [0.0; 7];
        for c in 0..7 {
            log_prior[c] = ((class_docs[c] + 1.0) / (total + 7.0)).ln();
        }
        Ok(Self {
            loglik,
            unseen,
            log_prior,
            examples,
        })
    }

    pub fn examples(&self) -> usize {
        self.examples
    }

    /// Posterior over [`CLASSES`] (sums to 1).
    fn posterior(&self, query: &str, context: Option<&str>) -> [f64; 7] {
        let feats = features(query, context);
        let mut logit = self.log_prior;
        let mut n = 0.0;
        for (tok, w) in &feats {
            let ll = self.loglik.get(tok).unwrap_or(&self.unseen);
            for c in 0..7 {
                logit[c] += w * ll[c];
            }
            n += w;
        }
        let scale = 1.0 / (n.max(1.0).sqrt() * TEMPERATURE);
        let max = logit.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mut p = [0.0; 7];
        let mut z = 0.0;
        for c in 0..7 {
            p[c] = ((logit[c] - max) * scale).exp();
            z += p[c];
        }
        for v in &mut p {
            *v /= z;
        }
        p
    }

    /// Synchronous classify, shared by the trait impl and tests.
    pub fn classify_sync(&self, query: &str, context: Option<&str>) -> Classification {
        let p = self.posterior(query, context);
        // First max wins on a tie -> deterministic.
        let mut best = 0;
        for c in 1..7 {
            if p[c] > p[best] {
                best = c;
            }
        }
        Classification {
            request_type: CLASSES[best],
            complexity: estimate_complexity(query, context),
            confidence: p[best] as f32,
        }
    }
}

#[async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local"
    }
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(self.classify_sync(input.query, input.context))
    }
}

/// Weighted feature tokens: query unigrams + bigrams (weight 1), context unigrams (weight
/// [`CONTEXT_WEIGHT`], prefixed `c:`), shape tokens and regex category votes.
fn features(query: &str, context: Option<&str>) -> Vec<(String, f64)> {
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mut out: Vec<(String, f64)> = Vec::with_capacity(words.len() * 2 + 8);
    for w in &words {
        out.push(((*w).to_string(), 1.0));
        // Stem feature (5-char prefix) so paraphrases ("summarize"/"summary") share evidence.
        if w.chars().count() > 5 {
            out.push((format!("~{}", w.chars().take(5).collect::<String>()), 1.0));
        }
    }
    for pair in words.windows(2) {
        out.push((format!("{}_{}", pair[0], pair[1]), 1.0));
    }
    if let Some(ctx) = context {
        for w in ctx
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .take(200)
        {
            out.push((format!("c:{w}"), CONTEXT_WEIGHT));
        }
    }
    let punct = query.chars().filter(|c| "{}();=<>[]`".contains(*c)).count();
    if punct >= 2 || context.is_some_and(|c| c.contains("```")) {
        out.push(("shape_code".into(), 1.0));
    }
    if query.contains('?') {
        out.push(("shape_question".into(), 1.0));
    }
    out.push((
        match words.len() {
            0..=4 => "len_xs",
            5..=12 => "len_s",
            13..=30 => "len_m",
            _ => "len_l",
        }
        .into(),
        1.0,
    ));
    for (rt, pats) in CATEGORY_PATTERNS.iter() {
        let hits = pats.iter().filter(|p| p.is_match(query)).count();
        if hits > 0 {
            out.push((format!("rx_{}", rt.as_str()), hits as f64));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::classifier::classify_request_type;

    const VAL_JSONL: &str = include_str!("../../data/classifier/val.jsonl");

    #[derive(Deserialize)]
    struct ValRow {
        request_type: String,
        query: String,
    }

    fn val() -> Vec<(RequestType, String)> {
        VAL_JSONL
            .lines()
            .map(|l| {
                let r: ValRow = serde_json::from_str(l).unwrap();
                (RequestType::from_wire(&r.request_type).unwrap(), r.query)
            })
            .collect()
    }

    #[test]
    fn embedded_model_loads_and_is_deterministic() {
        let m = LocalClassifier::embedded();
        assert!(m.examples() > 100);
        let a = m.classify_sync("Write a Rust function that parses CSV", None);
        let b = LocalClassifier::embedded()
            .classify_sync("Write a Rust function that parses CSV", None);
        assert_eq!(a, b);
        assert!((0.0..=1.0).contains(&a.confidence));
        assert!((1..=5).contains(&a.complexity));
    }

    /// How `TEMPERATURE` was chosen: 5-fold cross-validation on the *training* split only (the
    /// validation split is never used for tuning). Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "calibration utility, not a regression test"]
    fn calibration_cv_on_train_only() {
        let lines: Vec<&str> = TRAIN_JSONL.lines().collect();
        let (mut n, mut ok, mut bins) = (0usize, 0usize, [(0usize, 0f64, 0usize); 10]);
        for fold in 0..5 {
            let train: String = lines
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 5 != fold)
                .map(|(_, l)| format!("{l}\n"))
                .collect();
            let m = LocalClassifier::from_jsonl(&train).unwrap();
            for l in lines
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 5 == fold)
                .map(|(_, l)| l)
            {
                let r: ValRow = serde_json::from_str(l).unwrap();
                let c = m.classify_sync(&r.query, None);
                let good = c.request_type.as_str() == r.request_type;
                let b = ((c.confidence * 10.0) as usize).min(9);
                bins[b].0 += 1;
                bins[b].1 += c.confidence as f64;
                bins[b].2 += good as usize;
                n += 1;
                ok += good as usize;
            }
        }
        let ece: f64 = bins
            .iter()
            .filter(|b| b.0 > 0)
            .map(|b| {
                (b.0 as f64 / n as f64) * ((b.1 / b.0 as f64) - (b.2 as f64 / b.0 as f64)).abs()
            })
            .sum();
        eprintln!("cv n={n} acc={:.3} ece={ece:.3}", ok as f64 / n as f64);
    }

    #[test]
    fn rejects_bad_training_data() {
        assert!(LocalClassifier::from_jsonl("").is_err());
        assert!(LocalClassifier::from_jsonl("not json").is_err());
        assert!(LocalClassifier::from_jsonl(r#"{"request_type":"nope","query":"x"}"#).is_err());
    }

    /// Held-out check: the local model must not be worse than the regex baseline on the
    /// validation split it never trained on. (Measured numbers live in the docs.)
    #[test]
    fn local_beats_or_matches_regex_on_heldout_validation() {
        let m = LocalClassifier::embedded();
        let rows = val();
        let local = rows
            .iter()
            .filter(|(t, q)| m.classify_sync(q, None).request_type == *t)
            .count();
        let regex = rows
            .iter()
            .filter(|(t, q)| classify_request_type(q) == *t)
            .count();
        eprintln!("heldout n={} local={local} regex={regex}", rows.len());
        assert!(local >= regex, "local {local} < regex {regex}");
    }
}
