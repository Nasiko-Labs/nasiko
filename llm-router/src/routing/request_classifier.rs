//! P2 — Request Classifier for Cost-Aware Routing.
//!
//! This module provides the [`RequestClassifier`] trait, a shared abstraction that
//! the router (Level 3 in [`super::mod`]) and the evaluator
//! (`classifier_eval` example) both exercise through the same code path.
//!
//! ## Two implementations
//!
//! | Type                   | Enabled when                      | Produces                          |
//! |------------------------|-----------------------------------|-----------------------------------|
//! | [`RegexClassifier`]    | `CLASSIFIER_ENABLED=false` (default) | Same output as the legacy `classify_request_type` call — behavior **unchanged**. |
//! | [`MlRequestClassifier`]| `CLASSIFIER_ENABLED=true`         | 7-class request-type + 5-class complexity via logistic regression over hashed n-grams + dense features, with temperature-scaled confidence. Falls back to regex below `threshold` or on any error. |
//!
//! ## Routing integration
//!
//! At Level 3, the router calls:
//! ```text
//! ctx.request_classifier.classify(query, context)
//!     ↓
//! Classification { request_type, complexity, confidence }
//!     ↓
//! pick_model_thompson(&cells, request_type, w_q, w_c, rng)   ← unchanged
//! ```
//! Only `request_type` participates in Thompson sampling; `complexity` and `confidence`
//! are carried for the evaluator and future observability.
//!
//! ## Sticky routing
//!
//! Classification only triggers at `cold_start` / `switch` boundaries
//! (`is_fireable_boundary() == true`). During `continue` (tool loops), the router
//! reads the cached `CachedDecision` at Level 2 — this module is never called.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::classifier::{RequestType, classify_request_type};

// ---------------------------------------------------------------------------
// Public output type
// ---------------------------------------------------------------------------

/// The full result produced by any [`RequestClassifier`] implementation.
///
/// `request_type` is the field the router passes to `pick_model_thompson`;
/// `complexity` and `confidence` are carried for the evaluator JSONL output
/// and future observability use.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    /// 7-way request type — fed directly into `pick_model_thompson`.
    pub request_type: RequestType,
    /// 5-level complexity estimate (1 = trivial, 5 = very complex).
    /// `0` is the sentinel value used by `RegexClassifier` (unknown / not computed).
    pub complexity: u8,
    /// Posterior confidence in `request_type` after temperature scaling.
    /// Range `[0, 1]`. `RegexClassifier` returns `1.0` (it never hedges).
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// Shared trait
// ---------------------------------------------------------------------------

/// Classification abstraction shared by the router and the evaluator.
///
/// Both call sites use the same implementation / configuration path — this is the
/// single point of truth for P2 classification, not a bifurcated system.
pub trait RequestClassifier: Send + Sync {
    /// Classify `query` into a [`Classification`].
    ///
    /// `context` is the optional conversation context (preceding turns).
    /// Implementations may use it to improve classification.
    ///
    /// This function is intentionally **synchronous and cheap** — it runs in the
    /// Level-3 hot path before the async Thompson-sampling call.
    fn classify(&self, query: &str, context: Option<&str>) -> Classification;
}

// ---------------------------------------------------------------------------
// RegexClassifier — wraps the existing classify_request_type, default path
// ---------------------------------------------------------------------------

/// The default classifier: delegates to the existing
/// [`classify_request_type`] regex vote-count implementation.
///
/// When `CLASSIFIER_ENABLED=false` (the default), `LlmRouterCtx` holds an
/// `Arc<RegexClassifier>` and the router's behavior is **byte-for-byte
/// identical** to the behavior before P2 was merged.
pub struct RegexClassifier;

