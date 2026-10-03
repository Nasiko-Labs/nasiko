//! Shared text-feature primitives for the in-process models in `routing`.
//!
//! Two models live under `routing` and both turn a query string into a sparse feature
//! vector: the [salience gate](super::salience_classifier) model and the
//! [request classifier](super::request_model) model. The tokenization, n-gram
//! enumeration, and hashing-trick construction are identical for both, so they live here
//! once rather than being copied per model.
//!
//! **Hashing stability is a compatibility contract.** Trained weights are only meaningful
//! if `fnv1a` and `hash_sign` map the same gram to the same bucket/sign on every platform
//! and every Rust version. Both are therefore plain deterministic arithmetic (FNV-1a is a
//! fixed, published algorithm) — never `std::collections::hash_map::DefaultHasher`, whose
//! output is explicitly not stable across releases. Changing anything here invalidates
//! every committed weights asset that depends on it; retrain alongside any such change.

/// FNV-1a over raw bytes — simple, dependency-free, and deterministic across runs/platforms.
pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// The sign for a hashed gram: a second, differently-salted hash of the same gram picks
/// `+1` or `-1`. This is the standard hashing-trick construction — colliding grams with
/// opposite signs partially cancel instead of only ever adding, which bounds collision bias.
pub(super) fn hash_sign(gram: &str) -> f64 {
    if fnv1a(format!("sign:{gram}").as_bytes()) & 1 == 0 {
        1.0
    } else {
        -1.0
    }
}

/// Lowercase word tokens, splitting on anything that isn't alphanumeric. Empty tokens are
/// dropped, so runs of punctuation/whitespace just act as separators.
pub(super) fn word_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Word n-grams (`n` consecutive tokens joined by a single space) for `n` in `1..=max_n`.
pub(super) fn word_ngrams(tokens: &[String], max_n: usize) -> Vec<String> {
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

/// Character n-grams over the lowercased query, for `n` in `min_n..=max_n`. Whitespace is
/// lowercased but NOT stripped or collapsed — it stays part of the char stream and can
/// appear inside a gram. Operates on `char`s (not bytes) so multi-byte UTF-8 isn't split
/// mid-codepoint — this is what carries code-switching (e.g. `"Hola, ..."`) without a
/// network call.
pub(super) fn char_ngrams(text: &str, min_n: usize, max_n: usize) -> Vec<String> {
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

/// Numerically stable logistic function. Used by the salience model's scoring.
pub(super) fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_is_stable_for_known_inputs() {
        // Pinned FNV-1a 64-bit reference vectors — a regression guard for the hashing
        // contract: if this changes, every embedded model's buckets have silently moved.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn hash_sign_is_deterministic_and_only_ever_unit() {
        for gram in ["a", "the", "hello world", "𝒳", ""] {
            assert_eq!(hash_sign(gram), hash_sign(gram));
            assert!(hash_sign(gram) == 1.0 || hash_sign(gram) == -1.0);
        }
    }

    #[test]
    fn word_tokens_drops_punctuation_and_empty_runs() {
        assert_eq!(word_tokens("Hello,  world!!"), vec!["hello", "world"]);
        assert!(word_tokens("...").is_empty());
    }

    #[test]
    fn word_ngrams_caps_at_max_n_and_handles_short_input() {
        assert_eq!(word_ngrams(&word_tokens("a b"), 2), vec!["a", "b", "a b"]);
        assert_eq!(word_ngrams(&word_tokens("a"), 2), vec!["a"]);
        assert!(word_ngrams(&[], 2).is_empty());
    }

    #[test]
    fn char_ngrams_operates_on_chars_not_bytes() {
        // 4-char input, 3..=5 grams ⇒ two 3-grams + one 4-gram; the multi-byte char is
        // never split mid-codepoint.
        let grams = char_ngrams("a𝒳bc", 3, 5);
        assert_eq!(grams, vec!["a𝒳b", "𝒳bc", "a𝒳bc"]);
    }

    #[test]
    fn sigmoid_bounds_and_midpoint() {
        assert_eq!(sigmoid(0.0), 0.5);
        assert!(sigmoid(100.0) > 0.999);
        assert!(sigmoid(-100.0) < 0.001);
    }
}