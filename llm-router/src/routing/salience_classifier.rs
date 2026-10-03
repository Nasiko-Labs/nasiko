//! The salience classifier: a hashed n-gram + logistic-regression model that scores how
//! likely a query is to be *substantive* (worth classifying and pinning a model for) rather
//! than small talk.
//!
//! This module owns the model end-to-end — feature engine, weight loading, scoring, and the
//! confidence banding in [`Band`] — and knows nothing about the router. [`super::salience`]
//! holds the [`super::SalienceGate`] impl that adapts it to the routing flow; keeping the
//! two apart means the model can be scored, tested, and retrained without touching gate
//! wiring.
//!
//! Training never happens in Rust: the weights are fitted offline and land here as a JSON
//! artifact, embedded in the binary (see [`embedded_model`]) so the gate has no runtime
//! file dependency. Each weights file records how it was produced in its `provenance`
//! object — training timestamp, training-data hash, L2, and the feature-engine source it
//! was fitted against.
//!
//! **Changing the feature engine invalidates the weights.** Any edit to tokenization,
//! hashing, n-gram ranges, or the dense features changes what a given query maps to, while
//! the embedded weights still encode the *old* mapping — which degrades the model silently,
//! with no compile error and no test failure beyond the behavioural ones below. Refit and
//! re-embed alongside any such change.
//!
//! **Input contract.** [`score`] takes the query text only, matching
//! `SalienceGate::is_substantive(&self, query: &str)` — no conversation-history parameter,
//! per the decision to keep this pass query-only.

use super::features::{self, sigmoid, word_tokens};

/// Number of hashed feature buckets (the hashing-trick dimensionality). Must match the
/// `num_buckets` declared by the weights file, which [`load_model_from_json_str`] enforces.
/// Aliased from the shared [feature engine](super::features) so this model and the request
/// classifier cannot disagree about the hashing space.
pub const NUM_BUCKETS: usize = features::DEFAULT_NUM_BUCKETS;

/// Dense (non-hashed) hand-picked features, in the fixed order `dense_features` produces
/// them. Kept as named indices so `Weights::dense` stays self-documenting.
pub const NUM_DENSE_FEATURES: usize = 8;

const DENSE_TOKEN_COUNT: usize = 0;
const DENSE_CHAR_LENGTH: usize = 1;
const DENSE_CONTAINS_QUESTION_MARK: usize = 2;
const DENSE_ENDS_WITH_QUESTION_MARK: usize = 3;
const DENSE_HAS_IMPERATIVE_VERB: usize = 4;
const DENSE_DIGIT_RATIO: usize = 5;
const DENSE_EMOJI_ONLY: usize = 6;
const DENSE_PUNCTUATION_ONLY: usize = 7;

/// A short list of verbs/openers that commonly start a task request. Deliberately small
/// and conservative — this is a weak prior feature the logistic model can weight, not a
/// classifier on its own (compare `classifier.rs`'s regex vote count, which is exhaustive
/// by design; this is not).
const IMPERATIVE_OPENERS: &[&str] = &[
    "please",
    "can",
    "could",
    "would",
    "write",
    "fix",
    "debug",
    "explain",
    "refactor",
    "build",
    "create",
    "generate",
    "help",
    "show",
    "list",
    "find",
    "implement",
    "add",
    "remove",
    "delete",
    "update",
    "analyze",
    "summarize",
    "translate",
    "convert",
    "review",
];

/// A trained logistic-regression weight vector plus calibration constants.
///
/// Produced offline by a (not-yet-written) training script and either embedded as a
/// generated Rust file or loaded at startup — this struct only defines the shape.
/// [`Weights::zeros`] is a placeholder for testing the scoring math end-to-end before any
/// real weights exist; it carries no learned signal (see its doc comment).
#[derive(Debug, Clone)]
pub struct Weights {
    /// Logistic-regression intercept.
    pub bias: f64,
    /// One weight per hashed bucket, indexed by [`hash_to_bucket`]'s output.
    pub hashed: Vec<f64>,
    /// One weight per dense feature, indexed by the `DENSE_*` constants above.
    pub dense: [f64; NUM_DENSE_FEATURES],
    /// Platt scaling: `calibrated_logit = platt_a * raw_logit + platt_b`, applied before
    /// the final sigmoid. `(1.0, 0.0)` is the identity — no calibration.
    pub platt_a: f64,
    pub platt_b: f64,
}