impl RequestClassifier for RegexClassifier {
    fn classify(&self, query: &str, _context: Option<&str>) -> Classification {
        let request_type = classify_request_type(query);
        Classification {
            request_type,
            // Regex fallback does not compute complexity (0 = sentinel / unknown).
            complexity: 0,
            // The regex is deterministic and has no probabilistic confidence.
            confidence: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Feature engine (independent from salience_classifier.rs)
// ---------------------------------------------------------------------------
//
// The helpers below are intentionally NOT imported from salience_classifier.rs.
// The P2 feature engine has different n-gram ranges and additional structural
// features; keeping the two engines decoupled means changing one can never
// silently affect the other's model.

/// FNV-1a over raw bytes — deterministic across runs/platforms.
#[inline]
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Number of hashed feature buckets (hashing-trick dimensionality).
/// Must match `num_buckets` in the weights file.
const NUM_BUCKETS: usize = 1 << 16; // 65 536

/// Hash an n-gram into `(bucket, sign)` using the hashing trick.
#[inline]
fn hash_to_bucket(gram: &str) -> (usize, f64) {
    let bucket = (fnv1a(gram.as_bytes()) as usize) % NUM_BUCKETS;
    let sign_bit = fnv1a(format!("sign:{gram}").as_bytes()) & 1;
    (bucket, if sign_bit == 0 { 1.0 } else { -1.0 })
}

/// Lowercase word tokens, splitting on non-alphanumeric characters.
fn word_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Word n-grams for `n` in `1..=max_n`.
fn word_ngrams(tokens: &[String], max_n: usize) -> Vec<String> {
    let mut grams = Vec::new();
    for n in 1..=max_n {
        if n > tokens.len() {
            break;
        }
        for window in tokens.windows(n) {
            grams.push(window.join(" "));
        }
    }
    grams
}

/// Character n-grams for `n` in `min_n..=max_n` (char-aware, not byte).
fn char_ngrams(text: &str, min_n: usize, max_n: usize) -> Vec<String> {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut grams = Vec::new();
    for n in min_n..=max_n {
        if n > chars.len() {
            break;
        }
        for window in chars.windows(n) {
            grams.push(window.iter().collect());
        }
    }
    grams
}

/// Logistic sigmoid.
#[allow(dead_code)]
#[inline]
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Softmax over a slice, returning a new `Vec<f64>`.
fn softmax(logits: &[f64]) -> Vec<f64> {
    let max = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f64 = exps.iter().sum();
    exps.iter().map(|&e| e / sum.max(1e-12)).collect()
}

// ---------------------------------------------------------------------------
// Dense feature constants and extractors
// ---------------------------------------------------------------------------

const P2_DENSE_SENTENCE_COUNT: usize = 0;
const P2_DENSE_QUESTION_MARK: usize = 1;
const P2_DENSE_CODE_FENCE: usize = 2;
const P2_DENSE_URL: usize = 3;
const P2_DENSE_LIST_ITEMS: usize = 4;
const P2_DENSE_CONSTRAINT_WORDS: usize = 5;
const P2_DENSE_SEQUENTIAL_WORDS: usize = 6;
const P2_DENSE_CONTEXT_LEN: usize = 7;
const P2_DENSE_CHAR_LENGTH: usize = 8;
const P2_DENSE_TOKEN_COUNT: usize = 9;

/// Total number of P2 dense features.
pub const P2_NUM_DENSE: usize = 10;

/// Words that signal multi-constraint or planning queries.
const CONSTRAINT_WORDS: &[&str] = &[
    "must", "ensure", "require", "requires", "constraint", "constraints",
    "necessary", "mandatory", "need to", "needs to", "have to", "has to",
    "should", "ought",
];

/// Words that signal step-by-step / sequential reasoning.
const SEQUENTIAL_WORDS: &[&str] = &[
    "step", "steps", "first", "then", "next", "finally", "lastly",
    "procedure", "process", "workflow", "pipeline", "sequence",
];

/// Build the fixed-order P2 dense feature vector (10 features).
fn p2_dense_features(query: &str, context: Option<&str>) -> [f64; P2_NUM_DENSE] {
    let q_lower = query.to_lowercase();
    let tokens = word_tokens(query);
    let char_count = query.chars().count() as f64;

    let sentence_count = query
        .split(['.', '!', '?'])
        .filter(|s| !s.trim().is_empty())
        .count()
        .max(1) as f64;

    let has_question_mark = f64::from(query.contains('?'));

    let has_code_fence = f64::from(query.contains("```") || query.contains("~~~"));

    let has_url = f64::from(
        q_lower.contains("http://")
            || q_lower.contains("https://")
            || q_lower.contains("www."),
    );

    let list_items = query
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.starts_with('-') || t.starts_with('*') || {
                let first = t.chars().next();
                first.map(|c| c.is_ascii_digit()).unwrap_or(false) && t.contains('.')
            }
        })
        .count() as f64;

    let constraint_count = CONSTRAINT_WORDS
        .iter()
        .filter(|&&w| q_lower.contains(w))
        .count() as f64;

    let sequential_count = SEQUENTIAL_WORDS
        .iter()
        .filter(|&&w| q_lower.contains(w))
        .count() as f64;

    let context_len = context.map(|c| c.chars().count()).unwrap_or(0) as f64;

    let mut features = [0.0f64; P2_NUM_DENSE];
    features[P2_DENSE_SENTENCE_COUNT] = sentence_count.ln_1p();
    features[P2_DENSE_QUESTION_MARK] = has_question_mark;
    features[P2_DENSE_CODE_FENCE] = has_code_fence;
    features[P2_DENSE_URL] = has_url;
    features[P2_DENSE_LIST_ITEMS] = list_items.ln_1p();
    features[P2_DENSE_CONSTRAINT_WORDS] = constraint_count.ln_1p();
    features[P2_DENSE_SEQUENTIAL_WORDS] = sequential_count.ln_1p();
    features[P2_DENSE_CONTEXT_LEN] = context_len.ln_1p();
    features[P2_DENSE_CHAR_LENGTH] = char_count.ln_1p();
    features[P2_DENSE_TOKEN_COUNT] = (tokens.len() as f64).ln_1p();
    features
}

/// Build the per-bucket signed-sum hashed feature map.
///
/// Uses word 1-2grams from `query + optional context prefix (300 chars)`
/// and char 3-5grams from `query` only. Normalized by `sqrt(gram count)`.
fn p2_hashed_features(query: &str, context: Option<&str>) -> HashMap<usize, f64> {
    let ctx_prefix: String = context.unwrap_or("").chars().take(300).collect();
    let combined = if ctx_prefix.is_empty() {
        query.to_string()
    } else {
        format!("{ctx_prefix} {query}")
    };

    let tokens = word_tokens(&combined);
    let mut grams = word_ngrams(&tokens, 2);
    grams.extend(char_ngrams(query, 3, 5));

    let norm = (grams.len() as f64).sqrt().max(1.0);
    let mut by_bucket: HashMap<usize, f64> = HashMap::new();
    for gram in &grams {
        let (bucket, sign) = hash_to_bucket(gram);
        *by_bucket.entry(bucket).or_insert(0.0) += sign / norm;
    }
    by_bucket
}

// ---------------------------------------------------------------------------
// Regex vote features — 7 binary signals, one per RequestType
// ---------------------------------------------------------------------------

/// Binary one-hot features aligned to the 7 `RequestType` class order.
///
/// Gives the ML model direct access to the regex signal as an ablation-friendly
/// weak prior — the logistic weights can learn to trust or ignore it per class.
///
/// Order must match `REQUEST_TYPE_CLASSES`.
fn regex_vote_features(query: &str) -> [f64; 7] {
    let rt = classify_request_type(query);
    let idx = request_type_to_idx(rt);
    let mut feats = [0.0f64; 7];
    feats[idx] = 1.0;
    feats
}

// ---------------------------------------------------------------------------
// Class index helpers
// ---------------------------------------------------------------------------

/// Canonical ordering of the 7 request-type classes.
/// **Must stay stable** across weight exports.
pub const REQUEST_TYPE_CLASSES: [RequestType; 7] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];

