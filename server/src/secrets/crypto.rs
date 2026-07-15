//! AES-256-GCM encryption helpers for platform secrets.
//!
//! ## Key model
//!
//! A single **master key** (`SECRETS_ENCRYPTION_KEY`, base64-encoded 32 bytes) is
//! the platform root of trust.  From it, per-scope derived keys are produced with
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
//! The nonce is freshly sampled from the OS CSPRNG on every call, so encrypting
//! the same plaintext twice produces different ciphertexts (IND-CPA security).

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng},
};
use aes_gcm::aead::rand_core::RngCore;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use hkdf::Hkdf;
use sha2::Sha256;
use uuid::Uuid;

pub struct SecretsCrypto {
    cipher: Aes256Gcm,
}

impl SecretsCrypto {
    // -------------------------------------------------------------------------
    // Constructors
    // -------------------------------------------------------------------------

    /// Derive an agent-scoped key from the master key.
    ///
    /// Use this for every read/write to `agents.secrets_env`.
    /// Each agent gets a cryptographically independent AES-256-GCM key so that
    /// a compromise of one agent's secrets cannot be leveraged to decrypt another's.
    pub fn for_agent(agent_id: Uuid) -> Self {
        Self::derive(agent_id.as_bytes())
    }

    /// Derive a user-scoped key from the master key.
    ///
    /// Use this for every read/write to `user_secrets.encrypted_value`.
    pub fn for_user(user_id: Uuid) -> Self {
        Self::derive(user_id.as_bytes())
    }

    /// Derive the key for singleton platform-wide settings (e.g.
    /// `settings.oidc_client_secret_encrypted`) — there's exactly one such
    /// row, so the scope is a fixed label rather than a per-entity UUID.
    pub fn for_platform_settings() -> Self {
        Self::derive(b"platform-settings-v1")
    }

    /// Load the master key directly (no key derivation).
    ///
    /// Kept for backwards-compatibility with any call-site that encrypts data
    /// that is not scoped to a specific agent or user.  Prefer `for_agent` /
    /// `for_user` for all new code.
    #[deprecated(note = "use for_agent() or for_user() to get a scope-derived key")]
    pub fn from_env() -> Self {
        let cipher = Self::master_cipher();
        Self { cipher }
    }

    // -------------------------------------------------------------------------
    // Encrypt / decrypt
    // -------------------------------------------------------------------------

    /// Encrypt `plaintext` → `base64(nonce || ciphertext)`.
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
    pub fn decrypt(&self, encrypted: &str) -> Result<String, &'static str> {
        let combined = BASE64.decode(encrypted).map_err(|_| "invalid base64")?;
        if combined.len() < 12 {
            return Err("ciphertext too short");
        }
        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| "decryption failed")?;

        String::from_utf8(plaintext).map_err(|_| "invalid utf-8")
    }

    // -------------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------------

    /// Derive a 32-byte AES-256-GCM key with HKDF-SHA256.
    /// `info` is the scope context (e.g. UUID bytes).
    fn derive(info: &[u8]) -> Self {
        let master = Self::load_master_key();
        let hk = Hkdf::<Sha256>::new(None, &master);
        let mut derived = [0u8; 32];
        hk.expand(info, &mut derived)
            .expect("HKDF expand failed — info too long");
        let cipher = Aes256Gcm::new_from_slice(&derived).unwrap();
        Self { cipher }
    }

    fn master_cipher() -> Aes256Gcm {
        let key = Self::load_master_key();
        Aes256Gcm::new_from_slice(&key).unwrap()
    }

    fn load_master_key() -> Vec<u8> {
        let b64 = std::env::var("SECRETS_ENCRYPTION_KEY")
            .expect("SECRETS_ENCRYPTION_KEY must be set (base64-encoded 32-byte key)");
        let bytes = BASE64
            .decode(&b64)
            .expect("SECRETS_ENCRYPTION_KEY: invalid base64");
        assert_eq!(bytes.len(), 32, "SECRETS_ENCRYPTION_KEY must be exactly 32 bytes");
        bytes
    }
}
