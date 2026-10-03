//! The in-process request-classifier model: a compact multinomial-logistic-regression
//! classifier over hashed n-grams plus a handful of dense features.
//!
//! It predicts the two things the cost-aware router needs about a query — its
//! [`RequestType`] and its `complexity` (1–5) — and a calibrated `confidence` for the
//! request-type decision. Everything here is pure CPU and deterministic: the features are
//! hashed with the shared [`super::text_features`] primitives, the weights are `f64`, and
//! the argmax is a total order, so the same query always yields the same output.
//!
//! ## Why this shape
//!
//! A hashed n-gram linear model is the smallest thing that (a) generalises past the regex
//! keyword lists — it keys on surface form, not hand-written phrases — and (b) needs no
//! network, no tokenizer, and no runtime download: the parameter vector is small enough to
//! embed in the binary. [`super::salience_classifier`] uses the same family for the
//! small-talk gate; this is the routing counterpart, with a multi-class head instead of a
//! single sigmoid.
//!
//! ## Two heads, one feature vector
//!
//! One shared feature vector feeds two independent softmax heads:
//! - a 7-way **request-type** head (the classes are the router's [`RequestType`]s), whose
//!   max calibrated probability is reported as `confidence`;
//! - a 5-way **complexity** head, whose predicted *distribution* is turned into a single
//!   1–5 value by taking the expectation and rounding. Expectation-and-round beats argmax
//!   here: complexity is ordinal, so "mostly 3, sometimes 4" should land near 3.5→4 rather
//!   than snapping to whichever bin happened to be largest.
//!
//! ## Training
//!
//! Training never happens here. Weights are fitted offline by
//! [`examples/classifier_train.rs`](../../../examples/classifier_train.rs) on the labelled
//! data under `data/classifier/`, exported through [`WeightsFile`], and embedded via
//! [`embedded_model`]. **Any change to the feature engine invalidates the committed
//! weights** — the buckets move while the weights still encode the old mapping — so retrain
//! and re-embed alongside such a change. [`load_from_json_str`] enforces the dimension
//! checks that catch the mechanical half of that mistake (bucket count, head sizes).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::classifier::RequestType;
use super::text_features::{char_ngrams, fnv1a, hash_sign, word_ngrams, word_tokens};

/// Hashing-trick dimensionality for this model. Smaller than the salience model's 2^16
/// because the parameter vector is committed as a JSON asset and grows with `num_classes`;
/// 2^13 keeps the sparse export small while leaving collision pressure low for the
/// n-gram vocabularies seen in routing traffic.
pub const NUM_BUCKETS: usize = 1 << 13;

/// Number of dense (hand-picked) features, in the fixed order [`Features::dense`] produces.
pub const NUM_DENSE: usize = 8;

/// Size of the request-type head — one output per [`RequestType`].
pub const N_TYPE: usize = 7;

/// Size of the complexity head — complexity levels 1..=5.
pub const N_COMPLEXITY: usize = 5;

/// First word of a task request — a weak prior feature the model can weight, intentionally
/// small (neither exhaustive like the regex tables nor meant to stand alone). Covers the
/// openers that appear in the labelled set without enumerating every synonym.
const REQUEST_OPENERS: &[&str] = &[
    "write",
    "implement",
    "fix",
    "debug",
    "refactor",
    "explain",
    "rewrite",
    "summarize",
    "design",
    "draft",
    "analyze",
    "calculate",
    "describe",
    "add",
    "create",
    "build",
    "generate",
    "review",
    "convert",
    "translate",
    "list",
];

/// Dense feature indices, named so `Features::dense` stays self-documenting.
pub const DENSE_TOKEN_COUNT: usize = 0;
pub const DENSE_CHAR_LENGTH: usize = 1;
pub const DENSE_HAS_QUESTION_MARK: usize = 2;
pub const DENSE_HAS_CODE_MARKER: usize = 3;
pub const DENSE_HAS_OPENER: usize = 4;
pub const DENSE_CONTEXT_LENGTH: usize = 5;
pub const DENSE_HAS_DIGIT: usize = 6;
pub const DENSE_NEWLINE_COUNT: usize = 7;

/// A query's feature vector: sparse signed hashed buckets (sorted by bucket index, so the
/// dot product's floating-point accumulation order — and therefore its result — is fixed)
/// plus the fixed-order dense vector.
#[derive(Debug, Clone)]
pub struct Features {
    pub hashed: Vec<(usize, f64)>,
    pub dense: [f64; NUM_DENSE],
}