/// Canonical ordering of 5 complexity levels (1-indexed values).
pub const COMPLEXITY_CLASSES: [u8; 5] = [1, 2, 3, 4, 5];

/// Convert `RequestType` → class index (deterministic, O(7)).
#[inline]
pub fn request_type_to_idx(rt: RequestType) -> usize {
    REQUEST_TYPE_CLASSES.iter().position(|&r| r == rt).unwrap_or(6)
}

#[inline]
fn idx_to_request_type(idx: usize) -> RequestType {
    REQUEST_TYPE_CLASSES.get(idx).copied().unwrap_or(RequestType::General)
}

#[inline]
fn idx_to_complexity(idx: usize) -> u8 {
    COMPLEXITY_CLASSES.get(idx).copied().unwrap_or(3)
}

// ---------------------------------------------------------------------------
// Weight schema
// ---------------------------------------------------------------------------

/// In-memory P2 multi-class weight matrix.
///
/// Two independent classifier heads share the same feature vector:
/// - **type head**: 7 classes (`REQUEST_TYPE_CLASSES`)
/// - **complexity head**: 5 classes (`COMPLEXITY_CLASSES`)
///
/// Temperature scaling: logits are divided by `temperature` before softmax.
#[derive(Debug, Clone)]
pub struct P2Weights {
    /// Sparse hashed-feature weights per type class: `type_weights[cls][bucket]`.
    pub type_weights: Vec<Vec<f64>>,       // [7][NUM_BUCKETS]
    /// Sparse hashed-feature weights per complexity class.
    pub complexity_weights: Vec<Vec<f64>>, // [5][NUM_BUCKETS]
    /// Per-class bias for the type head. Length 7.
    pub type_bias: Vec<f64>,
    /// Per-class bias for the complexity head. Length 5.
    pub complexity_bias: Vec<f64>,
    /// Temperature scalar (≥ 0.1). Divides raw logits before softmax.
    pub temperature: f64,
    /// Confidence threshold: below this → regex fallback.
    pub threshold: f64,
    /// Human-readable training provenance (logged at startup).
    pub trained_at: String,
}

