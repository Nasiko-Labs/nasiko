//! Integration tests for AES-256-GCM encrypt/decrypt (SecretsCrypto).

use nasiko_secrets::{SecretsCrypto, SecretsError};
use uuid::Uuid;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Generate a valid base64-encoded 32-byte key and install it in the env.
fn install_valid_key() {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    let key = BASE64.encode([0x42u8; 32]);
    // SAFETY: single-threaded test; no other threads reading this var concurrently.
    unsafe { std::env::set_var("SECRETS_ENCRYPTION_KEY", &key) };
}

fn make_crypto() -> SecretsCrypto {
    install_valid_key();
    SecretsCrypto::for_system()
}

// ─── from_env ────────────────────────────────────────────────────────────────

#[test]
fn from_env_without_env_var_returns_missing_key() {
    // SAFETY: single-threaded test; no other threads reading this var concurrently.
    unsafe { std::env::remove_var("SECRETS_ENCRYPTION_KEY") };
    let result = SecretsCrypto::try_for_system();
    assert!(
        matches!(result, Err(SecretsError::MissingKey)),
        "missing env var must return MissingKey"
    );
}

#[test]
fn from_env_with_valid_key_in_env_succeeds() {
    install_valid_key();
    let result = SecretsCrypto::try_for_system();
    assert!(result.is_ok(), "valid key in env var must succeed");
}

// ─── Encrypt → Decrypt roundtrip ─────────────────────────────────────────────

#[test]
fn encrypt_decrypt_roundtrip_returns_original_plaintext() {
    let crypto = make_crypto();
    let plaintext = "hello, world!";
    let ciphertext = crypto.encrypt(plaintext);
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn encrypt_decrypt_empty_plaintext() {
    let crypto = make_crypto();
    let ciphertext = crypto.encrypt("");
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, "");
}

#[test]
fn encrypt_decrypt_unicode_plaintext() {
    let crypto = make_crypto();
    let plaintext = "emoji: 🔒 secret: パスワード";
    let ciphertext = crypto.encrypt(plaintext);
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn encrypt_decrypt_long_plaintext_1mb() {
    let crypto = make_crypto();
    // 1 MiB of 'A' characters
    let plaintext = "A".repeat(1_048_576);
    let ciphertext = crypto.encrypt(&plaintext);
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn encrypt_decrypt_multiline_plaintext() {
    let crypto = make_crypto();
    let plaintext = "line1\nline2\nline3\n";
    let ciphertext = crypto.encrypt(plaintext);
    assert_eq!(crypto.decrypt(&ciphertext).unwrap(), plaintext);
}

// ─── Nonce is random (same plaintext → different ciphertext) ─────────────────

#[test]
fn encrypting_same_plaintext_twice_produces_different_ciphertext() {
    let crypto = make_crypto();
    let plaintext = "determinism test";
    let c1 = crypto.encrypt(plaintext);
    let c2 = crypto.encrypt(plaintext);
    assert_ne!(
        c1, c2,
        "random nonce must cause different ciphertext on each call"
    );
}

#[test]
fn both_ciphertexts_still_decrypt_correctly() {
    let crypto = make_crypto();
    let plaintext = "both must decrypt";
    let c1 = crypto.encrypt(plaintext);
    let c2 = crypto.encrypt(plaintext);
    assert_eq!(crypto.decrypt(&c1).unwrap(), plaintext);
    assert_eq!(crypto.decrypt(&c2).unwrap(), plaintext);
}

// ─── Wrong scope fails decryption ────────────────────────────────────────────

#[test]
fn decrypt_with_different_scope_fails() {
    install_valid_key();
    let encryptor = SecretsCrypto::for_user(Uuid::new_v4());
    let ciphertext = encryptor.encrypt("sensitive data");

    // Different scope → different derived key
    let decryptor = SecretsCrypto::for_user(Uuid::new_v4());
    let result = decryptor.decrypt(&ciphertext);
    assert!(
        matches!(result, Err(SecretsError::DecryptionFailed(_))),
        "wrong scope must return DecryptionFailed"
    );
}

// ─── Corrupted ciphertext ─────────────────────────────────────────────────────

#[test]
fn corrupted_ciphertext_fails_decryption() {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    let crypto = make_crypto();
    let ciphertext = crypto.encrypt("secret");

    // Flip a byte in the decoded bytes, then re-encode.
    let mut raw = BASE64.decode(&ciphertext).unwrap();
    let last = raw.len() - 1;
    raw[last] ^= 0xFF; // flip all bits of last byte
    let corrupted = BASE64.encode(raw);

    let result = crypto.decrypt(&corrupted);
    assert!(
        matches!(result, Err(SecretsError::DecryptionFailed(_))),
        "corrupted ciphertext must return DecryptionFailed"
    );
}

#[test]
fn truncated_ciphertext_fails_decryption() {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    let crypto = make_crypto();
    let ciphertext = crypto.encrypt("secret");

    // Keep only the first 5 bytes (less than the 12-byte nonce minimum)
    let mut raw = BASE64.decode(&ciphertext).unwrap();
    raw.truncate(5);
    let truncated = BASE64.encode(raw);

    let result = crypto.decrypt(&truncated);
    assert!(
        matches!(result, Err(SecretsError::DecryptionFailed(_))),
        "data shorter than nonce must return DecryptionFailed"
    );
}

#[test]
fn garbage_string_fails_decryption() {
    let crypto = make_crypto();
    let result = crypto.decrypt("this-is-not-valid-base64-or-ciphertext!!!");
    assert!(result.is_err(), "garbage input must fail decryption");
}

#[test]
fn empty_string_fails_decryption() {
    let crypto = make_crypto();
    let result = crypto.decrypt("");
    assert!(
        result.is_err(),
        "empty ciphertext must fail decryption (too short)"
    );
}
