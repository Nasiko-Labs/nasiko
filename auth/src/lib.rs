pub mod headers;
pub mod jwt;
pub mod service;
#[cfg(test)]
mod tests;

pub use headers::*;
pub use service::AuthServiceImpl;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Organizational role — determines what a user can do.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Member,
    TeamMember,
    TeamLead,
    DepartmentManager,
    Admin,
}

/// Identity extracted after authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_superuser: bool,
    #[serde(default)]
    pub role: Option<Role>,
}

/// Consolidated auth trait — replaces AuthProvider, Authorizer, UserAuthService, TokenService.
#[async_trait]
pub trait AuthService: Send + Sync + 'static {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError>;
    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError>;
    async fn authenticate(&self, username: &str, password: &str) -> Result<LoginResult, AuthError>;
    async fn bootstrap_admin(&self, username: &str, password: &str) -> Result<(), AuthError>;
    async fn issue_agent_token(&self, agent_id: &str) -> Result<String, AuthError>;
    async fn upsert_oauth_user(&self, provider: &str, provider_id: &str, username: &str) -> Result<LoginResult, AuthError>;
    async fn lookup_user(&self, user_id: &str) -> Result<Identity, AuthError>;
    async fn revoke_tokens_for_user(&self, user_id: &str) -> Result<u64, AuthError>;
    async fn revoke_all_tokens(&self) -> Result<u64, AuthError>;
    async fn revoke_tokens_for_agent(&self, agent_id: &str) -> Result<u64, AuthError>;
    async fn can_access_agent(&self, identity: &Identity, agent_id: &str) -> bool;
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing token")]
    MissingToken,
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("expired token")]
    Expired,
    #[error("token revoked")]
    Revoked,
}

// ─── Password helpers ────────────────────────────────────────────────────────

/// Hash a password with bcrypt cost 12.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    bcrypt::hash(password, 12).map_err(|e| AuthError::InvalidToken(e.to_string()))
}

/// Verify a bcrypt password.
pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

/// Hash a password off the async executor (bcrypt is ~50-100ms CPU).
pub async fn hash_password_async(password: &str) -> Result<String, AuthError> {
    let pw = password.to_owned();
    tokio::task::spawn_blocking(move || bcrypt::hash(&pw, 12))
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?
        .map_err(|e| AuthError::InvalidToken(e.to_string()))
}

/// Verify a bcrypt password off the async executor.
pub async fn verify_password_async(password: &str, hash: &str) -> bool {
    let pw = password.to_owned();
    let h = hash.to_owned();
    tokio::task::spawn_blocking(move || bcrypt::verify(&pw, &h).unwrap_or(false))
        .await
        .unwrap_or(false)
}

const ACCESS_CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn random_charset_string(len: usize) -> String {
    use rand::TryRngCore;
    use rand::rngs::OsRng;
    let mut bytes = vec![0u8; len];
    OsRng.try_fill_bytes(&mut bytes).expect("OS CSPRNG unavailable");
    bytes.iter()
        .map(|&b| ACCESS_CHARSET[b as usize % ACCESS_CHARSET.len()] as char)
        .collect()
}

/// Generate a NASK_-prefixed access key using the OS CSPRNG.
pub fn generate_access_key() -> String {
    format!("NASK_{}", random_charset_string(22))
}

/// Generate a random access secret (URL-safe, 43 chars) using the OS CSPRNG.
pub fn generate_access_secret() -> String {
    random_charset_string(43)
}

// ─── User auth service types ──────────────────────────────────────────────────

/// Result returned by login/initialize-admin/oauth operations.
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub token: String,
    pub user_id: String,
    pub username: String,
    pub is_superuser: bool,
    pub role: String,
    pub expires_in: u64,
    pub access_key: Option<String>,
    pub access_secret: Option<String>,
}

// ─── Gateway-only JWT auth ───────────────────────────────────────────────────

pub struct SimpleJwtAuth {
    pub secret: String,
    pub expiry_secs: u64,
}

impl SimpleJwtAuth {
    pub fn from_env() -> Self {
        Self {
            secret: std::env::var("JWT_SECRET").expect("JWT_SECRET required"),
            expiry_secs: jwt::DEFAULT_EXPIRY_SECS,
        }
    }
}

#[async_trait]
impl AuthService for SimpleJwtAuth {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError> {
        jwt::decode_jwt(&self.secret, token)
    }

    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError> {
        jwt::encode_jwt(&self.secret, self.expiry_secs, identity)
    }

    async fn authenticate(&self, _username: &str, _password: &str) -> Result<LoginResult, AuthError> {
        Err(AuthError::InvalidToken("not supported by gateway auth".into()))
    }

    async fn bootstrap_admin(&self, _username: &str, _password: &str) -> Result<(), AuthError> {
        Ok(())
    }

    async fn issue_agent_token(&self, _agent_id: &str) -> Result<String, AuthError> {
        Err(AuthError::InvalidToken("not supported by gateway auth".into()))
    }

    async fn upsert_oauth_user(&self, _provider: &str, _provider_id: &str, _username: &str) -> Result<LoginResult, AuthError> {
        Err(AuthError::InvalidToken("not supported by gateway auth".into()))
    }

    async fn lookup_user(&self, _user_id: &str) -> Result<Identity, AuthError> {
        Err(AuthError::InvalidToken("not supported by gateway auth".into()))
    }

    async fn revoke_tokens_for_user(&self, _user_id: &str) -> Result<u64, AuthError> {
        Ok(0)
    }

    async fn revoke_all_tokens(&self) -> Result<u64, AuthError> {
        Ok(0)
    }

    async fn revoke_tokens_for_agent(&self, _agent_id: &str) -> Result<u64, AuthError> {
        Ok(0)
    }

    async fn can_access_agent(&self, _identity: &Identity, _agent_id: &str) -> bool {
        true
    }
}
