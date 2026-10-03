//! Offline `local` request classifier: lexical TF-IDF features + a linear model, no network,
//! no native dependencies. Weights are a small JSON file (`llm-router/models/`), trained by
//! `classifier_backend/train_lexical.py`.
//!
//! The featurizer mirrors `classifier_backend/lexical.py` exactly; golden vectors embedded in the
//! model file are checked in the unit tests so the two cannot drift.
//!
//! Semantics:
//! * `request_type` — arg-max label.
//! * `confidence`   — max softmax probability after temperature scaling fitted on out-of-fold
//!   predictions (a calibrated estimate, not a guarantee).
//! * `complexity`   — ridge regression on the same features, rounded and clamped to 1–5.
//! * Deterministic: features are accumulated in sorted index order, so results are bit-identical
//!   across runs.

use std::{collections::HashMap, sync::LazyLock};

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;

use super::{
    classifier::RequestType,
    request_classifier::{Classification, RequestClassifier},
};

/// Model bundled with the crate so `REQUEST_CLASSIFIER_BACKEND=local` works with no extra files.
const BUNDLED_MODEL: &str = include_str!("../../models/request-classifier-lexical-v1.json");

const DENSE_FEATURES: usize = 6;

static CODEISH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[{};]|=>|->|::|\bdef\b|\bfn\b").unwrap());
static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d").unwrap());

#[derive(Debug, Deserialize)]
struct Golden {
    query: String,
    context: String,
    probs: Vec<f64>,
    complexity_raw: f64,
}

#[derive(Debug, Deserialize)]
struct ModelFile {
    version: u32,
    labels: Vec<String>,
    context_scale: f64,
    features: Vec<String>,
    idf: Vec<f64>,
    /// `[n_features + DENSE_FEATURES][n_labels]`
    coef: Vec<Vec<f64>>,
    intercept: Vec<f64>,
    temperature: f64,
    threshold: f64,
    cx_coef: Vec<f64>,
    cx_intercept: f64,
    #[serde(default)]
    golden: Vec<Golden>,
}

#[derive(Debug)]
pub struct LocalRequestClassifier {
    labels: Vec<RequestType>,
    context_scale: f64,
    vocab: HashMap<String, usize>,
    idf: Vec<f64>,
    coef: Vec<Vec<f64>>,
    intercept: Vec<f64>,
    temperature: f64,
    threshold: f64,
    cx_coef: Vec<f64>,
    cx_intercept: f64,
}

impl LocalRequestClassifier {
    /// The model shipped inside the binary.
    pub fn bundled() -> Result<Self, String> {
        Self::from_json(BUNDLED_MODEL)
    }

