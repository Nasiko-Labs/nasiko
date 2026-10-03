//! Local (in-process) request classifier: a hashed n-gram + multinomial logistic-regression
//! model, trained offline by `training/train_request_classifier.py` and embedded in the
//! binary.
//!
//! This is the "local" backend of the [`RequestClassifier`](super::classifier::RequestClassifier)
//! interface. It answers two questions at once, because the router already has to learn both:
//!
//! - **Request type** — a 7-way softmax over [`RequestType`], so the router's Thompson
//!   tier selection can key on the kind of work rather than a handful of keywords.
//! - **Complexity** — a linear regressor rounded to 1–5, matching the public eval rubric.
//!
//! ## Why a local model
//!
//! The router's hot path cannot depend on a network call to decide which model to call: a
//! slow or unreachable classifier would either add latency to every routed request or force a
//! fallback that defeats the point. The salience gate already showed the pattern works here
//! (see [`super::salience_classifier`]): fit offline, embed the weights, score in
//! microseconds with no I/O and no runtime file dependency. Operators who prefer a hosted
//! model can select the `hosted` backend instead; regex remains the default.
//!
//! Inference is **pure and deterministic** — no RNG, no clock, no allocation beyond the
//! feature map — so the same input always yields the same verdict, which is what the
//! reproducibility requirement asks for.
//!
//! ## Feature compatibility
//!
//! The exported weights encode a specific feature mapping: the shared engine's n-gram ranges
//! and bucket count, plus this module's six dense features in a fixed order. The loader
//! validates every one of those constants against the file and refuses a mismatch rather than
//! silently scoring with the wrong mapping. Changing any constant here means retraining and
//! re-exporting the asset.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
};
use super::features;

/// Number of hashing-trick buckets. Must equal the value the weights file declares.
pub const NUM_BUCKETS: usize = 1 << 12; // 4096
/// Number of dense features (see [`dense_features`]). Must equal the weights file's.
pub const NUM_DENSE: usize = 6;
/// Number of request-type labels.
pub const NUM_LABELS: usize = 7;

const WORD_MAX_N: usize = 2;
const CHAR_MIN_N: usize = 3;
const CHAR_MAX_N: usize = 5;
/// Context n-grams contribute at this weight (see [`hashed_features`]). Must match the
/// training script's `CONTEXT_WEIGHT`.
const CONTEXT_WEIGHT: f64 = 0.5;

/// Label order — MUST match the training script's `LABELS`, which in turn mirrors
/// [`RequestType::as_str`]. The loader refuses a file whose labels don't parse to all seven
/// distinct types.
pub const LABELS: [RequestType; NUM_LABELS] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];

/// Constraint cue phrases for dense feature 5 (a weak complexity signal). Must match the
/// training script's `CONSTRAINT_CUES`.
const CONSTRAINT_CUES: [&str; 14] = [
    "must", "should", "do not", "don't", "without", "only", "at least", "ensure", "exactly",
    "before", "after", "instead", "reject", "never",
];

/// The embedded weights, validated and shaped for scoring.
#[derive(Debug)]
struct Weights {
    bias: [f64; NUM_LABELS],
    dense: [[f64; NUM_DENSE]; NUM_LABELS],
    /// One 7-vector per bucket; the vast majority are all-zero.
    hashed: Vec<[f64; NUM_LABELS]>,
    complexity_bias: f64,
    complexity_dense: [f64; NUM_DENSE],
    complexity_hashed: Vec<f64>,
    temperature: f64,
}

/// A validated model plus the provenance of the training run that produced it. The provenance
/// is for observability (logged at startup) and never gates loading; the compatibility
/// contract that *is* enforced is the feature-dimension check in [`load_model_from_json_str`].
#[derive(Debug)]
pub struct LoadedModel {
    pub classifier: LocalRequestClassifier,
    pub trained_at: String,
    pub dataset_sha256: String,
    pub train_examples: usize,
}

impl LoadedModel {
    /// Convenience passthrough to `self.classifier.score` — the provenance fields exist only
    /// for logging, so callers that just want a verdict don't have to destructure the wrapper.
    pub fn score(&self, query: &str, context: Option<&str>) -> Classification {
        self.classifier.score(query, context)
    }
}