/// Full P2 model: hashed weights + dense tail weights.
#[derive(Debug, Clone)]
pub struct P2Model {
    pub weights: P2Weights,
    /// Dense + regex-vote weights for the type head: `[7][P2_NUM_DENSE + 7]`.
    pub type_dense: Vec<Vec<f64>>,
    /// Dense + regex-vote weights for the complexity head: `[5][P2_NUM_DENSE + 7]`.
    pub complexity_dense: Vec<Vec<f64>>,
}

// ---------------------------------------------------------------------------
// Disk format
// ---------------------------------------------------------------------------

/// Wire format for `assets/request_classifier_weights.json`.
#[derive(serde::Deserialize)]
struct P2WeightsFile {
    #[allow(dead_code)]
    schema: Option<String>,
    num_buckets: usize,
    num_dense_features: usize,
    temperature: f64,
    type_bias: Vec<f64>,
    complexity_bias: Vec<f64>,
    /// Sparse: key = `"bucket_idx:class_idx"`, value = weight.
    type_hashed_weights: HashMap<String, f64>,
    /// Dense + regex-vote weights: `[7][P2_NUM_DENSE + 7]`.
    type_dense_weights: Vec<Vec<f64>>,
    /// Sparse: key = `"bucket_idx:class_idx"`, value = weight.
    complexity_hashed_weights: HashMap<String, f64>,
    /// Dense + regex-vote weights: `[5][P2_NUM_DENSE + 7]`.
    complexity_dense_weights: Vec<Vec<f64>>,
    trained_at: Option<String>,
}

