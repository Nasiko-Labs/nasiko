//! Local request-classifier backend: hashed n-gram multinomial logistic regression with two
//! heads — request type (7 classes, temperature-calibrated) and complexity (5 levels).
//!
//! It runs in-process on the CPU in microseconds, needs no network and no runtime file (the
//! trained weights are embedded in the binary, like the salience model's), and is fully
//! deterministic: features accumulate in a fixed bucket order, so identical inputs give
//! bit-identical outputs.
//!
//! **The feature engine is the single source of truth for training.** The
//! `classifier_features` example runs [`features`] over a dataset and the offline trainer
//! (`classifier/scripts/train_linear_gpu.ipynb`) fits on its output, so training and inference can
//! never drift apart. It reuses the salience model's tokenizer and FNV-1a hashing; any change
//! to [`features`] (or to those helpers) invalidates the weights — retrain alongside it.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::Deserialize;

use super::{
    Classification, ClassifyError, ClassifyInput, MAX_COMPLEXITY, MIN_COMPLEXITY,
    RequestClassifier, RequestType,
};
use crate::routing::salience_classifier::{char_ngrams, fnv1a, word_ngrams, word_tokens};

/// Hashed feature buckets. Must match the weights file's `num_buckets`.
pub const NUM_BUCKETS: usize = 1 << 18;
/// Dense features, in the order [`dense_features`] produces them.
pub const NUM_DENSE_FEATURES: usize = 8;
/// Number of [`RequestType`] variants the type head must cover.
const REQUEST_TYPE_COUNT: usize = 7;
/// Weights-file schema this build reads.
const SCHEMA: &str = "nasiko-request-classifier-linear-v1";
/// Context beyond this many characters is ignored: the label is decided by what the context
/// *is* (code, logs, prose), which its opening shows, and the cap bounds per-call latency.
const MAX_CONTEXT_CHARS: usize = 1500;
/// Characters that mark code; their density separates pasted code from prose.
const CODE_SYMBOLS: &[char] = &['{', '}', '(', ')', '[', ']', ';', '=', '<', '>', '`'];

/// The weights compiled into this binary (see `classifier/README.md` to retrain).
const EMBEDDED_WEIGHTS_JSON: &str = include_str!("../../../assets/request_classifier_weights.json");

/// Sparse hashed features (bucket → signed, length-normalised value) plus dense features.
/// A `BTreeMap` keeps accumulation order fixed, which keeps scores bit-identical across runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Features {
    /// Hashed n-gram buckets → summed signed values.
    pub hashed: BTreeMap<usize, f64>,
    /// Dense features in [`dense_features`] order.
    pub dense: [f64; NUM_DENSE_FEATURES],
}

/// Extract features from a request. Query and context grams are hashed under different
/// prefixes, so "explain" in the query and "explain" in pasted context are separate signals.
pub fn features(input: &ClassifyInput<'_>) -> Features {
    let context: String = input
        .context
        .unwrap_or_default()
        .chars()
        .take(MAX_CONTEXT_CHARS)
        .collect();
    let mut hashed = BTreeMap::new();
    add_text_block(&mut hashed, "q", input.query);
    add_text_block(&mut hashed, "c", &context);
    Features {
        hashed,
        dense: dense_features(input.query, &context),
    }
}

/// Hash word 1–2-grams and char 3–5-grams of `text` under `tag`, normalised by the square
/// root of the gram count so long texts don't dominate the logits by size alone.
fn add_text_block(hashed: &mut BTreeMap<usize, f64>, tag: &str, text: &str) {
    let words = word_ngrams(&word_tokens(text), 2)
        .into_iter()
        .map(|g| format!("{tag}w {g}"));
    let chars = char_ngrams(text, 3, 5)
        .into_iter()
        .map(|g| format!("{tag}c {g}"));
    let grams: Vec<String> = words.chain(chars).collect();
    let norm = (grams.len() as f64).sqrt().max(1.0);
    for gram in &grams {
        let (bucket, sign) = hash_gram(gram);
        *hashed.entry(bucket).or_insert(0.0) += sign / norm;
    }
}

