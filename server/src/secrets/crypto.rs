use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng},
};
use aes_gcm::aead::rand_core::RngCore;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

pub struct SecretsCrypto {
    cipher: Aes256Gcm,
}

impl SecretsCrypto {
    pub fn from_env() -> Self {
        let key_b64 = std::env::var("SECRETS_ENCRYPTION_KEY")
            .expect("SECRETS_ENCRYPTION_KEY must be set (base64-encoded 32-byte key)");
        let key_bytes = BASE64.decode(&key_b64).expect("invalid base64 for SECRETS_ENCRYPTION_KEY");
        assert_eq!(key_bytes.len(), 32, "SECRETS_ENCRYPTION_KEY must be 32 bytes");
        let cipher = Aes256Gcm::new_from_slice(&key_bytes).unwrap();
        Self { cipher }
    }

    /// Encrypt plaintext → base64(nonce || ciphertext)
    pub fn encrypt(&self, plaintext: &str) -> String {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .expect("encryption failed");

        let mut combined = nonce_bytes.to_vec();
        combined.extend_from_slice(&ciphertext);
        BASE64.encode(&combined)
    }

    /// Decrypt base64(nonce || ciphertext) → plaintext
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
}
