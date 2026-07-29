//! AES-256-GCM encryption for platform secrets, with per-scope key derivation.
//!
//! ## Key model
//!
//! A single **master key** (`SECRETS_ENCRYPTION_KEY`, base64-encoded 32 bytes) is
//! the platform root of trust. From it, per-scope derived keys are produced with
//! HKDF-SHA256 (RFC 5869):
//!
//! ```text
//! derived_key = HKDF-SHA256(
//!     ikm  = master_key,
//!     salt = (none — master key is already random and high-entropy),
//!     info = scope_bytes,      // e.g. agent UUID bytes or user UUID bytes
//! )
//! ```
//!
//! **Why HKDF instead of reusing the master key directly?**
//! - A ciphertext oracle against one agent's secrets cannot be used to attack
//!   another agent's secrets — each scope gets a cryptographically independent key.
//! - No extra database column or KMS call: the derived key is computed on the fly
//!   from the master key + entity UUID, both already available at call sites.
//! - The master key never leaves memory; only derived keys reach the AES-GCM cipher.
//!
//! ## Wire format
//!
//! `encrypt()` outputs `base64(12-byte-random-nonce || AES-256-GCM-ciphertext)`.
//! The nonce is freshly sampled from the OS CSPRNG on every call, so encrypting the
//! same plaintext twice produces different ciphertexts (IND-CPA security).
//!
//! ## Constructors
//!
//! Only the scope-derived constructors are public: [`SecretsCrypto::for_user`] /
//! [`SecretsCrypto::for_agent`] (panic on a missing/invalid master key, for startup
//! paths) and the fallible [`SecretsCrypto::try_for_user`] / [`try_for_agent`] for
//! request-path callers that must fail-closed with a proper error rather than panic.
//! There is intentionally **no** master-key-only constructor: such ciphertext would
//! be unreadable by every per-scope reader.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use hkdf::Hkdf;
use sha2::Sha256;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("encryption key not configured")]
    MissingKey,
    #[error("invalid key length (expected 32 bytes)")]
    InvalidKeyLength,
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("decryption failed: {0}")]
    DecryptionFailed(String),
}

/// HKDF `info` label for the platform/system scope. Distinct from the 16-byte UUID
/// scopes used by `for_user`/`for_agent`, so the derived key is independent from them.
const SYSTEM_SCOPE: &[u8] = b"nasiko-system";

/// HKDF `info` label for singleton platform-wide settings secrets. Distinct from
/// [`SYSTEM_SCOPE`] and the UUID scopes; must not change or stored ciphertext becomes
/// undecryptable.
const PLATFORM_SETTINGS_SCOPE: &[u8] = b"platform-settings-v1";

pub struct SecretsCrypto {
    cipher: Aes256Gcm,
}

impl SecretsCrypto {
    // ── Constructors (scope-derived) ────────────────────────────────────────

    /// Derive an agent-scoped key from the master key. Use for every read/write to
    /// agent-scoped secrets. Panics if the master key is missing/invalid (startup
    /// paths); use [`try_for_agent`](Self::try_for_agent) on the request path.
    pub fn for_agent(agent_id: Uuid) -> Self {
        Self::derive(agent_id.as_bytes())
    }

    /// Derive a user-scoped key from the master key. Use for every read/write to
    /// `user_secrets.encrypted_value`. Panics if the master key is missing/invalid.
    pub fn for_user(user_id: Uuid) -> Self {
        Self::derive(user_id.as_bytes())
    }

    /// Fallible [`for_agent`](Self::for_agent) — surfaces a missing/invalid master
    /// key as [`SecretsError`] instead of panicking. For request-path callers.
    pub fn try_for_agent(agent_id: Uuid) -> Result<Self, SecretsError> {
        Self::try_derive(agent_id.as_bytes())
    }

    /// Fallible [`for_user`](Self::for_user) — surfaces a missing/invalid master key
    /// as [`SecretsError`] instead of panicking. For request-path callers.
    pub fn try_for_user(user_id: Uuid) -> Result<Self, SecretsError> {
        Self::try_derive(user_id.as_bytes())
    }

    /// Derive a platform/system-scoped key from the master key. Use for secrets that
    /// belong to the platform itself rather than a specific user or agent (e.g. encrypted
    /// infra provisioning outputs). The scope label is distinct from any UUID scope, so
    /// this key is cryptographically independent from every user/agent key. Panics if the
    /// master key is missing/invalid; use [`try_for_system`](Self::try_for_system) on the
    /// request path.
    pub fn for_system() -> Self {
        Self::derive(SYSTEM_SCOPE)
    }

    /// Fallible [`for_system`](Self::for_system) — surfaces a missing/invalid master key
    /// as [`SecretsError`] instead of panicking. For request-path callers.
    pub fn try_for_system() -> Result<Self, SecretsError> {
        Self::try_derive(SYSTEM_SCOPE)
    }

    /// Derive the key for singleton platform-wide settings (e.g.
    /// `settings.oidc_client_secret_encrypted`) — there's exactly one such row, so the
    /// scope is a fixed label rather than a per-entity UUID. The label is distinct from
    /// [`SYSTEM_SCOPE`], keeping this key independent from `for_system`. Panics if the
    /// master key is missing/invalid.
    pub fn for_platform_settings() -> Self {
        Self::derive(PLATFORM_SETTINGS_SCOPE)
    }