/// The local classifier. Construct via [`LocalRequestClassifier::embedded`] or
/// [`LocalRequestClassifier::from_path`]; scoring itself is infallible.
#[derive(Debug)]
pub struct LocalRequestClassifier {
    weights: Weights,
}

impl LocalRequestClassifier {
    /// Load the model embedded in this binary (the normal path).
    pub fn embedded() -> Result<LoadedModel, String> {
        load_model_from_json_str(EMBEDDED_WEIGHTS_JSON)
    }

    /// Load a weights file from disk — the `CLASSIFIER_MODEL_PATH` override, for trialling a
    /// candidate model without a rebuild.
    pub fn from_path(path: &Path) -> Result<LoadedModel, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read request-classifier weights {path:?}: {e}"))?;
        load_model_from_json_str(&raw)
    }

    /// Deterministic synchronous scoring. Kept separate from the async trait impl so tests
    /// (and any synchronous caller) can score without a runtime.
    pub fn score(&self, query: &str, context: Option<&str>) -> Classification {
        let dense = dense_features(query, context);
        // Sort the sparse features by bucket before accumulating. `HashMap` iteration order
        // is randomized per map instance, and floating-point addition is not associative, so
        // summing in map order could make the logits (and, at a knife-edge, the argmax) differ
        // between runs. A fixed ascending-bucket order makes scoring bit-for-bit
        // reproducible, and matches the order scikit-learn's sparse dot product used at
        // training time.
        let mut feats: Vec<(usize, f64)> = hashed_features(query, context).into_iter().collect();
        feats.sort_unstable_by_key(|(bucket, _)| *bucket);

        let mut logits = self.weights.bias;
        for (bucket, value) in &feats {
            let column = &self.weights.hashed[*bucket];
            for c in 0..NUM_LABELS {
                logits[c] += value * column[c];
            }
        }
        for (i, value) in dense.iter().enumerate() {
            if *value == 0.0 {
                continue;
            }
            for c in 0..NUM_LABELS {
                logits[c] += value * self.weights.dense[c][i];
            }
        }

        // Temperature scaling then softmax. Argmax is invariant to the temperature; it only
        // shapes the confidence.
        for l in logits.iter_mut() {
            *l /= self.weights.temperature;
        }
        features::softmax_in_place(&mut logits);

        let (index, confidence) = logits
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .expect("NUM_LABELS > 0");

        let mut complexity_raw = self.weights.complexity_bias;
        for (bucket, value) in &feats {
            complexity_raw += value * self.weights.complexity_hashed[*bucket];
        }
        for (i, value) in dense.iter().enumerate() {
            complexity_raw += value * self.weights.complexity_dense[i];
        }
        let complexity = complexity_raw.clamp(1.0, 5.0).round() as u8;

        Classification {
            request_type: LABELS[index],
            complexity,
            confidence: *confidence as f32,
        }
    }
}

#[async_trait]
impl RequestClassifier for LocalRequestClassifier {
    fn name(&self) -> &str {
        "local_linear"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        // Pure CPU work — no await point, no failure mode beyond the (already validated)
        // weights being present.
        Ok(self.score(input.query, input.context))
    }
}

/// Combine query and context exactly as training does. An empty/whitespace context is
/// dropped so a missing context never changes the features.
fn combined_text(query: &str, context: Option<&str>) -> String {
    match context.map(str::trim).filter(|c| !c.is_empty()) {
        Some(context) => format!("{query}\n{context}"),
        None => query.to_string(),
    }
}

/// The hashed feature map: the query's n-grams, plus the context's n-grams at
/// [`CONTEXT_WEIGHT`].
///
/// Query and context are normalized *separately* (each by its own `sqrt(gram count)`) so a
/// long context cannot dilute the query's magnitudes — the failure mode when both are
/// concatenated before normalizing. The context still contributes, at reduced weight, so
/// attached snippets and framing inform the verdict without dominating it.
fn hashed_features(query: &str, context: Option<&str>) -> HashMap<usize, f64> {
    let mut out =
        features::hashed_feature_sum(query, NUM_BUCKETS, WORD_MAX_N, CHAR_MIN_N, CHAR_MAX_N);
    if let Some(context) = context.map(str::trim).filter(|c| !c.is_empty()) {
        for (bucket, value) in
            features::hashed_feature_sum(context, NUM_BUCKETS, WORD_MAX_N, CHAR_MIN_N, CHAR_MAX_N)
        {
            *out.entry(bucket).or_insert(0.0) += CONTEXT_WEIGHT * value;
        }
    }
    out
}

