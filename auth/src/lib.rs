pub mod jwt;
pub mod service;

pub use service::AuthServiceImpl;

/// Lifetime of an issued session token, and the single source of truth for it.
///
/// Both editions mint tokens with this TTL, and the server's session cookie
/// `Max-Age` is derived from it — a cookie shorter than the token logs the user
/// out while their token is still valid, which is what a hardcoded 12-hour
/// cookie used to do here.
pub const TOKEN_EXPIRY_SECS: u64 = 7 * 24 * 60 * 60;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Identity extracted after authentication.
///
/// Only carries the minimal fields shared across OSS and EE. Enterprise concepts
/// (role, team, department) are internal EE details resolved from the DB by the
/// EE `AuthService` implementation — they are never part of this shared interface
/// nor exposed in any public API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_superuser: bool,
}

/// Consolidated auth trait — replaces AuthProvider, Authorizer, UserAuthService, TokenService.
///
/// Permission checks (`can_*`) are async so the EE implementation can resolve the
/// user's role from the database. The default implementations grant everything,
/// matching the OSS single-user model (no RBAC); EE overrides them with DB-backed
/// hierarchy checks.
#[async_trait]
pub trait AuthService: Send + Sync + 'static {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError>;
    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError>;
    async fn authenticate(&self, username: &str, password: &str) -> Result<LoginResult, AuthError>;
    async fn bootstrap_admin(&self, username: &str, password: &str) -> Result<(), AuthError>;
    async fn issue_agent_token(&self, agent_id: &str) -> Result<String, AuthError>;
    async fn upsert_oauth_user(
        &self,
        provider: &str,
        provider_id: &str,
        username: &str,
        verified_email: Option<&str>,
    ) -> Result<LoginResult, AuthError>;
    async fn lookup_user(&self, user_id: &str) -> Result<Identity, AuthError>;
    async fn record_user_token(&self, token: &str, user_id: &str) -> Result<(), AuthError>;
    /// Revoke a single session by its token `jti` — used by logout, so signing
    /// out of one session (CLI or browser) never touches any other session.
    async fn revoke_token(&self, jti: &str) -> Result<u64, AuthError>;
    async fn revoke_tokens_for_user(&self, user_id: &str) -> Result<u64, AuthError>;
    async fn revoke_all_tokens(&self) -> Result<u64, AuthError>;
    async fn revoke_tokens_for_agent(&self, agent_id: &str) -> Result<u64, AuthError>;
    async fn can_access_agent(&self, identity: &Identity, agent_id: &str) -> bool;

    // ─── Permission checks (OSS: allow-all; EE: DB-backed RBAC) ──────────────────

    /// May the identity deploy/build agents and manage containers? (EE: ≥ team_member)
    async fn can_deploy(&self, identity: &Identity) -> bool {
        let _ = identity;
        true
    }
    // There is deliberately no `can_manage_secrets` here. Secret access is
    // authorized per-resource, not by role, and both surfaces already do it:
    // `/api/secrets` is scoped to `user_secrets.user_id` and encrypted under a
    // key derived from that id, and `/api/agents/{id}/secrets` goes through
    // `acl::can_manage_agent` (owner or superuser — an invoke grant or a public
    // flag must not confer secret-write). A role threshold would be strictly
    // wider than either. One existed here for a while, called by nothing, which
    // is worse than absent: it reads as protection while protecting nothing.

    /// May the identity manage users? (EE: ≥ admin)
    async fn can_manage_users(&self, identity: &Identity) -> bool {
        let _ = identity;
        true
    }
    /// May the identity manage the VM pool / scaling? (EE: ≥ admin)
    async fn can_manage_pool(&self, identity: &Identity) -> bool {
        let _ = identity;
        true
    }
    /// May the identity read org structure — teams/departments, their members and
    /// agents (the org chart + member emails)? (EE: ≥ team_lead)
    async fn can_read_org(&self, identity: &Identity) -> bool {
        let _ = identity;
        true
    }

    /// Scope a directory-style user listing (e.g. search/autocomplete) to what
    /// `identity` should see. `None` means unrestricted — OSS's single-user
    /// model always returns `None`; EE returns `None` for admins/superusers
    /// and `Some(user_ids)` for everyone else, restricted to their own team or
    /// department. Deliberately returns opaque user-id strings rather than
    /// taking a query/filter callback, so this shared trait never has to know
    /// about EE-only org-hierarchy columns (`team_id`/`department_id`) that
    /// don't exist on an OSS-only schema.
    async fn org_visible_user_ids(&self, identity: &Identity) -> Option<Vec<String>> {
        let _ = identity;
        None
    }

    /// Agents `identity` can see through an org-hierarchy grant — i.e. an
    /// `agent_grants` row of type `team`, `department` or `organization` that
    /// resolves via the caller's placement in the org chart.
    ///
    /// Listing endpoints union this with the ownership/public/user-grant
    /// predicate they compute in SQL. It exists as a seam for the same reason
    /// as [`Self::org_visible_user_ids`]: resolving a team or department grant
    /// needs the EE-only `users.team_id`/`users.department_id` columns, which a
    /// shared query must never reference. OSS has no org chart, so the default
    /// adds nothing.
    async fn org_granted_agent_ids(&self, identity: &Identity) -> Vec<String> {
        let _ = identity;
        Vec::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing token")]
    MissingToken,
    /// A genuinely malformed/undecodable JWT (not a DB or credential failure).
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("expired token")]
    Expired,
    #[error("token revoked")]
    Revoked,
    /// Bad username/access-key or wrong secret. Maps to 401.
    /// `remaining_attempts` is `Some` when login lockout is approaching.
    #[error("invalid credentials")]
    InvalidCredentials { remaining_attempts: Option<i32> },
    /// Account is deactivated. Maps to 401/403.
    #[error("account disabled")]
    Disabled,
    /// Too many failed login attempts — account temporarily locked.
    /// `retry_after_secs` is the number of seconds until the lockout expires.
    #[error("account locked")]
    AccountLocked { retry_after_secs: u64 },
    /// Requested user/agent/record does not exist. Maps to 404.
    #[error("not found")]
    NotFound,
    /// Uniqueness / state conflict. Maps to 409.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A backend/database failure — must surface as 500, NEVER as 401. Previously
    /// these were coerced to `InvalidToken(e.to_string())`, so a DB outage looked
    /// like an auth failure (and leaked the raw error).
    #[error("database error")]
    Database(#[from] sqlx::Error),
    /// Operation not supported by this AuthService impl (e.g. gateway pass-through).
    #[error("unsupported operation")]
    Unsupported,
    /// An unexpected internal failure unrelated to auth semantics (e.g. password
    /// hashing) — must surface as 500, never as a token/credential rejection.
    /// Mirrors `Database`: the Display impl is intentionally generic so the raw
    /// detail (logged by the caller) never round-trips into an HTTP response body.
    #[error("internal error")]
    Internal(String),
}