impl Features {
    /// Build the feature vector for `query` and optional `context` (e.g. pasted code or
    /// surrounding detail). Context is folded into the same hashed bag and contributes one
    /// dense length feature; it is a hint, never required.
    pub fn extract(query: &str, context: Option<&str>) -> Self {
        let mut combined = String::with_capacity(query.len() + 64);
        combined.push_str(query);
        if let Some(context) = context.filter(|c| !c.is_empty()) {
            combined.push('\n');
            combined.push_str(context);
        }

        let tokens = word_tokens(&combined);
        let mut grams = word_ngrams(&tokens, 2);
        grams.extend(char_ngrams(&combined, 3, 5));
        let norm = (grams.len() as f64).sqrt().max(1.0);

        // BTreeMap ⇒ accumulation and emission are in a fixed key order regardless of hash
        // seed, which is what makes the dot product reproducible run to run.
        let mut by_bucket: BTreeMap<usize, f64> = BTreeMap::new();
        for gram in &grams {
            let bucket = (fnv1a(gram.as_bytes()) as usize) % NUM_BUCKETS;
            *by_bucket.entry(bucket).or_insert(0.0) += hash_sign(gram) / norm;
        }

        let char_count = query.chars().count();
        let context_chars = context.map(|c| c.chars().count()).unwrap_or(0);
        let first_word_is_opener = word_tokens(query)
            .first()
            .is_some_and(|w| REQUEST_OPENERS.contains(&w.as_str()));

        let mut dense = [0.0; NUM_DENSE];
        dense[DENSE_TOKEN_COUNT] = (tokens.len() as f64).ln_1p();
        dense[DENSE_CHAR_LENGTH] = (char_count as f64).ln_1p();
        dense[DENSE_HAS_QUESTION_MARK] = if query.contains('?') { 1.0 } else { 0.0 };
        dense[DENSE_HAS_CODE_MARKER] = if query.contains("```") || query.contains('`') {
            1.0
        } else {
            0.0
        };
        dense[DENSE_HAS_OPENER] = if first_word_is_opener { 1.0 } else { 0.0 };
        dense[DENSE_CONTEXT_LENGTH] = (context_chars as f64).ln_1p();
        dense[DENSE_HAS_DIGIT] = if query.chars().any(|c| c.is_ascii_digit()) {
            1.0
        } else {
            0.0
        };
        dense[DENSE_NEWLINE_COUNT] = (query.matches('\n').count() as f64).ln_1p();

        Features {
            hashed: by_bucket.into_iter().collect(),
            dense,
        }
    }
}

/// Training provenance, carried for observability (logged at startup) and never used to
/// gate loading — the enforced compatibility contract is the dimension check in
/// [`RequestModel::from_wire`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// UTC timestamp of the training run.
    pub trained_at_utc: String,
    /// Number of labelled training queries.
    pub train_count: usize,
    /// Number of labelled validation queries.
    pub val_count: usize,
    /// Human-readable feature-engine description (what the weights were fitted against).
    pub feature: String,
    /// RNG seed used by the trainer's shuffling, for reproducibility.
    pub seed: u64,
    /// Free-form notes (e.g. measured validation accuracy).
    #[serde(default)]
    pub notes: String,
}

/// The on-disk / embedded weights schema (version 1). Deliberately a separate type from
/// [`RequestModel`]: this is the untrusted wire format (string-keyed sparse maps,
/// unvalidated dimensions); [`RequestModel`] is the validated in-memory form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightsFile {
    pub version: u32,
    pub provenance: Provenance,
    pub num_buckets: usize,
    pub num_dense: usize,
    /// Class order of the request-type head (one wire name per output).
    pub request_types: Vec<String>,
    /// Softmax temperature for the request-type head (calibration).
    pub temperature: f64,
    pub type_bias: Vec<f64>,
    pub type_dense: Vec<Vec<f64>>,
    /// Sparse hashed weights per request-type class; keys are bucket indices as strings.
    pub type_hashed: Vec<BTreeMap<String, f64>>,
    pub complexity_bias: Vec<f64>,
    pub complexity_dense: Vec<Vec<f64>>,
    pub complexity_hashed: Vec<BTreeMap<String, f64>>,
}

/// A validated request-classifier model ready to score queries.
#[derive(Debug, Clone)]
pub struct RequestModel {
    provenance: Provenance,
    temperature: f64,
    type_classes: Vec<RequestType>,
    type_bias: Vec<f64>,
    type_dense: Vec<Vec<f64>>,
    type_hashed: Vec<HashMap<usize, f64>>,
    cx_bias: Vec<f64>,
    cx_dense: Vec<Vec<f64>>,
    cx_hashed: Vec<HashMap<usize, f64>>,
}