/// Bucket and sign for a gram — the salience model's hashing-trick construction at this
/// model's bucket count.
fn hash_gram(gram: &str) -> (usize, f64) {
    let bucket = (fnv1a(gram.as_bytes()) % NUM_BUCKETS as u64) as usize;
    let sign = if fnv1a(format!("sign:{gram}").as_bytes()) & 1 == 0 {
        1.0
    } else {
        -1.0
    };
    (bucket, sign)
}

/// Fraction of `text`'s chars satisfying `pred`; 0 for empty text.
fn char_ratio(text: &str, pred: impl Fn(char) -> bool) -> f64 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    text.chars().filter(|&c| pred(c)).count() as f64 / total as f64
}

/// Fixed-order dense features: size, presence of context, code density, question form,
/// script, and digits.
fn dense_features(query: &str, context: &str) -> [f64; NUM_DENSE_FEATURES] {
    let is_code_symbol = |c: char| CODE_SYMBOLS.contains(&c);
    [
        (word_tokens(query).len() as f64).ln_1p(),
        (context.chars().count() as f64).ln_1p(),
        if context.trim().is_empty() { 0.0 } else { 1.0 },
        char_ratio(query, is_code_symbol),
        char_ratio(context, is_code_symbol),
        if query.trim_end().ends_with('?') {
            1.0
        } else {
            0.0
        },
        char_ratio(query, |c| !c.is_ascii()),
        char_ratio(query, |c| c.is_ascii_digit()),
    ]
}

/// One linear head: `logits[k] = bias[k] + Σ hashed·W_h[:,k] + Σ dense·W_d[:,k]`.
#[derive(Debug)]
struct Head {
    classes: usize,
    bias: Vec<f32>,
    /// Row-major `NUM_DENSE_FEATURES × classes`.
    dense: Vec<f32>,
    /// Row-major `NUM_BUCKETS × classes`; buckets absent from the file are zero.
    hashed: Vec<f32>,
}

impl Head {
    fn logits(&self, features: &Features) -> Vec<f64> {
        let k = self.classes;
        let mut logits: Vec<f64> = self.bias.iter().map(|&b| f64::from(b)).collect();
        for (&bucket, &value) in &features.hashed {
            let row = &self.hashed[bucket * k..(bucket + 1) * k];
            for (logit, &w) in logits.iter_mut().zip(row) {
                *logit += value * f64::from(w);
            }
        }
        for (f, &value) in features.dense.iter().enumerate() {
            let row = &self.dense[f * k..(f + 1) * k];
            for (logit, &w) in logits.iter_mut().zip(row) {
                *logit += value * f64::from(w);
            }
        }
        logits
    }
}

/// Index of the largest value; ties go to the lowest index, so the choice is deterministic.
fn argmax(values: &[f64]) -> usize {
    values
        .iter()
        .enumerate()
        .fold(0, |best, (i, &v)| if v > values[best] { i } else { best })
}

/// Softmax probability of `index` after dividing logits by `temperature`.
fn softmax_at(logits: &[f64], temperature: f64, index: usize) -> f64 {
    let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = logits
        .iter()
        .map(|l| ((l - max) / temperature).exp())
        .collect();
    exps[index] / exps.iter().sum::<f64>()
}

/// Weights-file layout (written by `classifier/scripts/train_linear_gpu.ipynb`).
#[derive(Deserialize)]
struct WeightsFile {
    schema: String,
    num_buckets: usize,
    num_dense_features: usize,
    request_types: Vec<String>,
    temperature: f64,
    type_head: HeadFile,
    complexity_head: HeadFile,
    #[serde(default)]
    provenance: serde_json::Value,
}