    // ── Encrypt / decrypt ───────────────────────────────────────────────────

    /// Encrypt `plaintext` → `base64(nonce || ciphertext)`. Infallible in practice
    /// (AES-GCM only fails on absurd input sizes).
    pub fn encrypt(&self, plaintext: &str) -> String {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .expect("AES-256-GCM encryption failed");

        let mut combined = nonce_bytes.to_vec();
        combined.extend_from_slice(&ciphertext);
        BASE64.encode(&combined)
    }

    /// Decrypt `base64(nonce || ciphertext)` → plaintext.
    pub fn decrypt(&self, encrypted: &str) -> Result<String, SecretsError> {
        let combined = BASE64
            .decode(encrypted)
            .map_err(|e| SecretsError::DecryptionFailed(format!("invalid base64: {e}")))?;
        if combined.len() < 12 {
            return Err(SecretsError::DecryptionFailed(
                "ciphertext too short".into(),
            ));
        }
        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretsError::DecryptionFailed("decryption failed".into()))?;

        String::from_utf8(plaintext)
            .map_err(|e| SecretsError::DecryptionFailed(format!("invalid utf-8: {e}")))
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn derive(info: &[u8]) -> Self {
        Self::derive_with_master(info, &load_master_key())
    }

    fn try_derive(info: &[u8]) -> Result<Self, SecretsError> {
        Ok(Self::derive_with_master(info, &try_load_master_key()?))
    }

    /// Derive a 32-byte AES-256-GCM key with HKDF-SHA256 (`salt = None`, `info` =
    /// scope bytes). The derivation is the stored-ciphertext contract — do not change.
    fn derive_with_master(info: &[u8], master: &[u8]) -> Self {
        let hk = Hkdf::<Sha256>::new(None, master);
        let mut derived = [0u8; 32];
        hk.expand(info, &mut derived)
            .expect("HKDF expand failed — info too long");
        let cipher = Aes256Gcm::new_from_slice(&derived).expect("derived key is 32 bytes");
        Self { cipher }
    }

    /// Test-only deterministic constructor — derive a user-scoped cipher from an
    /// explicit master key, so tests need not mutate the global env.
    #[cfg(test)]
    fn for_user_with_master(user_id: Uuid, master: &[u8]) -> Self {
        Self::derive_with_master(user_id.as_bytes(), master)
    }
}

fn try_load_master_key() -> Result<Vec<u8>, SecretsError> {
    let b64 = std::env::var("SECRETS_ENCRYPTION_KEY").map_err(|_| SecretsError::MissingKey)?;
    let bytes = BASE64
        .decode(&b64)
        .map_err(|_| SecretsError::InvalidKeyLength)?;
    if bytes.len() != 32 {
        return Err(SecretsError::InvalidKeyLength);
    }
    Ok(bytes)
}

fn load_master_key() -> Vec<u8> {
    try_load_master_key().expect("SECRETS_ENCRYPTION_KEY must be set (base64-encoded 32-byte key)")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: [u8; 32] = [1u8; 32];
    const VECTOR_UUID: &str = "11111111-1111-1111-1111-111111111111";
    const VECTOR_PLAINTEXT: &str = "sk-ant-test-PLAINTEXT-12345";
    // Ciphertext produced by the PRE-consolidation server `for_user` (the per-scope
    // crypto from commit 4dd0e37, before it was moved here), with `key = [1u8;32]`,
    // `user_id = 1111…1111`. Frozen to guarantee the consolidated crate still
    // decrypts secrets already written to the database. If this test ever fails, the
    // HKDF derivation or wire format drifted — existing rows would be unreadable.
    const VECTOR: &str =
        "ibgQSoOAiQea/DV1+9KFIvsCBCVET2mt76URYaUDi9BTFF92NzxlHEBDFhtVoWdbQCK7nzlgGQ==";

    #[test]
    fn decrypts_pre_consolidation_for_user_ciphertext() {
        let uuid = Uuid::parse_str(VECTOR_UUID).unwrap();
        let plaintext = SecretsCrypto::for_user_with_master(uuid, &MASTER)
            .decrypt(VECTOR)
            .expect("byte-compat: consolidated crate must decrypt old server ciphertext");
        assert_eq!(plaintext, VECTOR_PLAINTEXT);
    }

    #[test]
    fn round_trips_per_scope() {
        let uuid = Uuid::parse_str(VECTOR_UUID).unwrap();
        let crypto = SecretsCrypto::for_user_with_master(uuid, &MASTER);
        let ct = crypto.encrypt("hello-secret");
        assert_eq!(crypto.decrypt(&ct).unwrap(), "hello-secret");
    }

    #[test]
    fn different_scope_cannot_decrypt() {
        let a = Uuid::parse_str(VECTOR_UUID).unwrap();
        let b = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let ct = SecretsCrypto::for_user_with_master(a, &MASTER).encrypt("x");
        // Same master, different scope ⇒ independent key ⇒ GCM auth fails.
        assert!(
            SecretsCrypto::for_user_with_master(b, &MASTER)
                .decrypt(&ct)
                .is_err()
        );
    }
}
