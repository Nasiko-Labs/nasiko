//! Shared deterministic text featurization for the router's in-process linear models.
//!
//! Two models in this crate are trained offline and shipped as embedded weight vectors:
//! the Level 2.5 [salience classifier](super::salience_classifier) (small talk vs.
//! substantive) and the Level 3 [request classifier](super::request_classifier) (request
//! type + complexity). Both score hashed n-gram features, and a weight vector is only
//! meaningful if the feature *mapping* that produced it is reproduced exactly at inference
//! time. Keeping one copy of that mapping here — tokenization, n-gram ranges, FNV-1a
//! hashing, sign selection and the length normalizer — means the two models cannot drift
//! apart, and a change to the engine breaks both models' tests loudly instead of silently
//! degrading one of them.
//!
//! Everything in this module is pure: no I/O, no environment reads, no random state.
//! Determinism is a correctness property, not an incidental one — the same text must map to
//! the same feature vector on every run, machine and Rust version. That is why the hashing
//! is a hand-written FNV-1a rather than [`std::collections::hash_map::DefaultHasher`], whose
//! output is explicitly not guaranteed stable across Rust releases.
//!
//! ## The feature space
//!
//! A text is represented as a sparse map `bucket -> signed weight` (the hashing trick): word
//! 1..=`word_max_n`-grams plus character `char_min_n..=char_max_n`-grams are each hashed to a
//! bucket, and colliding grams accumulate with their own signs so they partially cancel
//! instead of always reinforcing. The total is divided by `sqrt(gram_count)` to damp — not
//! eliminate — the way long inputs inflate every score.
//!
//! Callers that need extra task-specific signal (input length, code fences, …) add their own
//! dense features on top; those are classifier-specific and deliberately do not live here.

use std::collections::HashMap;

/// Default hashing-trick dimensionality. Models that fit this size use it directly; a model
/// with a smaller training set may choose a smaller space to keep its weight file small, but
/// it must declare the same value it was trained with (the loader enforces this).
pub(crate) const DEFAULT_NUM_BUCKETS: usize = 1 << 16;

/// FNV-1a over raw bytes — dependency-free and deterministic across runs, platforms and Rust
/// versions. A trained weight vector is only meaningful if hashing is stable, so this must
/// never change without retraining every embedded model.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Hash an n-gram string into `(bucket, sign)` using the standard hashing-trick
/// construction: one hash picks the bucket, a second (differently-salted) hash picks the
/// sign, so hash collisions partially cancel instead of only ever adding.
pub(crate) fn hash_to_bucket(gram: &str, num_buckets: usize) -> (usize, f64) {
    let bucket = (fnv1a(gram.as_bytes()) as usize) % num_buckets;
    let sign_bit = fnv1a(format!("sign:{gram}").as_bytes()) & 1;
    let sign = if sign_bit == 0 { 1.0 } else { -1.0 };
    (bucket, sign)
}

