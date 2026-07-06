//! Integration tests for bcrypt password hashing helpers.

use nasiko_auth::{hash_password, hash_password_async, verify_password};

// ─── Synchronous helpers ──────────────────────────────────────────────────────

#[test]
fn hash_password_succeeds() {
    hash_password("hunter2").expect("hash_password must not fail for a valid password");
}

#[test]
fn verify_password_correct_password_returns_true() {
    let hash = hash_password("correct-horse-battery-staple").unwrap();
    assert!(verify_password("correct-horse-battery-staple", &hash));
}

#[test]
fn verify_password_wrong_password_returns_false() {
    let hash = hash_password("correct").unwrap();
    assert!(!verify_password("wrong", &hash));
}

#[test]
fn verify_password_empty_password_fails_against_nonempty_hash() {
    let hash = hash_password("nonempty").unwrap();
    assert!(!verify_password("", &hash));
}

#[test]
fn hash_is_not_plaintext() {
    let pw = "mysecretpassword";
    let hash = hash_password(pw).unwrap();
    assert_ne!(hash, pw, "stored hash must not equal the plaintext password");
}

#[test]
fn hash_starts_with_bcrypt_prefix() {
    let hash = hash_password("any_password").unwrap();
    // All bcrypt hashes start with $2b$ (or $2a$/$2y$ for older variants).
    assert!(
        hash.starts_with("$2b$") || hash.starts_with("$2a$") || hash.starts_with("$2y$"),
        "expected a bcrypt hash prefix, got: {hash}"
    );
}

#[test]
fn two_hashes_of_same_password_differ() {
    // bcrypt uses a random salt — identical passwords must produce different hashes.
    let h1 = hash_password("samepassword").unwrap();
    let h2 = hash_password("samepassword").unwrap();
    assert_ne!(h1, h2, "bcrypt hashes of the same password must differ due to random salt");
}

#[test]
fn both_hashes_still_verify_correctly() {
    let h1 = hash_password("samepassword").unwrap();
    let h2 = hash_password("samepassword").unwrap();
    assert!(verify_password("samepassword", &h1));
    assert!(verify_password("samepassword", &h2));
}

#[test]
fn verify_password_with_garbage_hash_returns_false() {
    assert!(!verify_password("password", "not-a-bcrypt-hash"));
}

// ─── Async variant ────────────────────────────────────────────────────────────

#[tokio::test]
async fn hash_password_async_succeeds() {
    hash_password_async("asyncpassword")
        .await
        .expect("hash_password_async must not fail");
}

#[tokio::test]
async fn hash_password_async_result_verifies_synchronously() {
    let hash = hash_password_async("myasyncpw").await.unwrap();
    assert!(verify_password("myasyncpw", &hash));
}

#[tokio::test]
async fn hash_password_async_wrong_password_does_not_verify() {
    let hash = hash_password_async("correctpw").await.unwrap();
    assert!(!verify_password("wrongpw", &hash));
}

#[tokio::test]
async fn hash_password_async_result_is_not_plaintext() {
    let pw = "asyncplaintext";
    let hash = hash_password_async(pw).await.unwrap();
    assert_ne!(hash, pw);
}