/// Load a full [`P2Model`] from a JSON string.
pub fn load_p2_model_from_json(json: &str, threshold: f64) -> Result<P2Model, String> {
    let file: P2WeightsFile =
        serde_json::from_str(json).map_err(|e| format!("P2 weights JSON parse error: {e}"))?;

    if file.num_buckets != NUM_BUCKETS {
        return Err(format!("P2 num_buckets={} != {NUM_BUCKETS}", file.num_buckets));
    }
    if file.num_dense_features != P2_NUM_DENSE {
        return Err(format!(
            "P2 num_dense_features={} != {P2_NUM_DENSE}",
            file.num_dense_features
        ));
    }
    if file.type_bias.len() != 7 {
        return Err(format!("type_bias len={} != 7", file.type_bias.len()));
    }
    if file.complexity_bias.len() != 5 {
        return Err(format!("complexity_bias len={} != 5", file.complexity_bias.len()));
    }
    if file.type_dense_weights.len() != 7 {
        return Err(format!(
            "type_dense_weights rows={} != 7",
            file.type_dense_weights.len()
        ));
    }
    if file.complexity_dense_weights.len() != 5 {
        return Err(format!(
            "complexity_dense_weights rows={} != 5",
            file.complexity_dense_weights.len()
        ));
    }
    let dense_tail = P2_NUM_DENSE + 7;
    for (i, row) in file.type_dense_weights.iter().enumerate() {
        if row.len() != dense_tail {
            return Err(format!("type_dense_weights[{i}] len={} != {dense_tail}", row.len()));
        }
    }
    for (i, row) in file.complexity_dense_weights.iter().enumerate() {
        if row.len() != dense_tail {
            return Err(format!(
                "complexity_dense_weights[{i}] len={} != {dense_tail}",
                row.len()
            ));
        }
    }

    let mut type_hashed = vec![vec![0.0f64; NUM_BUCKETS]; 7];
    for (key, &val) in &file.type_hashed_weights {
        let (bucket, cls) = parse_sparse_key(key)?;
        if cls < 7 {
            type_hashed[cls][bucket] = val;
        }
    }

    let mut complexity_hashed = vec![vec![0.0f64; NUM_BUCKETS]; 5];
    for (key, &val) in &file.complexity_hashed_weights {
        let (bucket, cls) = parse_sparse_key(key)?;
        if cls < 5 {
            complexity_hashed[cls][bucket] = val;
        }
    }

    Ok(P2Model {
        weights: P2Weights {
            type_weights: type_hashed,
            complexity_weights: complexity_hashed,
            type_bias: file.type_bias,
            complexity_bias: file.complexity_bias,
            temperature: file.temperature.max(0.1),
            threshold,
            trained_at: file.trained_at.unwrap_or_else(|| "unknown".to_string()),
        },
        type_dense: file.type_dense_weights,
        complexity_dense: file.complexity_dense_weights,
    })
}

fn parse_sparse_key(key: &str) -> Result<(usize, usize), String> {
    let mut parts = key.splitn(2, ':');
    let bucket: usize = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad sparse key: {key}"))?;
    let cls: usize = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad sparse key: {key}"))?;
    if bucket >= NUM_BUCKETS {
        return Err(format!("bucket {bucket} >= NUM_BUCKETS {NUM_BUCKETS}"));
    }
    Ok((bucket, cls))
}

// ---------------------------------------------------------------------------
// MlRequestClassifier
// ---------------------------------------------------------------------------

/// ML-backed P2 request classifier.
///
/// Scores the query (+ optional context prefix) using logistic regression over
/// hashed n-grams, dense structural features, and regex-vote features.
/// Temperature scaling calibrates the softmax confidence.
///
/// Falls back to [`RegexClassifier`] when confidence < `threshold` or on timeout.
pub struct MlRequestClassifier {
    model: P2Model,
    fallback: RegexClassifier,
    timeout: Duration,
}