#[derive(Deserialize)]
struct HeadFile {
    bias: Vec<f32>,
    dense: Vec<Vec<f32>>,
    /// Sparse `{bucket: [one weight per class]}`.
    hashed: BTreeMap<String, Vec<f32>>,
}

fn load_err(msg: impl Into<String>) -> ClassifyError {
    ClassifyError::ModelLoad(msg.into())
}

impl HeadFile {
    fn into_head(self, classes: usize) -> Result<Head, ClassifyError> {
        if self.bias.len() != classes || self.dense.len() != NUM_DENSE_FEATURES {
            return Err(load_err("head shape does not match its class count"));
        }
        if self.dense.iter().any(|row| row.len() != classes) {
            return Err(load_err("dense weight row has the wrong class count"));
        }
        let mut hashed = vec![0.0_f32; NUM_BUCKETS * classes];
        for (key, row) in self.hashed {
            let bucket: usize = key
                .parse()
                .map_err(|_| load_err(format!("bucket key {key:?} is not an index")))?;
            if bucket >= NUM_BUCKETS || row.len() != classes {
                return Err(load_err(format!(
                    "bucket {bucket} is out of range or malformed"
                )));
            }
            hashed[bucket * classes..(bucket + 1) * classes].copy_from_slice(&row);
        }
        Ok(Head {
            classes,
            bias: self.bias,
            dense: self.dense.concat(),
            hashed,
        })
    }
}

/// The local linear backend.
#[derive(Debug)]
pub struct LinearClassifier {
    request_types: Vec<RequestType>,
    temperature: f64,
    type_head: Head,
    complexity_head: Head,
    provenance: String,
}

impl LinearClassifier {
    /// Backend name reported in logs and eval output.
    pub const NAME: &'static str = "local";

    /// The model embedded in this binary.
    pub fn embedded() -> Result<Self, ClassifyError> {
        Self::from_json(EMBEDDED_WEIGHTS_JSON)
    }