impl RequestModel {
    /// The training provenance embedded with these weights (observability only).
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// The calibrated request-type temperature.
    pub fn temperature(&self) -> f64 {
        self.temperature
    }

    /// Validate and convert the wire format into a scoring model.
    ///
    /// Fails closed: any dimension mismatch, out-of-range bucket, unknown request-type name,
    /// or duplicate class is an error rather than a silently degraded model. A caller that
    /// cannot load the model must fall back to the regex classifier — never guess.
    pub fn from_wire(w: WeightsFile) -> Result<Self, String> {
        if w.version != 1 {
            return Err(format!("unsupported weights version {}", w.version));
        }
        if w.num_buckets != NUM_BUCKETS {
            return Err(format!(
                "weights num_buckets={} does not match this build's NUM_BUCKETS={NUM_BUCKETS}",
                w.num_buckets
            ));
        }
        if w.num_dense != NUM_DENSE {
            return Err(format!(
                "weights num_dense={} does not match this build's NUM_DENSE={NUM_DENSE}",
                w.num_dense
            ));
        }
        if !w.temperature.is_finite() || w.temperature <= 0.0 {
            return Err(format!("invalid temperature {}", w.temperature));
        }

        let type_classes = parse_classes(&w.request_types)?;
        let type_bias = check_head("type_bias", w.type_bias, N_TYPE)?;
        let type_dense = check_dense(&w.type_dense, N_TYPE)?;
        let type_hashed = check_hashed(&w.type_hashed, N_TYPE, w.num_buckets)?;

        let cx_bias = check_head("complexity_bias", w.complexity_bias, N_COMPLEXITY)?;
        let cx_dense = check_dense(&w.complexity_dense, N_COMPLEXITY)?;
        let cx_hashed = check_hashed(&w.complexity_hashed, N_COMPLEXITY, w.num_buckets)?;

        Ok(Self {
            provenance: w.provenance,
            temperature: w.temperature,
            type_classes,
            type_bias,
            type_dense,
            type_hashed,
            cx_bias,
            cx_dense,
            cx_hashed,
        })
    }

    /// Score a query: `(request_type, complexity 1..=5, confidence 0..=1)`.
    ///
    /// Deterministic — identical inputs always yield identical outputs.
    pub fn classify(&self, query: &str, context: Option<&str>) -> (RequestType, u8, f32) {
        let f = Features::extract(query, context);

        let type_logits =
            self.head_logits(&f, &self.type_bias, &self.type_dense, &self.type_hashed);
        let type_probs = softmax(&type_logits, self.temperature);
        let (idx, &confidence) = type_probs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("type head is non-empty");

        let cx_logits = self.head_logits(&f, &self.cx_bias, &self.cx_dense, &self.cx_hashed);
        let cx_probs = softmax(&cx_logits, 1.0);
        let expected: f64 = cx_probs
            .iter()
            .enumerate()
            .map(|(k, p)| (k as f64 + 1.0) * p)
            .sum();
        let complexity = expected.round().clamp(1.0, 5.0) as u8;

        (self.type_classes[idx], complexity, confidence as f32)
    }

    fn head_logits(
        &self,
        f: &Features,
        bias: &[f64],
        dense_w: &[Vec<f64>],
        hashed_w: &[HashMap<usize, f64>],
    ) -> Vec<f64> {
        let mut logits = vec![0.0; bias.len()];
        for (k, logit) in logits.iter_mut().enumerate() {
            let mut acc = bias[k];
            for (value, weight) in f.dense.iter().zip(dense_w[k].iter()) {
                acc += value * weight;
            }
            // `f.hashed` is sorted by bucket, so this accumulation order is fixed.
            for (bucket, value) in &f.hashed {
                if let Some(w) = hashed_w[k].get(bucket) {
                    acc += value * w;
                }
            }
            *logit = acc;
        }
        logits
    }
}