/// Statically embedded P2 weights, built into the binary.
pub const EMBEDDED_WEIGHTS_JSON: &str =
    include_str!("../../assets/request_classifier_weights.json");

impl MlRequestClassifier {
    /// Load from the statically embedded weights asset.
    pub fn embedded(threshold: f64, timeout_ms: u64) -> Result<Self, String> {
        Self::from_json(EMBEDDED_WEIGHTS_JSON, threshold, timeout_ms)
    }

    /// Load from a weights JSON string.
    pub fn from_json(json: &str, threshold: f64, timeout_ms: u64) -> Result<Self, String> {
        let model = load_p2_model_from_json(json, threshold)?;
        Ok(Self {
            model,
            fallback: RegexClassifier,
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    /// Load from a file path.
    pub fn from_path(path: &str, threshold: f64, timeout_ms: u64) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read P2 weights from '{path}': {e}"))?;
        Self::from_json(&json, threshold, timeout_ms)
    }

    /// Score a single example, returning `(type_probs[7], complexity_probs[5])`.
    fn score(&self, query: &str, context: Option<&str>) -> ([f64; 7], [f64; 5]) {
        let hashed = p2_hashed_features(query, context);
        let mut sorted_hashed: Vec<(usize, f64)> = hashed.into_iter().collect();
        sorted_hashed.sort_unstable_by_key(|&(b, _)| b);

        let dense = p2_dense_features(query, context);
        let regex_votes = regex_vote_features(query);
        let temp = self.model.weights.temperature;

        // --- Type head ---
        let type_logits: Vec<f64> = (0..7)
            .map(|cls| {
                let hashed_w = &self.model.weights.type_weights[cls];
                let dense_w = &self.model.type_dense[cls];
                let bias = self.model.weights.type_bias[cls];

                let hashed_dot: f64 =
                    sorted_hashed.iter().map(|&(b, fv)| hashed_w[b] * fv).sum();
                let dense_dot: f64 = dense
                    .iter()
                    .zip(&dense_w[..P2_NUM_DENSE])
                    .map(|(&f, &w)| f * w)
                    .sum();
                let regex_dot: f64 = regex_votes
                    .iter()
                    .zip(&dense_w[P2_NUM_DENSE..])
                    .map(|(&f, &w)| f * w)
                    .sum();
                (bias + hashed_dot + dense_dot + regex_dot) / temp
            })
            .collect();

        let type_probs_v = softmax(&type_logits);
        let mut type_probs = [0.0f64; 7];
        type_probs.copy_from_slice(&type_probs_v);

        // --- Complexity head ---
        let complexity_logits: Vec<f64> = (0..5)
            .map(|cls| {
                let hashed_w = &self.model.weights.complexity_weights[cls];
                let dense_w = &self.model.complexity_dense[cls];
                let bias = self.model.weights.complexity_bias[cls];

                let hashed_dot: f64 =
                    sorted_hashed.iter().map(|&(b, fv)| hashed_w[b] * fv).sum();
                let dense_dot: f64 = dense
                    .iter()
                    .zip(&dense_w[..P2_NUM_DENSE])
                    .map(|(&f, &w)| f * w)
                    .sum();
                let regex_dot: f64 = regex_votes
                    .iter()
                    .zip(&dense_w[P2_NUM_DENSE..])
                    .map(|(&f, &w)| f * w)
                    .sum();
                (bias + hashed_dot + dense_dot + regex_dot) / temp
            })
            .collect();

        let complexity_probs_v = softmax(&complexity_logits);
        let mut complexity_probs = [0.0f64; 5];
        complexity_probs.copy_from_slice(&complexity_probs_v);

        (type_probs, complexity_probs)
    }
}

impl RequestClassifier for MlRequestClassifier {
    fn classify(&self, query: &str, context: Option<&str>) -> Classification {
        let started = Instant::now();
        let (type_probs, complexity_probs) = self.score(query, context);

        let elapsed = started.elapsed();
        if elapsed > self.timeout {
            tracing::warn!(
                target: "nasiko::llm_router::request_classifier",
                elapsed_ms = elapsed.as_millis(),
                timeout_ms = self.timeout.as_millis(),
                "MlRequestClassifier: scoring exceeded timeout, falling back to regex"
            );
            return self.fallback.classify(query, context);
        }

        let (type_idx, &confidence) = type_probs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap_or((6, &0.0));

        let complexity_idx = complexity_probs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(2);
        let complexity = idx_to_complexity(complexity_idx);

        // Below threshold → use regex for request_type but still return ML complexity/confidence.
        if confidence < self.model.weights.threshold {
            tracing::debug!(
                target: "nasiko::llm_router::request_classifier",
                confidence,
                threshold = self.model.weights.threshold,
                "MlRequestClassifier: confidence below threshold, falling back to regex for request_type"
            );
            let fallback = self.fallback.classify(query, context);
            return Classification {
                request_type: fallback.request_type,
                complexity,
                confidence,
            };
        }

        let request_type = idx_to_request_type(type_idx);
        tracing::debug!(
            target: "nasiko::llm_router::request_classifier",
            request_type = request_type.as_str(),
            complexity,
            confidence,
            elapsed_us = elapsed.as_micros(),
            "MlRequestClassifier: classified"
        );

        Classification {
            request_type,
            complexity,
            confidence,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_classifier_matches_classify_request_type() {
        let clf = RegexClassifier;
        let cases = [
            "build me a python script that parses CSV",
            "explain what this function does",
            "design a microservices architecture",
            "what is the capital of France",
            "write a poem about autumn",
        ];
        for q in &cases {
            let expected = classify_request_type(q);
            let got = clf.classify(q, None);
            assert_eq!(
                got.request_type, expected,
                "RegexClassifier diverged for: {q}"
            );
            assert_eq!(got.complexity, 0, "RegexClassifier sentinel complexity must be 0");
            assert!(
                (got.confidence - 1.0).abs() < 1e-9,
                "RegexClassifier confidence must be 1.0"
            );
        }
    }

    #[test]
    fn request_type_idx_roundtrip() {
        for &rt in &REQUEST_TYPE_CLASSES {
            let idx = request_type_to_idx(rt);
            assert_eq!(idx_to_request_type(idx), rt, "roundtrip failed for {:?}", rt);
        }
    }

    #[test]
    fn softmax_sums_to_one() {
        let logits = [1.0, 2.0, 3.0, 0.5, -1.0, 4.0, 2.5];
        let probs = softmax(&logits);
        let sum: f64 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "softmax sum={sum}");
    }

    #[test]
    fn p2_dense_features_has_correct_len() {
        let feats = p2_dense_features("hello world", None);
        assert_eq!(feats.len(), P2_NUM_DENSE);
    }

    #[test]
    fn regex_vote_features_one_hot() {
        let cases = [
            "write a Python function",
            "explain this code",
            "what is 2 + 2",
        ];
        for q in &cases {
            let feats = regex_vote_features(q);
            let ones: usize = feats.iter().filter(|&&x| (x - 1.0).abs() < 1e-9).count();
            let zeros: usize = feats.iter().filter(|&&x| x.abs() < 1e-9).count();
            assert_eq!(ones, 1, "expected one-hot for: {q}");
            assert_eq!(zeros, 6, "expected six zeros for: {q}");
        }
    }

    #[test]
    fn zero_weight_model_falls_back_to_regex() {
        // Uniform logits → confidence = 1/7 ≈ 0.143 < 0.70 threshold → regex fallback.
        let model = P2Model {
            weights: P2Weights {
                type_weights: vec![vec![0.0; NUM_BUCKETS]; 7],
                complexity_weights: vec![vec![0.0; NUM_BUCKETS]; 5],
                type_bias: vec![0.0; 7],
                complexity_bias: vec![0.0; 5],
                temperature: 1.0,
                threshold: 0.70,
                trained_at: "test".to_string(),
            },
            type_dense: vec![vec![0.0; P2_NUM_DENSE + 7]; 7],
            complexity_dense: vec![vec![0.0; P2_NUM_DENSE + 7]; 5],
        };
        let clf = MlRequestClassifier {
            model,
            fallback: RegexClassifier,
            timeout: Duration::from_millis(100),
        };
        let q = "build me a python script that parses CSV";
        let ml = clf.classify(q, None);
        let regex = classify_request_type(q);
        assert_eq!(
            ml.request_type, regex,
            "zero-weight model should fall back to regex"
        );
    }

    #[test]
    fn embedded_weights_load_and_classify() {
        let clf = MlRequestClassifier::embedded(0.70, 5000)
            .expect("embedded weights must be valid and load successfully");
        let result = clf.classify("write a python function to calculate fibonacci", None);
        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert!(result.complexity >= 1 && result.complexity <= 5);
        assert!(result.confidence > 0.0 && result.confidence <= 1.0);
    }

    #[test]
    fn python_rust_feature_parity_fixtures() {
        let fixtures_json = include_str!("../../training/parity_fixtures.json");
        let fixtures: Vec<serde_json::Value> =
            serde_json::from_str(fixtures_json).expect("valid parity fixtures json");

        for item in fixtures {
            let q = item["query"].as_str().unwrap();
            let ctx = item.get("context").and_then(|c| c.as_str());

            // 1. Dense features parity
            let rust_dense = p2_dense_features(q, ctx);
            let py_dense: Vec<f64> = item["dense"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            assert_eq!(rust_dense.len(), py_dense.len());
            for (i, (&r, &p)) in rust_dense.iter().zip(&py_dense).enumerate() {
                assert!(
                    (r - p).abs() < 1e-5,
                    "Dense feature {i} mismatch for '{q}': rust={r}, py={p}"
                );
            }

            // 2. Regex vote parity
            let rust_votes = regex_vote_features(q);
            let py_votes: Vec<f64> = item["regex_votes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            assert_eq!(rust_votes.len(), py_votes.len());
            for (i, (&r, &p)) in rust_votes.iter().zip(&py_votes).enumerate() {
                assert!(
                    (r - p).abs() < 1e-6,
                    "Regex vote {i} mismatch for '{q}': rust={r}, py={p}"
                );
            }

            // 3. Hashed features parity (bucket count, sum, and sampled buckets)
            let rust_hashed = p2_hashed_features(q, ctx);
            let py_num_buckets = item["num_hashed_buckets"].as_u64().unwrap() as usize;
            assert_eq!(
                rust_hashed.len(),
                py_num_buckets,
                "Hashed bucket count mismatch for '{q}'"
            );

            let rust_sum: f64 = rust_hashed.values().sum();
            let py_sum = item["sum_hashed"].as_f64().unwrap();
            assert!(
                (rust_sum - py_sum).abs() < 1e-4,
                "Hashed sum mismatch for '{q}': rust={rust_sum}, py={py_sum}"
            );

            if let Some(samples) = item.get("sample_hashed").and_then(|s| s.as_array()) {
                for sample in samples {
                    let bucket = sample["bucket"].as_u64().unwrap() as usize;
                    let py_val = sample["val"].as_f64().unwrap();
                    let rust_val = rust_hashed.get(&bucket).copied().unwrap_or(0.0);
                    assert!(
                        (rust_val - py_val).abs() < 1e-5,
                        "Hashed bucket {bucket} mismatch for '{q}': rust={rust_val}, py={py_val}"
                    );
                }
            }
        }
    }
}
