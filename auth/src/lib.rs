pub mod headers;
pub mod jwt;
pub mod service;

pub use headers::*;
pub use service::UserAuthServiceImpl;

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
    pub team_id: Option<String>,
    #[serde(default)]
    pub department_id: Option<String>,
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub exp: u64,
    #[serde(default)]
    pub iat: u64,
}

/// Validate a token and extract identity.
#[async_trait]
pub trait AuthProvider: Send + Sync + 'static {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError>;

    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError> {
        let _ = identity;
        Err(AuthError::InvalidToken("token issuance not supported by this provider".into()))
    }
}

/// What can this identity do?
#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn can_access_agent(&self, identity: &Identity, agent_id: &str) -> bool;
    async fn can_discover_agent(&self, identity: &Identity, agent_id: &str) -> bool;
    fn can_deploy(&self, identity: &Identity) -> bool;
    fn can_manage_secrets(&self, identity: &Identity) -> bool;
    fn can_manage_users(&self, identity: &Identity) -> bool;
    fn can_manage_pool(&self, identity: &Identity) -> bool;
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

/// Generate a NASK_-prefixed access key using URL-safe random bytes.
pub fn generate_access_key() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::rng();
    let suffix: String = (0..22)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect();
    format!("NASK_{}", suffix)
}

/// Generate a random access secret (URL-safe, 32 bytes → 43 chars).
pub fn generate_access_secret() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::rng();
    (0..43)
        .map(|_| CHARSET[rng.random_range(0..CHARSET.len())] as char)
        .collect()
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
    pub team_id: Option<String>,
    pub department_id: Option<String>,
    pub expires_in: u64,
    pub access_key: Option<String>,
    pub access_secret: Option<String>,
}

/// High-level user authentication operations (login, admin init, oauth, agent tokens).
/// Implementations hold the DB pool and use an AuthProvider for token issuance.
#[async_trait]
pub trait UserAuthService: Send + Sync + 'static {
    async fn authenticate(&self, access_key: &str, access_secret: &str) -> Result<LoginResult, AuthError>;
    async fn initialize_admin(&self, username: &str, email: &str) -> Result<LoginResult, AuthError>;
    async fn issue_agent_token(&self, agent_id: &str) -> Result<String, AuthError>;
    async fn upsert_oauth_user(&self, provider: &str, provider_id: &str, username: &str) -> Result<LoginResult, AuthError>;
    async fn lookup_user(&self, user_id: &str) -> Result<Identity, AuthError>;
}

/// Token revocation operations.
#[async_trait]
pub trait TokenService: Send + Sync + 'static {
    async fn revoke_for_user(&self, user_id: &str) -> Result<u64, AuthError>;
    async fn revoke_all(&self) -> Result<u64, AuthError>;
}

/// No-op implementation — used in dev/passthrough mode.
pub struct NoopUserAuthService;

#[async_trait]
impl UserAuthService for NoopUserAuthService {
    async fn authenticate(&self, _access_key: &str, _access_secret: &str) -> Result<LoginResult, AuthError> {
        Err(AuthError::InvalidToken("user auth not configured".into()))
    }
    async fn initialize_admin(&self, _username: &str, _email: &str) -> Result<LoginResult, AuthError> {
        Err(AuthError::InvalidToken("user auth not configured".into()))
    }
    async fn issue_agent_token(&self, _agent_id: &str) -> Result<String, AuthError> {
        Err(AuthError::InvalidToken("user auth not configured".into()))
    }
    async fn upsert_oauth_user(&self, _provider: &str, _provider_id: &str, _username: &str) -> Result<LoginResult, AuthError> {
        Err(AuthError::InvalidToken("user auth not configured".into()))
    }
    async fn lookup_user(&self, _user_id: &str) -> Result<Identity, AuthError> {
        Err(AuthError::InvalidToken("user auth not configured".into()))
    }
}

pub struct NoopTokenService;

#[async_trait]
impl TokenService for NoopTokenService {
    async fn revoke_for_user(&self, _user_id: &str) -> Result<u64, AuthError> { Ok(0) }
    async fn revoke_all(&self) -> Result<u64, AuthError> { Ok(0) }
}

// ─── OSS default implementations ─────────────────────────────────────────────

pub struct SingleUserAuth;

#[async_trait]
impl AuthProvider for SingleUserAuth {
    async fn validate_token(&self, _token: &str) -> Result<Identity, AuthError> {
        Ok(Identity {
            user_id: "00000000-0000-0000-0000-000000000000".to_string(),
            sub: "00000000-0000-0000-0000-000000000000".to_string(),
            exp: u64::MAX,
            iat: 0,
            username: "admin".to_string(),
            is_superuser: true,
            team_id: None,
            department_id: None,
            role: Some(Role::Admin),
        })
    }
}

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
impl AuthProvider for SimpleJwtAuth {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError> {
        jwt::decode_jwt(&self.secret, token)
    }

    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError> {
        jwt::encode_jwt(&self.secret, self.expiry_secs, identity)
    }
}

pub struct NoopAuthorizer;

#[async_trait]
impl Authorizer for NoopAuthorizer {
    async fn can_access_agent(&self, _identity: &Identity, _agent_id: &str) -> bool { true }
    async fn can_discover_agent(&self, _identity: &Identity, _agent_id: &str) -> bool { true }
    fn can_deploy(&self, _identity: &Identity) -> bool { true }
    fn can_manage_secrets(&self, _identity: &Identity) -> bool { true }
    fn can_manage_users(&self, _identity: &Identity) -> bool { true }
    fn can_manage_pool(&self, _identity: &Identity) -> bool { true }
}

// ─── Backwards-compatible aliases ────────────────────────────────────────────

pub type Claims = Identity;

impl Identity {
    pub fn sub(&self) -> &str { &self.user_id }
}

pub trait AclChecker: Authorizer {}
impl<T: Authorizer> AclChecker for T {}
pub type NoopAcl = NoopAuthorizer;