    /// A model from a weights file — for trialling a retrained model without a rebuild.
    pub fn from_path(path: &str) -> Result<Self, ClassifyError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| load_err(format!("cannot read {path}: {e}")))?;
        Self::from_json(&raw)
    }

    fn from_json(raw: &str) -> Result<Self, ClassifyError> {
        let file: WeightsFile =
            serde_json::from_str(raw).map_err(|e| load_err(format!("invalid weights: {e}")))?;
        if file.schema != SCHEMA
            || file.num_buckets != NUM_BUCKETS
            || file.num_dense_features != NUM_DENSE_FEATURES
        {
            return Err(load_err(format!(
                "weights are {} with {} buckets / {} dense features; this build reads {SCHEMA} \
                 with {NUM_BUCKETS} / {NUM_DENSE_FEATURES}",
                file.schema, file.num_buckets, file.num_dense_features
            )));
        }
        let request_types = file
            .request_types
            .iter()
            .map(|label| {
                RequestType::from_wire(label)
                    .ok_or_else(|| load_err(format!("unknown label {label}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Every request type exactly once: an empty or duplicated label list would load and
        // then fail (or panic) on every request instead of failing here, at load.
        let distinct: std::collections::HashSet<_> = request_types.iter().collect();
        if request_types.len() != REQUEST_TYPE_COUNT || distinct.len() != REQUEST_TYPE_COUNT {
            return Err(load_err(format!(
                "weights must list each of the {REQUEST_TYPE_COUNT} request types exactly once"
            )));
        }
        if file.temperature.is_nan() || file.temperature <= 0.0 {
            return Err(load_err("temperature must be positive"));
        }
        let levels = usize::from(MAX_COMPLEXITY - MIN_COMPLEXITY + 1);
        Ok(Self {
            type_head: file.type_head.into_head(request_types.len())?,
            complexity_head: file.complexity_head.into_head(levels)?,
            request_types,
            temperature: file.temperature,
            provenance: file.provenance.to_string(),
        })
    }

    /// Training provenance recorded in the weights file, for startup logs.
    pub fn provenance(&self) -> &str {
        &self.provenance
    }

    fn classify_now(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let features = features(input);
        let type_logits = self.type_head.logits(&features);
        let best = argmax(&type_logits);
        let confidence = softmax_at(&type_logits, self.temperature, best);
        let level = argmax(&self.complexity_head.logits(&features));
        let complexity = MIN_COMPLEXITY
            + u8::try_from(level)
                .map_err(|_| ClassifyError::Inference("complexity index".into()))?;
        Classification::new(self.request_types[best], complexity, confidence as f32)
    }
}

#[async_trait]
impl RequestClassifier for LinearClassifier {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        self.classify_now(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(model: &LinearClassifier, query: &str, context: Option<&str>) -> Classification {
        model
            .classify_now(&ClassifyInput { query, context })
            .expect("embedded model classifies")
    }

    #[test]
    fn embedded_model_loads() {
        let model = LinearClassifier::embedded().expect("embedded weights parse");
        assert_eq!(model.request_types.len(), 7);
        assert!(model.provenance().contains("trained_at_utc"));
    }

    #[test]
    fn embedded_model_gets_clear_cases_right() {
        // Regex near-misses: keywords point one way, the deliverable another.
        let model = LinearClassifier::embedded().expect("embedded weights parse");
        let cases = [
            (
                "write a short poem about the Python language",
                RequestType::Writing,
            ),
            (
                "implement a function that reverses a linked list in Rust",
                RequestType::CodeGeneration,
            ),
            ("hey, thanks a lot, that helped!", RequestType::General),
            (
                "what's the default port for PostgreSQL?",
                RequestType::FactualLookup,
            ),
        ];
        for (query, expected) in cases {
            assert_eq!(
                classify(&model, query, None).request_type,
                expected,
                "{query}"
            );
        }
    }

    #[test]
    fn identical_inputs_give_bit_identical_outputs() {
        let model = LinearClassifier::embedded().expect("embedded weights parse");
        let context = Some("fn next(n: &mut u64) -> u64 { let old = *n; *n += 1; old }");
        let first = classify(&model, "why does this return the old value?", context);
        for _ in 0..10 {
            let again = classify(&model, "why does this return the old value?", context);
            assert_eq!(again, first);
            assert_eq!(again.confidence.to_bits(), first.confidence.to_bits());
        }
    }

    #[test]
    fn context_contributes_features() {
        let without = features(&ClassifyInput {
            query: "explain this",
            context: None,
        });
        let with = features(&ClassifyInput {
            query: "explain this",
            context: Some("SELECT * FROM orders WHERE id = 1;"),
        });
        assert!(with.hashed.len() > without.hashed.len());
        assert_eq!(without.dense[2], 0.0);
        assert_eq!(with.dense[2], 1.0);
    }

    #[test]
    fn malformed_weights_are_load_errors() {
        for raw in [
            "not json",
            r#"{"schema":"other","num_buckets":1,"num_dense_features":8,"request_types":[],"temperature":1,"type_head":{"bias":[],"dense":[],"hashed":{}},"complexity_head":{"bias":[],"dense":[],"hashed":{}}}"#,
        ] {
            assert!(matches!(
                LinearClassifier::from_json(raw),
                Err(ClassifyError::ModelLoad(_))
            ));
        }
        // A well-formed file whose label list is empty must be rejected at load, not panic later.
        let mut empty: serde_json::Value =
            serde_json::from_str(EMBEDDED_WEIGHTS_JSON).expect("embedded weights are JSON");
        empty["request_types"] = serde_json::json!([]);
        assert!(matches!(
            LinearClassifier::from_json(&empty.to_string()),
            Err(ClassifyError::ModelLoad(_))
        ));
        assert!(matches!(
            LinearClassifier::from_path("/nonexistent/weights.json"),
            Err(ClassifyError::ModelLoad(_))
        ));
    }
}
