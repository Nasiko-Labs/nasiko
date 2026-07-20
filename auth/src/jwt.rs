use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::{AuthError, Identity};

pub const DEFAULT_EXPIRY_SECS: u64 = 7 * 24 * 60 * 60; // 7 days (matches EE auth)

/// Token type sentinel — distinguishes user sessions from agent service accounts.
const TOKEN_TYPE_USER: &str = "user";
const TOKEN_TYPE_AGENT: &str = "agent";

fn default_user_token_type() -> String {
    TOKEN_TYPE_USER.to_owned()
}

/// Internal JWT claims — never exposed outside this module.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JwtClaims {
    pub sub: String,
    #[serde(default)]
    pub jti: String,
    pub exp: u64,
    pub iat: u64,
    pub username: String,
    pub is_superuser: bool,
    /// "user" | "agent". Defaults to "user" so legacy tokens (pre-AUTH-3)
    /// decode correctly — they carry no token_type claim.
    #[serde(default = "default_user_token_type")]
    pub token_type: String,
}

/// Encode a user session JWT (token_type = "user").
pub fn encode_jwt(
    secret: &str,
    expiry_secs: u64,
    identity: &Identity,
) -> Result<String, AuthError> {
    encode_jwt_inner(secret, expiry_secs, TOKEN_TYPE_USER, identity)
}

/// Encode an agent service-account JWT (token_type = "agent").
/// These tokens are REJECTED by `decode_jwt` / `decode_jwt_with_jti` so they
/// cannot be used to authenticate as a human user (AUTH-3).
pub fn encode_agent_jwt(
    secret: &str,
    expiry_secs: u64,
    identity: &Identity,
) -> Result<String, AuthError> {
    encode_jwt_inner(secret, expiry_secs, TOKEN_TYPE_AGENT, identity)
}

fn encode_jwt_inner(
    secret: &str,
    expiry_secs: u64,
    token_type: &str,
    identity: &Identity,
) -> Result<String, AuthError> {
    let now = Utc::now().timestamp() as u64;
    let claims = JwtClaims {
        sub: identity.user_id.clone(),
        jti: uuid::Uuid::new_v4().to_string(),
        exp: now + expiry_secs,
        iat: now,
        username: identity.username.clone(),
        is_superuser: identity.is_superuser,
        token_type: token_type.to_owned(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AuthError::InvalidToken(e.to_string()))
}

pub fn decode_jwt(secret: &str, token: &str) -> Result<Identity, AuthError> {
    let mut validation = Validation::default();
    validation.required_spec_claims.clear();
    validation.validate_exp = true;
    validation.leeway = 0; // treat exp literally — no clock-skew tolerance

    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::Expired,
        _ => AuthError::InvalidToken(e.to_string()),
    })?;

    let c = data.claims;
    // Agent tokens must not be accepted as human-user credentials (AUTH-3).
    if c.token_type == TOKEN_TYPE_AGENT {
        return Err(AuthError::InvalidToken(
            "agent tokens cannot authenticate as a user".into(),
        ));
    }
    Ok(Identity {
        user_id: c.sub,
        username: c.username,
        is_superuser: c.is_superuser,
    })
}

/// Like `decode_jwt` but also returns the `jti` claim for token revocation use.
pub fn decode_jwt_with_jti(secret: &str, token: &str) -> Result<(Identity, String), AuthError> {
    let mut validation = Validation::default();
    validation.required_spec_claims.clear();
    validation.validate_exp = true;
    validation.leeway = 0; // treat exp literally — no clock-skew tolerance

    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::Expired,
        _ => AuthError::InvalidToken(e.to_string()),
    })?;

    let c = data.claims;
    // Agent tokens must not be accepted as human-user credentials (AUTH-3).
    if c.token_type == TOKEN_TYPE_AGENT {
        return Err(AuthError::InvalidToken(
            "agent tokens cannot authenticate as a user".into(),
        ));
    }
    let jti = c.jti.clone();
    let identity = Identity {
        user_id: c.sub,
        username: c.username,
        is_superuser: c.is_superuser,
    };

    Ok((identity, jti))
}

/// SHA-256 hex digest of a jti string — used as token_hash in auth_tokens table.
pub fn hash_jti(jti: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(jti.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Extract the `jti` claim from a JWT without verifying the signature.
/// Only call this on tokens you just issued — the purpose is to record
/// the JTI for later revocation, not to authenticate anything.
pub fn extract_jti(token: &str) -> Option<String> {
    use base64::prelude::*;
    let payload = token.split('.').nth(1)?;
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("jti")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}