/// Lowercase word tokens, splitting on anything that isn't alphanumeric. Empty tokens are
/// dropped, so runs of punctuation/whitespace just act as separators.
pub(crate) fn word_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Word n-grams (`n` consecutive tokens joined by a single space) for `n` in `1..=max_n`.
pub(crate) fn word_ngrams(tokens: &[String], max_n: usize) -> Vec<String> {
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

/// Character n-grams over the lowercased text, for `n` in `min_n..=max_n`. Whitespace is
/// lowercased but NOT stripped or collapsed — it stays part of the char stream and can
/// appear inside a gram. Operates on `char`s (not bytes) so multi-byte UTF-8 isn't split
/// mid-codepoint, which is what makes the engine carry code-switched/non-Latin input (e.g.
/// `"Hola, ..."`) without a network call.
pub(crate) fn char_ngrams(text: &str, min_n: usize, max_n: usize) -> Vec<String> {
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

/// Extract every hashed n-gram feature from `text` into a per-bucket signed sum: each gram
/// contributes its own `sign` (not a sign borrowed from whichever gram happened to hash into
/// that bucket first), so colliding grams with opposite signs partially cancel as the
/// hashing trick intends. The sum is then divided by `sqrt(total gram count)`, which dampens
/// (does not eliminate) how much sheer input length can inflate a logit.
///
/// `word_max_n`, `char_min_n` and `char_max_n` are the n-gram ranges; they are parameters
/// rather than constants because each model may declare a different (and, once trained,
/// frozen) range.
pub(crate) fn hashed_feature_sum(
    text: &str,
    num_buckets: usize,
    word_max_n: usize,
    char_min_n: usize,
    char_max_n: usize,
) -> HashMap<usize, f64> {
    let tokens = word_tokens(text);
    let mut grams = word_ngrams(&tokens, word_max_n);
    grams.extend(char_ngrams(text, char_min_n, char_max_n));
    let norm = (grams.len() as f64).sqrt().max(1.0);

    let mut by_bucket: HashMap<usize, f64> = HashMap::new();
    for gram in &grams {
        let (bucket, sign) = hash_to_bucket(gram, num_buckets);
        *by_bucket.entry(bucket).or_insert(0.0) += sign / norm;
    }
    by_bucket
}

/// Standard logistic sigmoid, `1 / (1 + e^-x)`.
pub(crate) fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Numerically stable in-place softmax over a slice of logits. Subtracting the max keeps the
/// exponentials in range; an all-`-inf` input yields a uniform distribution rather than NaN.
pub(crate) fn softmax_in_place(logits: &mut [f64]) {
    let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() {
        let uniform = 1.0 / logits.len().max(1) as f64;
        logits.fill(uniform);
        return;
    }
    let mut sum = 0.0;
    for x in logits.iter_mut() {
        *x = (*x - max).exp();
        sum += *x;
    }
    if sum > 0.0 {
        for x in logits.iter_mut() {
            *x /= sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_is_deterministic_and_distinguishes_inputs() {
        assert_eq!(fnv1a(b"hello"), fnv1a(b"hello"));
        assert_ne!(fnv1a(b"hello"), fnv1a(b"world"));
    }

    #[test]
    fn hash_to_bucket_stays_in_range_and_signs_are_unit() {
        for gram in ["hi", "refactor this", "a", "🎉🎉🎉", ""] {
            for buckets in [8, 1024, DEFAULT_NUM_BUCKETS] {
                let (bucket, sign) = hash_to_bucket(gram, buckets);
                assert!(bucket < buckets);
                assert!(sign == 1.0 || sign == -1.0);
            }
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
        let grams = char_ngrams("Hola, ¿cómo estás?", 3, 5);
        assert!(!grams.is_empty());
        assert!(grams.iter().all(|g| g.chars().count() >= 3));
    }

    #[test]
    fn char_ngrams_shorter_than_min_n_yields_nothing() {
        assert!(char_ngrams("hi", 3, 5).is_empty());
    }

    #[test]
    fn hashed_feature_sum_empty_input_is_finite() {
        let features = hashed_feature_sum("", DEFAULT_NUM_BUCKETS, 2, 3, 5);
        assert!(features.values().all(|v| v.is_finite()));
    }

    #[test]
    fn hashed_feature_sum_is_deterministic() {
        let a = hashed_feature_sum("implement a parser", DEFAULT_NUM_BUCKETS, 2, 3, 5);
        let b = hashed_feature_sum("implement a parser", DEFAULT_NUM_BUCKETS, 2, 3, 5);
        assert_eq!(a, b);
    }

    #[test]
    fn hashed_feature_sum_bucket_count_changes_the_mapping() {
        // A model trained with a different bucket count must not silently load against this
        // engine: the same text maps to different buckets. This is why the loader enforces
        // the trained `num_buckets`.
        let a = hashed_feature_sum("hello world", 64, 2, 3, 5);
        let b = hashed_feature_sum("hello world", 128, 2, 3, 5);
        assert_ne!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
    }

    #[test]
    fn hashed_feature_sum_dampens_repetition_growth() {
        let short = hashed_feature_sum("debug", DEFAULT_NUM_BUCKETS, 2, 3, 5);
        let long = hashed_feature_sum(&"debug ".repeat(50), DEFAULT_NUM_BUCKETS, 2, 3, 5);
        let short_max = short.values().cloned().fold(0.0_f64, f64::max);
        let long_max = long.values().cloned().fold(0.0_f64, f64::max);
        assert!(short_max > 0.0 && long_max > 0.0);
        let ratio = long_max / short_max;
        assert!(
            ratio < 10.0,
            "expected repetition growth to be damped below the raw 50x factor, got {ratio}"
        );
    }

    #[test]
    fn sigmoid_bounds_and_midpoint() {
        assert_eq!(sigmoid(0.0), 0.5);
        assert!(sigmoid(100.0) > 0.999);
        assert!(sigmoid(-100.0) < 0.001);
    }

    #[test]
    fn softmax_is_a_distribution() {
        let mut logits = [2.0, 1.0, -1.0, 0.0];
        softmax_in_place(&mut logits);
        let sum: f64 = logits.iter().sum();
        assert!((sum - 1.0).abs() < 1e-12);
        assert!(logits.iter().all(|p| (0.0..=1.0).contains(p)));
    }

    #[test]
    fn softmax_degenerate_input_is_uniform_not_nan() {
        let mut logits = [f64::NEG_INFINITY, f64::NEG_INFINITY];
        softmax_in_place(&mut logits);
        assert_eq!(logits, [0.5, 0.5]);
    }
}