impl Weights {
    /// An all-zero weight vector with identity calibration. `score()` on this always
    /// returns `0.5` (`sigmoid(0) == 0.5`) regardless of input — useful only for exercising
    /// the feature-extraction and scoring plumbing before real weights exist. **Not a
    /// trained model**; must not be treated as a usable classifier. Test-only now that
    /// [`load_model_from_file`] provides the real construction path.
    #[cfg(test)]
    pub fn zeros() -> Self {
        Self {
            bias: 0.0,
            hashed: vec![0.0; NUM_BUCKETS],
            dense: [0.0; NUM_DENSE_FEATURES],
            platt_a: 1.0,
            platt_b: 0.0,
        }
    }
}

/// Extract every hashed n-gram feature (word 1-2grams + char 3-5grams) from `query` into a
/// per-bucket signed sum. The signed-sum construction and the `sqrt(gram count)` normalizer
/// live in the shared [feature engine](super::features); this wrapper pins this model's
/// (frozen) n-gram ranges and bucket count so [`raw_logit`] stays consistent with the
/// embedded weights. Changing either constant invalidates the weights and needs a retrain.
fn hashed_features(query: &str) -> std::collections::HashMap<usize, f64> {
    features::hashed_feature_sum(query, NUM_BUCKETS, 2, 3, 5)
}

/// Whether every non-whitespace char in `text` is outside the Unicode letter/number range —
/// the crude "emoji/symbol-only" signal. Uses Unicode-aware `char::is_alphanumeric` (not
/// the ASCII-only variant) so text in any script — Chinese, Cyrillic, Arabic, Devanagari,
/// etc. — is correctly recognised as language, not emoji; a precise Unicode emoji
/// classification would still need a data table this module intentionally avoids per the
/// no-new-dependencies constraint, so genuinely non-letter symbol runs (e.g. mixed
/// emoji/punctuation) can still be approximate.
fn is_emoji_only(text: &str) -> bool {
    let non_space: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    !non_space.is_empty()
        && non_space.iter().all(|c| !c.is_alphanumeric())
        && non_space.iter().any(|c| !c.is_ascii_punctuation())
}

/// Whether every non-whitespace char is ASCII punctuation (e.g. `"..."`, `"???"`).
fn is_punctuation_only(text: &str) -> bool {
    let non_space: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    !non_space.is_empty() && non_space.iter().all(|c| c.is_ascii_punctuation())
}

/// Build the fixed-order dense feature vector for `query` (see the `DENSE_*` constants).
fn dense_features(query: &str) -> [f64; NUM_DENSE_FEATURES] {
    let tokens = word_tokens(query);
    let char_count = query.chars().count();
    let digit_count = query.chars().filter(|c| c.is_ascii_digit()).count();
    let digit_ratio = if char_count > 0 {
        digit_count as f64 / char_count as f64
    } else {
        0.0
    };
    let first_word_is_imperative = tokens
        .first()
        .is_some_and(|w| IMPERATIVE_OPENERS.contains(&w.as_str()));

    let mut features = [0.0; NUM_DENSE_FEATURES];
    // log1p to keep unbounded counts from dominating the dot product.
    features[DENSE_TOKEN_COUNT] = (tokens.len() as f64).ln_1p();
    features[DENSE_CHAR_LENGTH] = (char_count as f64).ln_1p();
    features[DENSE_CONTAINS_QUESTION_MARK] = if query.contains('?') { 1.0 } else { 0.0 };
    features[DENSE_ENDS_WITH_QUESTION_MARK] = if query.trim_end().ends_with('?') {
        1.0
    } else {
        0.0
    };
    features[DENSE_HAS_IMPERATIVE_VERB] = if first_word_is_imperative { 1.0 } else { 0.0 };
    features[DENSE_DIGIT_RATIO] = digit_ratio;
    features[DENSE_EMOJI_ONLY] = if is_emoji_only(query) { 1.0 } else { 0.0 };
    features[DENSE_PUNCTUATION_ONLY] = if is_punctuation_only(query) { 1.0 } else { 0.0 };
    features
}