/// The six dense features, in the fixed order the weights were trained with:
/// 0. log1p(word count of the query)
/// 1. log1p(char count of the query)
/// 2. log1p(char count of the context)
/// 3. contains a backtick/code fence
/// 4. query contains `?`
/// 5. log1p(count of constraint cue phrases in the combined text)
fn dense_features(query: &str, context: Option<&str>) -> [f64; NUM_DENSE] {
    let context = context.unwrap_or("");
    let text = combined_text(query, Some(context));
    let lower = text.to_lowercase();
    let constraint_hits = CONSTRAINT_CUES
        .iter()
        .filter(|cue| lower.contains(**cue))
        .count();
    [
        (features::word_tokens(query).len() as f64).ln_1p(),
        (query.chars().count() as f64).ln_1p(),
        (context.trim().chars().count() as f64).ln_1p(),
        if text.contains('`') { 1.0 } else { 0.0 },
        if query.contains('?') { 1.0 } else { 0.0 },
        (constraint_hits as f64).ln_1p(),
    ]
}

// ---------------------------------------------------------------------------
// Weights file (untrusted wire format) + validation
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WeightsFile {
    schema: String,
    num_buckets: usize,
    num_dense_features: usize,
    word_max_n: usize,
    char_min_n: usize,
    char_max_n: usize,
    context_weight: f64,
    labels: Vec<String>,
    bias: Vec<f64>,
    dense_weights: Vec<Vec<f64>>,
    hashed_weights: HashMap<String, Vec<f64>>,
    temperature: f64,
    complexity: ComplexityFile,
    provenance: ProvenanceFile,
}

#[derive(Deserialize)]
struct ComplexityFile {
    bias: f64,
    dense_weights: Vec<f64>,
    hashed_weights: HashMap<String, f64>,
}

#[derive(Deserialize)]
struct ProvenanceFile {
    trained_at_utc: String,
    dataset_sha256: String,
    train_examples: usize,
}

const EXPECTED_SCHEMA: &str = "nasiko-request-classifier-weights-v1";

