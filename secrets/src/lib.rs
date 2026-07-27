use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
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

pub struct SecretsCrypto {
    cipher: Aes256Gcm,
}

impl SecretsCrypto {
    pub fn from_key(key_base64: &str) -> Result<Self, SecretsError> {
        let key_bytes = BASE64
            .decode(key_base64)
            .map_err(|_| SecretsError::InvalidKeyLength)?;
        if key_bytes.len() != 32 {
            return Err(SecretsError::InvalidKeyLength);
        }
        let cipher =
            Aes256Gcm::new_from_slice(&key_bytes).map_err(|_| SecretsError::InvalidKeyLength)?;
        Ok(Self { cipher })
    }

    pub fn from_env() -> Result<Self, SecretsError> {
        let key = std::env::var("SECRETS_ENCRYPTION_KEY").map_err(|_| SecretsError::MissingKey)?;
        Self::from_key(&key)
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String, SecretsError> {
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| SecretsError::EncryptionFailed(e.to_string()))?;
        let mut combined = nonce_bytes.to_vec();
        combined.extend(ciphertext);
        Ok(BASE64.encode(combined))
    }

    pub fn decrypt(&self, encrypted: &str) -> Result<String, SecretsError> {
        let data = BASE64
            .decode(encrypted)
            .map_err(|e| SecretsError::DecryptionFailed(e.to_string()))?;
        if data.len() < 12 {
            return Err(SecretsError::DecryptionFailed("data too short".into()));
        }
        let (nonce_bytes, ciphertext) = data.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| SecretsError::DecryptionFailed(e.to_string()))?;
        String::from_utf8(plaintext).map_err(|e| SecretsError::DecryptionFailed(e.to_string()))
    }

    // ── Per-scope derived keys (HKDF-SHA256) ────────────────────────────────
    // Additive: `encrypt`/`decrypt` above operate on a `SecretsCrypto` built
    // from the master key directly (`from_key`/`from_env`); these constructors
    // derive a cryptographically-independent AES-256-GCM key per entity
    // (user/agent) from the same master `SECRETS_ENCRYPTION_KEY`, so a
    // compromise of one scope's secrets can't be leveraged against another's.
    // Used by the MCP gateway for per-user OAuth token / credential
    // encryption. Infallible: a missing/malformed master key is an
    // unrecoverable boot-time misconfiguration, not a per-request error —
    // callers already assume `SECRETS_ENCRYPTION_KEY` is valid (see
    // `Config::validate_secrets_key`, checked at startup).

    /// Derive a user-scoped cipher from the master key (HKDF-SHA256, info = user UUID).
    pub fn for_user(user_id: Uuid) -> Self {
        Self::derive(user_id.as_bytes())
    }

    /// Derive an agent-scoped cipher from the master key (HKDF-SHA256, info = agent UUID).
    pub fn for_agent(agent_id: Uuid) -> Self {
        Self::derive(agent_id.as_bytes())
    }

    fn derive(info: &[u8]) -> Self {
        let master = Self::load_master_key();
        let hk = Hkdf::<Sha256>::new(None, &master);
        let mut derived = [0u8; 32];
        hk.expand(info, &mut derived)
            .expect("HKDF expand failed — info too long");
        let cipher = Aes256Gcm::new_from_slice(&derived).expect("derived key is 32 bytes");
        Self { cipher }
    }

    fn load_master_key() -> Vec<u8> {
        let b64 = std::env::var("SECRETS_ENCRYPTION_KEY")
            .expect("SECRETS_ENCRYPTION_KEY must be set (base64-encoded 32-byte key)");
        let bytes = BASE64
            .decode(&b64)
            .expect("SECRETS_ENCRYPTION_KEY: invalid base64");
        assert_eq!(
            bytes.len(),
            32,
            "SECRETS_ENCRYPTION_KEY must decode to exactly 32 bytes"
        );
        bytes
    }
}
