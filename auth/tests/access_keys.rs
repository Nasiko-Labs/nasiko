//! Integration tests for access key and secret generation.

use nasiko_auth::{generate_access_key, generate_access_secret};

// ─── generate_access_key ─────────────────────────────────────────────────────

#[test]
fn access_key_starts_with_nask_prefix() {
    let key = generate_access_key();
    assert!(key.starts_with("NASK_"), "key must start with 'NASK_', got: {key}");
}

#[test]
fn access_key_total_length_is_27() {
    // "NASK_" (5) + 22 random chars = 27
    let key = generate_access_key();
    assert_eq!(key.len(), 27, "expected NASK_(5)+22 = 27 chars, got {}: {key}", key.len());
}

#[test]
fn access_key_random_portion_uses_allowed_charset() {
    const ALLOWED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let key = generate_access_key();
    let random_part = &key["NASK_".len()..];
    for ch in random_part.chars() {
        assert!(
            ALLOWED.contains(ch),
            "character '{ch}' in key '{key}' is outside the allowed charset"
        );
    }
}

#[test]
fn access_keys_are_unique() {
    let k1 = generate_access_key();
    let k2 = generate_access_key();
    assert_ne!(k1, k2, "consecutive access keys must differ");
}

#[test]
fn access_keys_are_unique_across_many_calls() {
    use std::collections::HashSet;
    let keys: HashSet<String> = (0..50).map(|_| generate_access_key()).collect();
    assert_eq!(keys.len(), 50, "all 50 generated keys should be unique");
}

// ─── generate_access_secret ──────────────────────────────────────────────────

#[test]
fn access_secret_length_is_43() {
    let secret = generate_access_secret();
    assert_eq!(secret.len(), 43, "expected 43 chars, got {}: {secret}", secret.len());
}

#[test]
fn access_secret_uses_allowed_charset() {
    const ALLOWED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let secret = generate_access_secret();
    for ch in secret.chars() {
        assert!(
            ALLOWED.contains(ch),
            "character '{ch}' in secret '{secret}' is outside the allowed charset"
        );
    }
}

#[test]
fn access_secrets_are_unique() {
    let s1 = generate_access_secret();
    let s2 = generate_access_secret();
    assert_ne!(s1, s2, "consecutive secrets must differ");
}

#[test]
fn access_secrets_are_unique_across_many_calls() {
    use std::collections::HashSet;
    let secrets: HashSet<String> = (0..50).map(|_| generate_access_secret()).collect();
    assert_eq!(secrets.len(), 50, "all 50 generated secrets should be unique");
}

#[test]
fn access_key_and_secret_differ() {
    // A key and a secret generated back-to-back must not be equal.
    // (Different lengths ensure this, but assert explicitly for clarity.)
    let key = generate_access_key();
    let secret = generate_access_secret();
    assert_ne!(key, secret);
}