/// Parse and validate a weights JSON string. Returns a descriptive `Err` on any mismatch —
/// a missing file, malformed JSON, a stale feature-engine constant, a label set that doesn't
/// match this build, or an out-of-range bucket key. The caller treats every failure the same
/// way: keep the regex classifier rather than score with the wrong weights.
fn load_model_from_json_str(raw: &str) -> Result<LoadedModel, String> {
    let parsed: WeightsFile =
        serde_json::from_str(raw).map_err(|e| format!("invalid weights JSON: {e}"))?;

    if parsed.schema != EXPECTED_SCHEMA {
        return Err(format!(
            "unexpected weights schema {:?}, expected {EXPECTED_SCHEMA:?}",
            parsed.schema
        ));
    }
    if parsed.num_buckets != NUM_BUCKETS {
        return Err(format!(
            "weights num_buckets={} does not match this build's NUM_BUCKETS={NUM_BUCKETS}",
            parsed.num_buckets
        ));
    }
    if parsed.num_dense_features != NUM_DENSE {
        return Err(format!(
            "weights num_dense_features={} does not match this build's NUM_DENSE={NUM_DENSE}",
            parsed.num_dense_features
        ));
    }
    if (parsed.word_max_n, parsed.char_min_n, parsed.char_max_n)
        != (WORD_MAX_N, CHAR_MIN_N, CHAR_MAX_N)
    {
        return Err(format!(
            "weights n-gram ranges (word<={}, char {}..={}) do not match this build (word<={WORD_MAX_N}, char {CHAR_MIN_N}..={CHAR_MAX_N})",
            parsed.word_max_n, parsed.char_min_n, parsed.char_max_n
        ));
    }
    if parsed.context_weight != CONTEXT_WEIGHT {
        return Err(format!(
            "weights context_weight={} does not match this build's CONTEXT_WEIGHT={CONTEXT_WEIGHT}",
            parsed.context_weight
        ));
    }
    if parsed.labels.len() != NUM_LABELS {
        return Err(format!(
            "weights declare {} labels, expected {NUM_LABELS}",
            parsed.labels.len()
        ));
    }
    // Every label must parse, and the set must be exactly the seven request types.
    let mut seen = [false; NUM_LABELS];
    for label in &parsed.labels {
        let rt = RequestType::from_wire(label)
            .ok_or_else(|| format!("weights contain unknown request type {label:?}"))?;
        let index = LABELS
            .iter()
            .position(|l| *l == rt)
            .expect("from_wire yields a known type");
        if seen[index] {
            return Err(format!("weights declare duplicate label {label:?}"));
        }
        seen[index] = true;
    }
    if !seen.iter().all(|s| *s) {
        return Err("weights do not declare all seven request types".to_string());
    }

    if parsed.bias.len() != NUM_LABELS {
        return Err(format!(
            "weights bias has {} entries, expected {NUM_LABELS}",
            parsed.bias.len()
        ));
    }
    if parsed.dense_weights.len() != NUM_LABELS {
        return Err(format!(
            "weights dense_weights has {} rows, expected {NUM_LABELS}",
            parsed.dense_weights.len()
        ));
    }
    let mut dense = [[0.0_f64; NUM_DENSE]; NUM_LABELS];
    for (c, row) in parsed.dense_weights.iter().enumerate() {
        if row.len() != NUM_DENSE {
            return Err(format!(
                "weights dense_weights[{c}] has {} entries, expected {NUM_DENSE}",
                row.len()
            ));
        }
        dense[c].copy_from_slice(row);
    }
    let bias: [f64; NUM_LABELS] = parsed
        .bias
        .try_into()
        .map_err(|_| "weights bias has the wrong length".to_string())?;

    if !parsed.temperature.is_finite() || parsed.temperature <= 0.0 {
        return Err(format!(
            "weights temperature {} is not a positive finite number",
            parsed.temperature
        ));
    }

    let mut hashed = vec![[0.0_f64; NUM_LABELS]; NUM_BUCKETS];
    for (key, values) in &parsed.hashed_weights {
        let bucket: usize = key
            .parse()
            .map_err(|_| format!("hashed_weights key {key:?} is not a valid bucket index"))?;
        if bucket >= NUM_BUCKETS {
            return Err(format!(
                "hashed_weights key {bucket} is out of range (NUM_BUCKETS={NUM_BUCKETS})"
            ));
        }
        if values.len() != NUM_LABELS {
            return Err(format!(
                "hashed_weights[{bucket}] has {} values, expected {NUM_LABELS}",
                values.len()
            ));
        }
        hashed[bucket].copy_from_slice(values);
    }

    if parsed.complexity.dense_weights.len() != NUM_DENSE {
        return Err(format!(
            "complexity.dense_weights has {} entries, expected {NUM_DENSE}",
            parsed.complexity.dense_weights.len()
        ));
    }
    let mut complexity_dense = [0.0_f64; NUM_DENSE];
    complexity_dense.copy_from_slice(&parsed.complexity.dense_weights);
    let mut complexity_hashed = vec![0.0_f64; NUM_BUCKETS];
    for (key, value) in &parsed.complexity.hashed_weights {
        let bucket: usize = key.parse().map_err(|_| {
            format!("complexity.hashed_weights key {key:?} is not a valid bucket index")
        })?;
        if bucket >= NUM_BUCKETS {
            return Err(format!(
                "complexity.hashed_weights key {bucket} is out of range"
            ));
        }
        complexity_hashed[bucket] = *value;
    }

    Ok(LoadedModel {
        classifier: LocalRequestClassifier {
            weights: Weights {
                bias,
                dense,
                hashed,
                complexity_bias: parsed.complexity.bias,
                complexity_dense,
                complexity_hashed,
                temperature: parsed.temperature,
            },
        },
        trained_at: parsed.provenance.trained_at_utc,
        dataset_sha256: parsed.provenance.dataset_sha256,
        train_examples: parsed.provenance.train_examples,
    })
}

