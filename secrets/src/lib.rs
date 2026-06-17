use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

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
        let key_bytes = BASE64.decode(key_base64).map_err(|_| SecretsError::InvalidKeyLength)?;
        if key_bytes.len() != 32 {
            return Err(SecretsError::InvalidKeyLength);
        }
        let cipher = Aes256Gcm::new_from_slice(&key_bytes)
            .map_err(|_| SecretsError::InvalidKeyLength)?;
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
}