/// A model loaded from a weights JSON file, plus the provenance string identifying the
/// training run it came from (`provenance.trained_at_utc`). The provenance is carried for
/// observability only — it is logged at startup so the running model is identifiable — and
/// is never used to gate loading; the compatibility contract that *is* enforced is the
/// feature-dimension check in [`load_model_from_json_str`].
#[derive(Debug)]
pub struct LoadedModel {
    pub weights: Weights,
    pub trained_at: String,
}

/// On-disk shape of a trained weights file, matching the training pipeline's
/// `weights_export` dict exactly. Deliberately a separate type from [`Weights`]: this one
/// is the untrusted wire format (sparse, string-keyed, unvalidated dimensions); [`Weights`]
/// is the validated, dense, ready-to-score in-memory form. `#[serde(deny_unknown_fields)]`
/// is intentionally omitted — the file also carries `schema`/`hashed_weights_format` keys
/// this loader doesn't need, and a future training-script addition shouldn't break loading.
#[derive(serde::Deserialize)]
struct WeightsFile {
    num_buckets: usize,
    num_dense_features: usize,
    bias: f64,
    dense_weights: Vec<f64>,
    hashed_weights: std::collections::HashMap<String, f64>,
    platt_a: f64,
    platt_b: f64,
    provenance: WeightsFileProvenance,
}

#[derive(serde::Deserialize)]
struct WeightsFileProvenance {
    trained_at_utc: String,
}

/// Load and validate a trained model from a weights JSON file. Fails (rather than
/// panicking or silently zero-filling) on a missing/unreadable file, malformed JSON, a
/// dimension mismatch against this build's `NUM_BUCKETS`/`NUM_DENSE_FEATURES`, or a
/// hashed-weight key that isn't a valid in-range bucket index — every failure mode a
/// caller should treat the same way: refuse the model and fall back to classifying every
/// boundary turn, never crash or serve something quietly wrong.
pub fn load_model_from_file(path: &std::path::Path) -> Result<LoadedModel, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read weights file {path:?}: {e}"))?;
    load_model_from_json_str(&raw)
}

fn load_model_from_json_str(raw: &str) -> Result<LoadedModel, String> {
    let parsed: WeightsFile =
        serde_json::from_str(raw).map_err(|e| format!("invalid weights JSON: {e}"))?;

    if parsed.num_buckets != NUM_BUCKETS {
        return Err(format!(
            "weights file num_buckets={} does not match this build's NUM_BUCKETS={NUM_BUCKETS}",
            parsed.num_buckets
        ));
    }
    if parsed.num_dense_features != NUM_DENSE_FEATURES {
        return Err(format!(
            "weights file num_dense_features={} does not match this build's NUM_DENSE_FEATURES={NUM_DENSE_FEATURES}",
            parsed.num_dense_features
        ));
    }
    if parsed.dense_weights.len() != NUM_DENSE_FEATURES {
        return Err(format!(
            "weights file dense_weights has {} entries, expected {NUM_DENSE_FEATURES}",
            parsed.dense_weights.len()
        ));
    }

    let mut hashed = vec![0.0_f64; NUM_BUCKETS];
    for (key, value) in &parsed.hashed_weights {
        let bucket: usize = key
            .parse()
            .map_err(|_| format!("hashed_weights key {key:?} is not a valid bucket index"))?;
        if bucket >= NUM_BUCKETS {
            return Err(format!(
                "hashed_weights key {bucket} is out of range (NUM_BUCKETS={NUM_BUCKETS})"
            ));
        }
        hashed[bucket] = *value;
    }

    let mut dense = [0.0_f64; NUM_DENSE_FEATURES];
    dense.copy_from_slice(&parsed.dense_weights);

    Ok(LoadedModel {
        weights: Weights {
            bias: parsed.bias,
            hashed,
            dense,
            platt_a: parsed.platt_a,
            platt_b: parsed.platt_b,
        },
        trained_at: parsed.provenance.trained_at_utc,
    })
}