/// Round-trip a validated model back to the wire format, for the trainer's export path.
impl From<&RequestModel> for WeightsFile {
    fn from(m: &RequestModel) -> Self {
        let to_sparse = |maps: &[HashMap<usize, f64>]| -> Vec<BTreeMap<String, f64>> {
            maps.iter()
                .map(|m| {
                    m.iter()
                        .map(|(bucket, w)| (bucket.to_string(), *w))
                        .collect()
                })
                .collect()
        };
        WeightsFile {
            version: 1,
            provenance: m.provenance.clone(),
            num_buckets: NUM_BUCKETS,
            num_dense: NUM_DENSE,
            request_types: m
                .type_classes
                .iter()
                .map(|c| c.as_str().to_string())
                .collect(),
            temperature: m.temperature,
            type_bias: m.type_bias.clone(),
            type_dense: m.type_dense.clone(),
            type_hashed: to_sparse(&m.type_hashed),
            complexity_bias: m.cx_bias.clone(),
            complexity_dense: m.cx_dense.clone(),
            complexity_hashed: to_sparse(&m.cx_hashed),
        }
    }
}

/// Construct a model from raw head parameters. Used by the trainer to build a
/// [`RequestModel`] it can then export; construction still routes through
/// [`RequestModel::from_wire`], so a trainer bug cannot produce an unloadable asset.
#[allow(clippy::too_many_arguments)]
pub fn build_model(
    provenance: Provenance,
    temperature: f64,
    type_classes: Vec<RequestType>,
    type_bias: Vec<f64>,
    type_dense: Vec<Vec<f64>>,
    type_hashed: Vec<HashMap<usize, f64>>,
    cx_bias: Vec<f64>,
    cx_dense: Vec<Vec<f64>>,
    cx_hashed: Vec<HashMap<usize, f64>>,
) -> Result<RequestModel, String> {
    let to_sparse = |maps: &[HashMap<usize, f64>]| -> Vec<BTreeMap<String, f64>> {
        maps.iter()
            .map(|m| m.iter().map(|(b, w)| (b.to_string(), *w)).collect())
            .collect()
    };
    let wire = WeightsFile {
        version: 1,
        provenance,
        num_buckets: NUM_BUCKETS,
        num_dense: NUM_DENSE,
        request_types: type_classes
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        temperature,
        type_bias,
        type_dense,
        type_hashed: to_sparse(&type_hashed),
        complexity_bias: cx_bias,
        complexity_dense: cx_dense,
        complexity_hashed: to_sparse(&cx_hashed),
    };
    RequestModel::from_wire(wire)
}

fn parse_classes(names: &[String]) -> Result<Vec<RequestType>, String> {
    if names.len() != N_TYPE {
        return Err(format!(
            "request_types has {} entries, expected {N_TYPE}",
            names.len()
        ));
    }
    let mut classes = Vec::with_capacity(N_TYPE);
    for name in names {
        let rt = RequestType::from_wire(name)
            .ok_or_else(|| format!("request_types contains unknown class {name:?}"))?;
        if classes.contains(&rt) {
            return Err(format!("request_types contains duplicate class {name:?}"));
        }
        classes.push(rt);
    }
    Ok(classes)
}

fn check_head(name: &str, values: Vec<f64>, expected: usize) -> Result<Vec<f64>, String> {
    if values.len() != expected {
        return Err(format!(
            "{name} has {} entries, expected {expected}",
            values.len()
        ));
    }
    if !values.iter().all(|v| v.is_finite()) {
        return Err(format!("{name} contains a non-finite value"));
    }
    Ok(values)
}

fn check_dense(rows: &[Vec<f64>], expected_rows: usize) -> Result<Vec<Vec<f64>>, String> {
    if rows.len() != expected_rows {
        return Err(format!(
            "dense head has {} rows, expected {expected_rows}",
            rows.len()
        ));
    }
    for (i, row) in rows.iter().enumerate() {
        if row.len() != NUM_DENSE {
            return Err(format!(
                "dense head row {i} has {} entries, expected {NUM_DENSE}",
                row.len()
            ));
        }
        if !row.iter().all(|v| v.is_finite()) {
            return Err(format!("dense head row {i} contains a non-finite value"));
        }
    }
    Ok(rows.to_vec())
}

fn check_hashed(
    maps: &[BTreeMap<String, f64>],
    expected: usize,
    num_buckets: usize,
) -> Result<Vec<HashMap<usize, f64>>, String> {
    if maps.len() != expected {
        return Err(format!(
            "hashed head has {} maps, expected {expected}",
            maps.len()
        ));
    }
    let mut out = Vec::with_capacity(expected);
    for (i, map) in maps.iter().enumerate() {
        let mut converted = HashMap::with_capacity(map.len());
        for (key, w) in map {
            let bucket: usize = key
                .parse()
                .map_err(|_| format!("hashed head {i}: key {key:?} is not a bucket index"))?;
            if bucket >= num_buckets {
                return Err(format!(
                    "hashed head {i}: bucket {bucket} out of range (num_buckets={num_buckets})"
                ));
            }
            if !w.is_finite() {
                return Err(format!("hashed head {i}: bucket {bucket} weight is non-finite"));
            }
            converted.insert(bucket, *w);
        }
        out.push(converted);
    }
    Ok(out)
}