    pub fn from_path(path: &str) -> Result<Self, String> {
        let json = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
        Self::from_json(&json)
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let m: ModelFile = serde_json::from_str(json).map_err(|e| format!("model json: {e}"))?;
        if m.version != 1 {
            return Err(format!("unsupported model version {}", m.version));
        }
        let labels = m
            .labels
            .iter()
            .map(|l| RequestType::from_wire(l).ok_or_else(|| format!("unknown label {l}")))
            .collect::<Result<Vec<_>, _>>()?;
        let n = m.features.len();
        if m.idf.len() != n
            || m.coef.len() != n + DENSE_FEATURES
            || m.coef.iter().any(|r| r.len() != labels.len())
            || m.intercept.len() != labels.len()
            || m.cx_coef.len() != n + DENSE_FEATURES
        {
            return Err("model dimensions are inconsistent".into());
        }
        let all_finite = m
            .coef
            .iter()
            .flatten()
            .chain(&m.idf)
            .chain(&m.intercept)
            .chain(&m.cx_coef)
            .chain([&m.temperature, &m.cx_intercept, &m.context_scale])
            .all(|v| v.is_finite());
        if !all_finite || m.temperature <= 0.0 {
            return Err("model contains non-finite or invalid values".into());
        }
        let vocab = m
            .features
            .iter()
            .enumerate()
            .map(|(i, f)| (f.clone(), i))
            .collect();
        let this = Self {
            labels,
            context_scale: m.context_scale,
            vocab,
            idf: m.idf,
            coef: m.coef,
            intercept: m.intercept,
            temperature: m.temperature,
            threshold: m.threshold,
            cx_coef: m.cx_coef,
            cx_intercept: m.cx_intercept,
        };
        for g in &m.golden {
            let (probs, cx) = this.score(&g.query, &g.context);
            let max_diff = probs
                .iter()
                .zip(&g.probs)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max);
            if max_diff > 1e-4 || (cx - g.complexity_raw).abs() > 1e-3 {
                return Err(format!(
                    "golden check failed for {:?}: prob diff {max_diff}, complexity {cx} vs {}",
                    g.query, g.complexity_raw
                ));
            }
        }
        Ok(this)
    }

    /// Confidence below which the training run recommends falling back (informational).
    pub fn recommended_min_confidence(&self) -> f64 {
        self.threshold
    }

    /// `(probabilities in label order, raw complexity regression value)`.
    fn score(&self, query: &str, context: &str) -> (Vec<f64>, f64) {
        let mut counts: HashMap<String, u32> = HashMap::new();
        segment_features(query, 'q', &mut counts);
        segment_features(context, 'x', &mut counts);

        // Sorted by index so float accumulation order is fixed (bit-identical across runs).
        let mut weighted: Vec<(usize, f64)> = counts
            .iter()
            .filter_map(|(f, &tf)| {
                let i = *self.vocab.get(f)?;
                let mut v = (1.0 + f64::from(tf).ln()) * self.idf[i];
                if f.starts_with('x') {
                    v *= self.context_scale;
                }
                Some((i, v))
            })
            .collect();
        weighted.sort_unstable_by_key(|&(i, _)| i);
        let norm = weighted.iter().map(|(_, v)| v * v).sum::<f64>().sqrt();
        if norm > 0.0 {
            for (_, v) in &mut weighted {
                *v /= norm;
            }
        }

        let n = self.vocab.len();
        let mut logits = self.intercept.clone();
        let mut cx = self.cx_intercept;
        let mut add = |idx: usize, value: f64| {
            for (l, w) in logits.iter_mut().zip(&self.coef[idx]) {
                *l += value * w;
            }
            cx += value * self.cx_coef[idx];
        };
        for &(i, v) in &weighted {
            add(i, v);
        }
        for (j, v) in dense_features(query, context).into_iter().enumerate() {
            add(n + j, v);
        }

        let scaled: Vec<f64> = logits.iter().map(|l| l / self.temperature).collect();
        let max = scaled.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = scaled.iter().map(|l| (l - max).exp()).collect();
        let sum: f64 = exps.iter().sum();
        (exps.into_iter().map(|e| e / sum).collect(), cx)
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

fn segment_features(text: &str, prefix: char, out: &mut HashMap<String, u32>) {
    let toks = tokens(text);
    for t in &toks {
        *out.entry(format!("{prefix}w:{t}")).or_default() += 1;
    }
    for pair in toks.windows(2) {
        *out.entry(format!("{prefix}b:{} {}", pair[0], pair[1]))
            .or_default() += 1;
    }
    for t in &toks {
        if t.chars().count() < 2 {
            continue;
        }
        let padded: Vec<char> = format!(" {t} ").chars().collect();
        for n in 3..=5 {
            for w in padded.windows(n) {
                let gram: String = w.iter().collect();
                *out.entry(format!("{prefix}c:{gram}")).or_default() += 1;
            }
        }
    }
}

fn dense_features(query: &str, context: &str) -> [f64; DENSE_FEATURES] {
    let qw = query.split_whitespace().count() as f64;
    let cw = context.split_whitespace().count() as f64;
    let both = format!("{context}{query}");
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    [
        qw.ln_1p() / 5.0,
        cw.ln_1p() / 6.0,
        flag(context.contains("```") || query.contains("```")),
        flag(CODEISH.is_match(&both)),
        flag(query.trim().ends_with('?')),
        flag(DIGIT.is_match(query)),
    ]
}

#[async_trait]
impl RequestClassifier for LocalRequestClassifier {
    fn name(&self) -> &str {
        "local-lexical-v1"
    }

    async fn classify(&self, query: &str, context: Option<&str>) -> Result<Classification, String> {
        let (probs, cx) = self.score(query, context.unwrap_or(""));
        let (best, conf) =
            probs
                .iter()
                .enumerate()
                .fold((0usize, f64::NEG_INFINITY), |acc, (i, &p)| {
                    if p > acc.1 { (i, p) } else { acc }
                });
        Ok(Classification {
            request_type: self.labels[best],
            complexity: cx.round().clamp(1.0, 5.0) as u8,
            confidence: conf,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_model_loads_and_matches_python_golden_vectors() {
        // `from_json` re-scores every golden vector and errors on drift from the Python featurizer.
        let model = LocalRequestClassifier::bundled().expect("bundled model must load");
        assert!(model.recommended_min_confidence() > 0.0);
    }

    #[tokio::test]
    async fn outputs_are_valid_and_bit_identical_across_calls() {
        let model = LocalRequestClassifier::bundled().unwrap();
        for query in [
            "What is the capital of France?",
            "Write a rust function that parses CSV",
            "",
            "日本語のテスト ünïcode",
        ] {
            let a = model.classify(query, Some("ctx")).await.unwrap();
            let b = model.classify(query, Some("ctx")).await.unwrap();
            assert_eq!(a, b);
            assert!((1..=5).contains(&a.complexity));
            assert!((0.0..=1.0).contains(&a.confidence));
        }
    }

    #[tokio::test]
    async fn classifies_obvious_requests() {
        let model = LocalRequestClassifier::bundled().unwrap();
        let out = model
            .classify("What port does PostgreSQL use by default?", None)
            .await
            .unwrap();
        assert_eq!(out.request_type, RequestType::FactualLookup);
    }

    #[test]
    fn corrupt_models_are_rejected() {
        assert!(LocalRequestClassifier::from_json("{}").is_err());
        assert!(LocalRequestClassifier::from_path("/nonexistent/model.json").is_err());
        let truncated = &BUNDLED_MODEL[..BUNDLED_MODEL.len() / 2];
        assert!(LocalRequestClassifier::from_json(truncated).is_err());
    }
}