// ─── Password helpers ────────────────────────────────────────────────────────

/// Minimum length for a **user-chosen** password, in characters.
pub const MIN_PASSWORD_LEN: usize = 12;

/// Maximum length for a user-chosen password, in characters. NIST SP 800-63B
/// asks verifiers to accept at least 64.
pub const MAX_PASSWORD_LEN: usize = 64;

/// bcrypt silently truncates its input at 72 bytes. Accepting a longer password
/// would mean the tail of it never affects the hash — two different passwords
/// sharing a 72-byte prefix would verify against each other — so it is rejected
/// with an explicit message rather than quietly ignored. Byte-counted, not
/// character-counted: 64 CJK characters are ~192 bytes.
pub const MAX_PASSWORD_BYTES: usize = 72;

/// Why a user-chosen password was refused.
///
/// Carries a stable slug so the HTTP layer can satisfy `API_CONVENTIONS.md` §2
/// without restating the policy, and a message safe to show the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordPolicyError {
    TooShort,
    TooLong,
    TooManyBytes,
    MissingLowercase,
    MissingUppercase,
    MissingDigit,
    MissingSymbol,
}

impl PasswordPolicyError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TooShort => "password_too_short",
            Self::TooLong => "password_too_long",
            Self::TooManyBytes => "password_too_many_bytes",
            Self::MissingLowercase => "password_missing_lowercase",
            Self::MissingUppercase => "password_missing_uppercase",
            Self::MissingDigit => "password_missing_digit",
            Self::MissingSymbol => "password_missing_symbol",
        }
    }

    pub fn message(self) -> String {
        match self {
            Self::TooShort => format!("password must be at least {MIN_PASSWORD_LEN} characters"),
            Self::TooLong => format!("password must be at most {MAX_PASSWORD_LEN} characters"),
            Self::TooManyBytes => {
                format!("password must be at most {MAX_PASSWORD_BYTES} bytes")
            }
            Self::MissingLowercase => "password must contain a lowercase letter".into(),
            Self::MissingUppercase => "password must contain an uppercase letter".into(),
            Self::MissingDigit => "password must contain a digit".into(),
            Self::MissingSymbol => {
                "password must contain a symbol (anything that is not a letter or digit)".into()
            }
        }
    }
}

