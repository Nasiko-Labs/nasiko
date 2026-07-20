//! Integration tests for AES-256-GCM encrypt/decrypt (SecretsCrypto).

use nasiko_secrets::{SecretsCrypto, SecretsError};

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Generate a valid base64-encoded 32-byte key.
fn valid_key_b64() -> String {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    BASE64.encode([0x42u8; 32])
}

fn make_crypto() -> SecretsCrypto {
    SecretsCrypto::from_key(&valid_key_b64()).expect("valid key must succeed")
}

// ─── from_key construction ────────────────────────────────────────────────────

#[test]
fn from_key_with_valid_32_byte_key_succeeds() {
    make_crypto(); // panics on error
}

#[test]
fn from_key_with_wrong_length_fails() {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    // 16 bytes — too short for AES-256
    let short = BASE64.encode([0u8; 16]);
    let result = SecretsCrypto::from_key(&short);
    assert!(
        matches!(result, Err(SecretsError::InvalidKeyLength)),
        "16-byte key must return InvalidKeyLength"
    );
}

#[test]
fn from_key_with_33_byte_key_fails() {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    let too_long = BASE64.encode([0u8; 33]);
    let result = SecretsCrypto::from_key(&too_long);
    assert!(
        matches!(result, Err(SecretsError::InvalidKeyLength)),
        "33-byte key must return InvalidKeyLength"
    );
}

#[test]
fn from_key_with_empty_string_fails() {
    let result = SecretsCrypto::from_key("");
    // empty base64 decodes to 0 bytes → InvalidKeyLength
    assert!(
        matches!(result, Err(SecretsError::InvalidKeyLength)),
        "empty key must return InvalidKeyLength"
    );
}

#[test]
fn from_key_with_invalid_base64_fails() {
    let result = SecretsCrypto::from_key("!!!not-base64!!!");
    assert!(
        matches!(result, Err(SecretsError::InvalidKeyLength)),
        "invalid base64 must return InvalidKeyLength"
    );
}

// ─── from_env ────────────────────────────────────────────────────────────────

#[test]
fn from_env_without_env_var_returns_missing_key() {
    // Make sure the env var is absent; use a unique name to avoid test pollution.
    // SAFETY: single-threaded test; no other threads reading this var concurrently.
    unsafe { std::env::remove_var("SECRETS_ENCRYPTION_KEY") };
    let result = SecretsCrypto::from_env();
    assert!(
        matches!(result, Err(SecretsError::MissingKey)),
        "missing env var must return MissingKey"
    );
}

#[test]
fn from_env_with_valid_key_in_env_succeeds() {
    // Set the variable and then clean up regardless of outcome.
    // SAFETY: single-threaded test; no other threads reading this var concurrently.
    unsafe { std::env::set_var("SECRETS_ENCRYPTION_KEY", valid_key_b64()) };
    let result = SecretsCrypto::from_env();
    unsafe { std::env::remove_var("SECRETS_ENCRYPTION_KEY") };
    assert!(result.is_ok(), "valid key in env var must succeed");
}

// ─── Encrypt → Decrypt roundtrip ─────────────────────────────────────────────

#[test]
fn encrypt_decrypt_roundtrip_returns_original_plaintext() {
    let crypto = make_crypto();
    let plaintext = "hello, world!";
    let ciphertext = crypto.encrypt(plaintext).unwrap();
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn encrypt_decrypt_empty_plaintext() {
    let crypto = make_crypto();
    let ciphertext = crypto.encrypt("").unwrap();
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, "");
}

#[test]
fn encrypt_decrypt_unicode_plaintext() {
    let crypto = make_crypto();
    let plaintext = "emoji: 🔒 secret: パスワード";
    let ciphertext = crypto.encrypt(plaintext).unwrap();
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn encrypt_decrypt_long_plaintext_1mb() {
    let crypto = make_crypto();
    // 1 MiB of 'A' characters
    let plaintext = "A".repeat(1_048_576);
    let ciphertext = crypto.encrypt(&plaintext).unwrap();
    let decrypted = crypto.decrypt(&ciphertext).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn encrypt_decrypt_multiline_plaintext() {
    let crypto = make_crypto();
    let plaintext = "line1\nline2\nline3\n";
    let ciphertext = crypto.encrypt(plaintext).unwrap();
    assert_eq!(crypto.decrypt(&ciphertext).unwrap(), plaintext);
}

// ─── Nonce is random (same plaintext → different ciphertext) ─────────────────

#[test]
fn encrypting_same_plaintext_twice_produces_different_ciphertext() {
    let crypto = make_crypto();
    let plaintext = "determinism test";
    let c1 = crypto.encrypt(plaintext).unwrap();
    let c2 = crypto.encrypt(plaintext).unwrap();
    assert_ne!(
        c1, c2,
        "random nonce must cause different ciphertext on each call"
    );
}

#[test]
fn both_ciphertexts_still_decrypt_correctly() {
    let crypto = make_crypto();
    let plaintext = "both must decrypt";
    let c1 = crypto.encrypt(plaintext).unwrap();
    let c2 = crypto.encrypt(plaintext).unwrap();
    assert_eq!(crypto.decrypt(&c1).unwrap(), plaintext);
    assert_eq!(crypto.decrypt(&c2).unwrap(), plaintext);
}

// ─── Wrong key fails decryption ───────────────────────────────────────────────

#[test]
fn decrypt_with_wrong_key_fails() {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    let encryptor = make_crypto();
    let ciphertext = encryptor.encrypt("sensitive data").unwrap();

    // Different key bytes
    let wrong_key = BASE64.encode([0xFFu8; 32]);
    let decryptor = SecretsCrypto::from_key(&wrong_key).unwrap();
    let result = decryptor.decrypt(&ciphertext);
    assert!(
        matches!(result, Err(SecretsError::DecryptionFailed(_))),
        "wrong key must return DecryptionFailed"
    );
}

// ─── Corrupted ciphertext ─────────────────────────────────────────────────────

#[test]
fn corrupted_ciphertext_fails_decryption() {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    let crypto = make_crypto();
    let ciphertext = crypto.encrypt("secret").unwrap();

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
    let ciphertext = crypto.encrypt("secret").unwrap();

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