/// Numerically stable softmax with temperature: `softmax(logits / T)`.
pub fn softmax(logits: &[f64], temperature: f64) -> Vec<f64> {
    let t = temperature.max(1e-6);
    let max = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mut exps: Vec<f64> = logits.iter().map(|l| ((l - max) / t).exp()).collect();
    let sum: f64 = exps.iter().sum();
    if sum > 0.0 {
        for e in &mut exps {
            *e /= sum;
        }
    }
    exps
}

/// Parse the model embedded in this binary.
///
/// Infallible in practice (the bytes are fixed at compile time and a test asserts they
/// load), but returns `Result` so a corrupted asset degrades to the regex classifier
/// instead of taking startup down.
pub fn embedded_model() -> Result<RequestModel, String> {
    load_from_json_str(EMBEDDED_WEIGHTS_JSON)
}

/// The trained model compiled into this binary (see the module docs on retraining).
pub const EMBEDDED_WEIGHTS_JSON: &str = include_str!("../../assets/classifier_weights.json");

/// Load and validate a weights asset from a JSON string.
pub fn load_from_json_str(raw: &str) -> Result<RequestModel, String> {
    let wire: WeightsFile =
        serde_json::from_str(raw).map_err(|e| format!("invalid classifier weights JSON: {e}"))?;
    RequestModel::from_wire(wire)
}

/// Load and validate a weights asset from a path (the `CLASSIFIER_MODEL_PATH` override).
pub fn load_from_file(path: &std::path::Path) -> Result<RequestModel, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read classifier weights file {path:?}: {e}"))?;
    load_from_json_str(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_model_loads_and_is_versioned() {
        let model = embedded_model().expect("the embedded classifier weights must load");
        assert!(!model.provenance().trained_at_utc.is_empty());
        assert!(model.provenance().train_count > 0);
        assert!(model.temperature() > 0.0);
    }

    #[test]
    fn classify_outputs_are_in_range_and_deterministic() {
        let model = embedded_model().unwrap();
        for query in [
            "write a python function that reverses a string",
            "what is the capital of France?",
            "hi",
            "",
            "Please refactor this module to use the repository pattern and explain the trade-offs.",
        ] {
            let a = model.classify(query, None);
            let b = model.classify(query, None);
            assert_eq!(a, b, "classification must be deterministic for {query:?}");
            assert!((1..=5).contains(&a.1), "complexity out of range: {a:?}");
            assert!((0.0..=1.0).contains(&a.2), "confidence out of range: {a:?}");
        }
    }

    #[test]
    fn embedded_model_separates_clear_intents() {
        // A smoke check that the shipped artifact learned the obvious cases end to end —
        // not a substitute for the held-out eval.
        let model = embedded_model().unwrap();
        assert_eq!(
            model.classify("write a python function to parse csv", None).0,
            RequestType::CodeGeneration
        );
        assert_eq!(
            model.classify("what is the capital of France?", None).0,
            RequestType::FactualLookup
        );
    }

    #[test]
    fn features_are_sorted_and_finite() {
        let f = Features::extract("hello world", Some("```rust\nfn main() {}\n```"));
        assert!(f.hashed.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(f.hashed.iter().all(|(_, v)| v.is_finite()));
        assert!(f.dense.iter().all(|v| v.is_finite()));
        assert_eq!(f.dense[DENSE_HAS_CODE_MARKER], 1.0);
    }

    #[test]
    fn from_wire_rejects_wrong_bucket_count() {
        let mut wire = WeightsFile::from(&embedded_model().unwrap());
        wire.num_buckets = 16;
        assert!(RequestModel::from_wire(wire).is_err());
    }

    #[test]
    fn from_wire_rejects_unknown_class() {
        let mut wire = WeightsFile::from(&embedded_model().unwrap());
        wire.request_types[0] = "not_a_real_class".into();
        assert!(RequestModel::from_wire(wire).is_err());
    }

    #[test]
    fn softmax_is_a_distribution() {
        let p = softmax(&[1.0, 2.0, 3.0], 1.0);
        assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(p[2] > p[1] && p[1] > p[0]);
        // Lower temperature sharpens the distribution.
        let sharp = softmax(&[1.0, 2.0, 3.0], 0.5);
        assert!(sharp[2] > p[2]);
    }
}