use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::{AuthError, Identity, Role};

pub const DEFAULT_EXPIRY_SECS: u64 = 7 * 24 * 60 * 60; // 7 days

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub department_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

pub fn encode_jwt(secret: &str, expiry_secs: u64, identity: &Identity) -> Result<String, AuthError> {
    let now = Utc::now().timestamp() as u64;
    let claims = JwtClaims {
        sub: identity.user_id.clone(),
        jti: uuid::Uuid::new_v4().to_string(),
        exp: now + expiry_secs,
        iat: now,
        username: identity.username.clone(),
        is_superuser: identity.is_superuser,
        team_id: identity.team_id.clone(),
        department_id: identity.department_id.clone(),
        role: identity.role.as_ref().map(|r| {
            serde_json::to_value(r)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        }),
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
    let role: Option<Role> = c
        .role
        .as_deref()
        .and_then(|r| serde_json::from_value(serde_json::Value::String(r.to_owned())).ok());

    Ok(Identity {
        user_id: c.sub.clone(),
        sub: c.sub,
        exp: c.exp,
        iat: c.iat,
        username: c.username,
        is_superuser: c.is_superuser,
        team_id: c.team_id,
        department_id: c.department_id,
        role,
    })
}

/// Like `decode_jwt` but also returns the `jti` claim for token revocation use.
pub fn decode_jwt_with_jti(secret: &str, token: &str) -> Result<(Identity, String), AuthError> {
    let mut validation = Validation::default();
    validation.required_spec_claims.clear();
    validation.validate_exp = true;

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
    let jti = c.jti.clone();
    let role: Option<crate::Role> = c
        .role
        .as_deref()
        .and_then(|r| serde_json::from_value(serde_json::Value::String(r.to_owned())).ok());

    let identity = Identity {
        user_id: c.sub.clone(),
        sub: c.sub,
        exp: c.exp,
        iat: c.iat,
        username: c.username,
        is_superuser: c.is_superuser,
        team_id: c.team_id,
        department_id: c.department_id,
        role,
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