/// The trained model compiled into this binary.
///
/// Embedding it (rather than reading a path at startup) is what lets the gate be on by
/// default with no deployment step: there is no file to ship alongside the binary, no path
/// to configure, and no startup I/O that can fail in production. Retraining means replacing
/// this asset and rebuilding.
/// Operators can still override it at runtime with `SALIENCE_WEIGHTS_PATH`, which is the
/// escape hatch for testing a candidate model without a rebuild.
const EMBEDDED_WEIGHTS_JSON: &str = include_str!("../../assets/salience_weights.json");

/// Parse the model embedded in this binary.
///
/// This is infallible in practice — the bytes are fixed at compile time and
/// `embedded_model_parses` asserts they load — but it still returns `Result` rather than
/// panicking, so a corrupted asset degrades the gate instead of taking the router's startup
/// down with it.
pub fn embedded_model() -> Result<LoadedModel, String> {
    load_model_from_json_str(EMBEDDED_WEIGHTS_JSON)
}

/// Where a score falls relative to the configured thresholds.
///
/// The gate's policy hangs entirely off this three-way split rather than a single cutoff,
/// because "confidently small talk" and "not sure" call for opposite actions — see
/// [`super::salience::ClassifierSalienceGate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// `score < low_threshold` — confidently small talk.
    ConfidentSmallTalk,
    /// Between the thresholds — the model has no confident answer.
    Uncertain,
    /// `score > high_threshold` — confidently substantive.
    ConfidentSubstantive,
}

impl Band {
    /// Band `probability` against the configured thresholds.
    pub fn of(probability: f64, low_threshold: f64, high_threshold: f64) -> Self {
        if probability < low_threshold {
            Band::ConfidentSmallTalk
        } else if probability > high_threshold {
            Band::ConfidentSubstantive
        } else {
            Band::Uncertain
        }
    }

    /// A short, stable label for structured logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Band::ConfidentSmallTalk => "confident_small_talk",
            Band::Uncertain => "uncertain",
            Band::ConfidentSubstantive => "confident_substantive",
        }
    }
}

/// The raw (pre-calibration) logistic-regression logit for `query` under `weights`: bias
/// plus the hashed-feature dot product plus the dense-feature dot product.
fn raw_logit(query: &str, weights: &Weights) -> f64 {
    let mut logit = weights.bias;
    for (bucket, value) in hashed_features(query) {
        logit += value * weights.hashed[bucket];
    }
    let dense = dense_features(query);
    for (value, weight) in dense.iter().zip(weights.dense.iter()) {
        logit += value * weight;
    }
    logit
}