/// The trained model compiled into this binary — no runtime file dependency, which is what
/// lets the local backend be selected with a single env var and no deployment step.
const EMBEDDED_WEIGHTS_JSON: &str = include_str!("../../assets/request_classifier_weights.json");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_model_loads_and_reports_provenance() {
        let model = LocalRequestClassifier::embedded().expect("embedded weights must load");
        assert!(!model.trained_at.is_empty());
        assert_eq!(
            model.dataset_sha256.len(),
            64,
            "sha256 of the training data"
        );
        assert!(model.train_examples > 0);
    }

    #[test]
    fn embedded_model_separates_clear_request_types() {
        // The shipped model's actual verdicts on unambiguous inputs — not a synthetic
        // stand-in. A coding task should not read as writing, and a greeting should fall in
        // the general bucket.
        let model = LocalRequestClassifier::embedded().unwrap();
        let write = model.score("Write a Python function that reverses a string.", None);
        assert_eq!(write.request_type, RequestType::CodeGeneration);
        let explain = model.score("Explain why this function returns the cached value.", None);
        assert_eq!(explain.request_type, RequestType::CodeUnderstanding);
        let fact = model.score("What is the capital of France?", None);
        assert_eq!(fact.request_type, RequestType::FactualLookup);
        let greet = model.score("Hello, how are you?", None);
        assert_eq!(greet.request_type, RequestType::General);
        let design = model.score(
            "Design a database schema for multi-tenant audit logs.",
            None,
        );
        assert_eq!(design.request_type, RequestType::TechnicalDesign);
    }

    #[test]
    fn scoring_is_deterministic() {
        let model = LocalRequestClassifier::embedded().unwrap();
        let a = model.score("Refactor this function to be pure.", None);
        let b = model.score("Refactor this function to be pure.", None);
        assert_eq!(a, b);
    }

    #[test]
    fn confidence_stays_in_unit_interval_and_complexity_in_range() {
        let model = LocalRequestClassifier::embedded().unwrap();
        for q in [
            "",
            "hi",
            "Design migration from sync to queued processing with rollout phases.",
            "Fix typo",
            "Write a Rust function that merges two sorted slices.",
        ] {
            let c = model.score(q, None);
            assert!(
                (0.0..=1.0).contains(&c.confidence),
                "confidence out of range: {c:?}"
            );
            assert!(
                (1..=5).contains(&c.complexity),
                "complexity out of range: {c:?}"
            );
        }
    }

    #[test]
    fn context_can_change_the_features() {
        // Same query, different context ⇒ the combined text differs, so a trained model that
        // uses context is free to change its verdict. This only asserts the plumbing (an
        // empty context must be identical to no context).
        let model = LocalRequestClassifier::embedded().unwrap();
        assert_eq!(
            model.score("Explain this.", None),
            model.score("Explain this.", Some("   ")),
        );
        // A long code context is a different input; it must still produce a valid verdict.
        let with_code = model.score("Explain this.", Some("```rust\nlet x = 1;\n```"));
        assert!((1..=5).contains(&with_code.complexity));
    }

    #[test]
    fn loader_rejects_a_bucket_count_mismatch() {
        let json = EMBEDDED_WEIGHTS_JSON.replace("\"num_buckets\": 4096", "\"num_buckets\": 8");
        let err = load_model_from_json_str(&json).unwrap_err();
        assert!(err.contains("num_buckets"), "unexpected error: {err}");
    }

    #[test]
    fn loader_rejects_an_unknown_label() {
        let json = EMBEDDED_WEIGHTS_JSON.replace("\"code_generation\"", "\"not_a_type\"");
        let err = load_model_from_json_str(&json).unwrap_err();
        assert!(
            err.contains("unknown request type"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn loader_rejects_malformed_json() {
        assert!(load_model_from_json_str("{}").is_err());
        assert!(load_model_from_json_str("not json").is_err());
    }

    #[test]
    fn loader_rejects_missing_file() {
        let err = LocalRequestClassifier::from_path(Path::new("/nonexistent/weights.json"))
            .expect_err("a missing weights file must be an error");
        assert!(err.contains("failed to read"), "unexpected error: {err}");
    }

    #[test]
    fn loader_reads_a_valid_file_from_disk() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/request_classifier_weights.json");
        let model = LocalRequestClassifier::from_path(&path).expect("real asset must load");
        assert!(model.train_examples > 0);
    }
}