/// Enforce the composition policy on a password the **user chose**.
///
/// Deliberately NOT applied to generated credentials. `generate_access_secret`
/// draws from an alphanumeric + `-_` alphabet and the installer's
/// `generate_secret` is alphanumeric only, so running either through this would
/// reject values the platform itself minted — and would fail every cluster
/// install, since the bootstrap `ADMIN_PASSWORD` is generated that way. Those
/// are 143-258 bits of CSPRNG output; composition rules exist to push *human*
/// choices off a small predictable set and buy such values nothing.
///
/// Character classes are Unicode-aware, so a non-Latin password is judged by
/// the same rules rather than being rejected for lacking ASCII.
pub fn validate_password(password: &str) -> Result<(), PasswordPolicyError> {
    // Byte check first: it is the one limit that silently corrupts rather than
    // merely refusing, so it should be reported even for an otherwise fine
    // password.
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(PasswordPolicyError::TooManyBytes);
    }

    let chars = password.chars().count();
    if chars < MIN_PASSWORD_LEN {
        return Err(PasswordPolicyError::TooShort);
    }
    if chars > MAX_PASSWORD_LEN {
        return Err(PasswordPolicyError::TooLong);
    }

    if !password.chars().any(char::is_lowercase) {
        return Err(PasswordPolicyError::MissingLowercase);
    }
    if !password.chars().any(char::is_uppercase) {
        return Err(PasswordPolicyError::MissingUppercase);
    }
    if !password.chars().any(|c| c.is_numeric()) {
        return Err(PasswordPolicyError::MissingDigit);
    }
    // "Symbol" is defined by exclusion so every punctuation mark, currency sign
    // and space counts — an allowlist of ASCII specials would reject a password
    // whose only symbol is an em dash or a non-Latin punctuation mark.
    if !password.chars().any(|c| !c.is_alphanumeric()) {
        return Err(PasswordPolicyError::MissingSymbol);
    }

    Ok(())
}

/// Hash a password with bcrypt cost 12.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    bcrypt::hash(password, 12).map_err(|e| AuthError::Internal(e.to_string()))
}

/// Verify a bcrypt password.
pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

/// Hash a password off the async executor (bcrypt cost-12 is ~50-100ms CPU, so it
/// runs on the blocking pool to avoid stalling a tokio worker under concurrent logins).
pub async fn hash_password_async(password: &str) -> Result<String, AuthError> {
    let pw = password.to_owned();
    tokio::task::spawn_blocking(move || bcrypt::hash(&pw, 12))
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
        .map_err(|e| AuthError::Internal(e.to_string()))
}

/// Verify a bcrypt password off the async executor. A join failure verifies as `false` (deny).
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
    OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS CSPRNG unavailable");
    bytes
        .iter()
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

    async fn authenticate(
        &self,
        _username: &str,
        _password: &str,
    ) -> Result<LoginResult, AuthError> {
        Err(AuthError::Unsupported)
    }

    async fn bootstrap_admin(&self, _username: &str, _password: &str) -> Result<(), AuthError> {
        Ok(())
    }

    async fn issue_agent_token(&self, _agent_id: &str) -> Result<String, AuthError> {
        Err(AuthError::Unsupported)
    }

    async fn upsert_oauth_user(
        &self,
        _provider: &str,
        _provider_id: &str,
        _username: &str,
        _verified_email: Option<&str>,
    ) -> Result<LoginResult, AuthError> {
        Err(AuthError::Unsupported)
    }

    async fn lookup_user(&self, _user_id: &str) -> Result<Identity, AuthError> {
        Err(AuthError::Unsupported)
    }

    async fn record_user_token(&self, _token: &str, _user_id: &str) -> Result<(), AuthError> {
        Ok(())
    }

    async fn revoke_token(&self, _jti: &str) -> Result<u64, AuthError> {
        Ok(0)
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