/// `P(sufficient_context)` for `query` under `weights`: the raw logit, Platt-calibrated,
/// then squashed through the sigmoid. With [`Weights::zeros`] this is always `0.5` — see
/// that constructor's doc comment.
pub fn score(query: &str, weights: &Weights) -> f64 {
    let logit = raw_logit(query, weights);
    let calibrated = weights.platt_a * logit + weights.platt_b;
    sigmoid(calibrated)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Primitives the engine owns, exercised here directly (the module's non-test code goes
    // through `hashed_features`, which wraps them).
    use super::features::{char_ngrams, fnv1a, hash_to_bucket, word_ngrams};

    /// End-to-end check against the actual shipped artifact — not a synthetic fixture.
    /// Loads `assets/salience_weights.json` through the real [`load_model_from_file`] path
    /// and checks it scores a clear small-talk query low and a clear task query high,
    /// proving the loader + scorer combination works against what training actually
    /// produced, not just against hand-written test JSON.
    ///
    /// This asserts rather than skips: the weights are committed to the repo and embedded
    /// in the binary, so their absence is a build-breaking error, not a dev-checkout
    /// variation.
    #[test]
    fn real_trained_weights_score_sensibly() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/salience_weights.json");
        let model = load_model_from_file(&path).expect("real trained weights should load");
        let small_talk_score = score("hi", &model.weights);
        let task_score = score(
            "Can you refactor this function to use async/await?",
            &model.weights,
        );
        assert!(
            small_talk_score < 0.5,
            "expected 'hi' to score low, got {small_talk_score}"
        );
        assert!(
            task_score > 0.5,
            "expected a clear task query to score high, got {task_score}"
        );
    }

    fn valid_weights_json() -> String {
        format!(
            r#"{{
                "num_buckets": {NUM_BUCKETS},
                "num_dense_features": {NUM_DENSE_FEATURES},
                "bias": -0.5,
                "dense_weights": [1.0, -2.0, 0.5, 0.5, 2.0, -1.0, 0.0, 0.0],
                "hashed_weights": {{"5": 0.3, "100": -0.1}},
                "platt_a": 0.9,
                "platt_b": 0.1,
                "provenance": {{"trained_at_utc": "2026-09-10T12:43:22Z"}}
            }}"#
        )
    }

    #[test]
    fn load_model_from_json_str_valid_input_round_trips() {
        let model = load_model_from_json_str(&valid_weights_json()).expect("should parse");
        assert_eq!(model.trained_at, "2026-09-10T12:43:22Z");
        assert_eq!(model.weights.bias, -0.5);
        assert_eq!(
            model.weights.dense,
            [1.0, -2.0, 0.5, 0.5, 2.0, -1.0, 0.0, 0.0]
        );
        assert_eq!(model.weights.hashed.len(), NUM_BUCKETS);
        assert_eq!(model.weights.hashed[5], 0.3);
        assert_eq!(model.weights.hashed[100], -0.1);
        // every other bucket defaults to 0.0
        assert_eq!(model.weights.hashed[6], 0.0);
    }

    #[test]
    fn load_model_from_json_str_rejects_malformed_json() {
        assert!(load_model_from_json_str("not json").is_err());
        assert!(load_model_from_json_str("{}").is_err());
    }

    #[test]
    fn load_model_from_json_str_rejects_bucket_count_mismatch() {
        let json = valid_weights_json().replace(
            &format!("\"num_buckets\": {NUM_BUCKETS}"),
            "\"num_buckets\": 4",
        );
        let err = load_model_from_json_str(&json).unwrap_err();
        assert!(err.contains("num_buckets"), "unexpected error: {err}");
    }

    #[test]
    fn load_model_from_json_str_rejects_dense_feature_count_mismatch() {
        let json = valid_weights_json().replace(
            &format!("\"num_dense_features\": {NUM_DENSE_FEATURES}"),
            "\"num_dense_features\": 3",
        );
        let err = load_model_from_json_str(&json).unwrap_err();
        assert!(
            err.contains("num_dense_features"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn load_model_from_json_str_rejects_dense_weights_length_mismatch() {
        let json = valid_weights_json()
            .replace("[1.0, -2.0, 0.5, 0.5, 2.0, -1.0, 0.0, 0.0]", "[1.0, 2.0]");
        let err = load_model_from_json_str(&json).unwrap_err();
        assert!(err.contains("dense_weights"), "unexpected error: {err}");
    }

    #[test]
    fn load_model_from_json_str_rejects_out_of_range_bucket_index() {
        let json = valid_weights_json().replace(
            r#"{"5": 0.3, "100": -0.1}"#,
            &format!(r#"{{"{NUM_BUCKETS}": 0.3}}"#),
        );
        let err = load_model_from_json_str(&json).unwrap_err();
        assert!(err.contains("out of range"), "unexpected error: {err}");
    }

    #[test]
    fn load_model_from_json_str_rejects_non_numeric_bucket_key() {
        let json =
            valid_weights_json().replace(r#"{"5": 0.3, "100": -0.1}"#, r#"{"not-a-number": 0.3}"#);
        let err = load_model_from_json_str(&json).unwrap_err();
        assert!(
            err.contains("not a valid bucket index"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn load_model_from_file_reads_and_parses() {
        let path = std::env::temp_dir().join(format!(
            "nasiko-salience-test-{}-{}.json",
            std::process::id(),
            "load_model_from_file_reads_and_parses"
        ));
        std::fs::write(&path, valid_weights_json()).expect("write temp file");
        let model = load_model_from_file(&path).expect("should load");
        assert_eq!(model.trained_at, "2026-09-10T12:43:22Z");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_model_from_file_missing_path_errors() {
        let path = std::env::temp_dir().join("nasiko-salience-test-definitely-does-not-exist.json");
        let err = load_model_from_file(&path).unwrap_err();
        assert!(err.contains("failed to read"), "unexpected error: {err}");
    }

    #[test]
    fn fnv1a_is_deterministic() {
        assert_eq!(fnv1a(b"hello"), fnv1a(b"hello"));
        assert_ne!(fnv1a(b"hello"), fnv1a(b"world"));
    }

    #[test]
    fn hash_to_bucket_stays_in_range() {
        for gram in ["hi", "refactor this", "a", "🎉🎉🎉", ""] {
            let (bucket, sign) = hash_to_bucket(gram, NUM_BUCKETS);
            assert!(bucket < NUM_BUCKETS);
            assert!(sign == 1.0 || sign == -1.0);
        }
    }

    #[test]
    fn word_tokens_lowercases_and_splits_on_punctuation() {
        assert_eq!(
            word_tokens("Hi! Can you help me?"),
            vec!["hi", "can", "you", "help", "me"]
        );
        assert!(word_tokens("   ").is_empty());
    }

    #[test]
    fn word_ngrams_produces_1_and_2_grams() {
        let tokens = word_tokens("fix this bug");
        let grams = word_ngrams(&tokens, 2);
        assert!(grams.contains(&"fix".to_string()));
        assert!(grams.contains(&"this".to_string()));
        assert!(grams.contains(&"fix this".to_string()));
        assert!(grams.contains(&"this bug".to_string()));
        assert!(!grams.contains(&"fix this bug".to_string()));
    }

    #[test]
    fn char_ngrams_handles_multibyte_without_panicking() {
        // Code-switched input is exactly the case char n-grams exist to cover.
        let grams = char_ngrams("Hola, ¿cómo estás?", 3, 5);
        assert!(!grams.is_empty());
        assert!(grams.iter().all(|g| g.chars().count() >= 3));
    }

    #[test]
    fn char_ngrams_shorter_than_min_n_yields_nothing() {
        assert!(char_ngrams("hi", 3, 5).is_empty());
    }

    #[test]
    fn dense_features_detect_question_and_imperative() {
        let f = dense_features("Can you refactor this?");
        assert_eq!(f[DENSE_CONTAINS_QUESTION_MARK], 1.0);
        assert_eq!(f[DENSE_ENDS_WITH_QUESTION_MARK], 1.0);
        assert_eq!(f[DENSE_HAS_IMPERATIVE_VERB], 1.0);
    }

    #[test]
    fn dense_features_plain_greeting_has_no_imperative_or_question() {
        let f = dense_features("hello there");
        assert_eq!(f[DENSE_CONTAINS_QUESTION_MARK], 0.0);
        assert_eq!(f[DENSE_ENDS_WITH_QUESTION_MARK], 0.0);
        assert_eq!(f[DENSE_HAS_IMPERATIVE_VERB], 0.0);
    }

    #[test]
    fn digit_ratio_reflects_proportion_of_digits() {
        let f = dense_features("2024");
        assert_eq!(f[DENSE_DIGIT_RATIO], 1.0);
        let f = dense_features("no digits here");
        assert_eq!(f[DENSE_DIGIT_RATIO], 0.0);
    }

    #[test]
    fn emoji_only_detects_pure_emoji_and_rejects_mixed_text() {
        assert!(is_emoji_only("🎉🔥"));
        assert!(!is_emoji_only("nice 🎉"));
        assert!(!is_emoji_only(""));
    }

    #[test]
    fn emoji_only_does_not_misclassify_non_latin_scripts() {
        // Regression: an earlier ASCII-only check flagged any non-Latin-script text
        // (Chinese, Cyrillic, Arabic, Devanagari, ...) as "emoji-only", which would have
        // undermined the whole point of the char-n-gram code-switching support.
        assert!(!is_emoji_only("你好，请帮我调试这个"));
        assert!(!is_emoji_only("Привет, помоги мне"));
        assert!(!is_emoji_only("مرحبا، ساعدني"));
        assert!(!is_emoji_only("नमस्ते, मेरी मदद करो"));
    }

    #[test]
    fn hashed_features_normalization_dampens_repetition_growth() {
        // Repeating one word 50x still accumulates its own bucket linearly before the
        // sqrt(total-gram-count) division, so magnitude isn't flattened to equal — but it
        // is damped from a 50x raw-count blowup down to roughly sqrt-scale growth. This
        // documents the actual (sub-linear, not eliminated) effect rather than assuming
        // full length-invariance.
        let short = hashed_features("debug");
        let long = hashed_features(&"debug ".repeat(50));
        let short_max = short.values().cloned().fold(0.0_f64, f64::max);
        let long_max = long.values().cloned().fold(0.0_f64, f64::max);
        assert!(short_max > 0.0 && long_max > 0.0);
        let ratio = long_max / short_max;
        assert!(
            ratio < 10.0,
            "expected repetition growth to be damped well below the raw 50x repeat factor, got ratio {ratio} (short={short_max}, long={long_max})"
        );
    }

    #[test]
    fn hashed_features_empty_query_does_not_divide_by_zero() {
        let features = hashed_features("");
        assert!(features.values().all(|v| v.is_finite()));
    }

    #[test]
    fn punctuation_only_detects_pure_punctuation_and_rejects_letters() {
        assert!(is_punctuation_only("???"));
        assert!(is_punctuation_only("..."));
        assert!(!is_punctuation_only("ok?"));
        assert!(!is_punctuation_only(""));
    }

    #[test]
    fn sigmoid_bounds_and_midpoint() {
        assert_eq!(sigmoid(0.0), 0.5);
        assert!(sigmoid(100.0) > 0.999);
        assert!(sigmoid(-100.0) < 0.001);
    }

    #[test]
    fn zero_weights_always_score_one_half() {
        let weights = Weights::zeros();
        for query in ["hi", "refactor this function to use async/await", "", "🎉"] {
            assert_eq!(score(query, &weights), 0.5);
        }
    }

    #[test]
    fn score_is_deterministic_for_the_same_input() {
        let weights = Weights::zeros();
        let a = score("hello there", &weights);
        let b = score("hello there", &weights);
        assert_eq!(a, b);
    }

    #[test]
    fn score_stays_in_unit_interval_with_nonzero_weights() {
        // Not a trained model — just exercises the dot product with non-degenerate
        // weights so a future real weight vector can't silently blow the sigmoid's domain.
        let mut weights = Weights::zeros();
        for w in weights.hashed.iter_mut().take(100) {
            *w = 3.5;
        }
        weights.dense = [1.0, -2.0, 0.5, 0.5, 2.0, -1.0, 0.0, 0.0];
        weights.bias = -0.7;
        for query in [
            "hi",
            "refactor this function",
            "Hola, can you help me debug this?",
        ] {
            let p = score(query, &weights);
            assert!((0.0..=1.0).contains(&p), "probability out of range: {p}");
        }
    }
}
