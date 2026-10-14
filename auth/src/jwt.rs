use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::{AuthError, Identity};

pub const DEFAULT_EXPIRY_SECS: u64 = 7 * 24 * 60 * 60; // 7 days (matches EE auth)

/// Token type sentinel — distinguishes user sessions from agent service accounts.
const TOKEN_TYPE_USER: &str = "user";
const TOKEN_TYPE_AGENT: &str = "agent";
/// A short-lived, scoped, audience-bound credential minted FROM a session for a
/// server-side renderer acting on the user's behalf (ui/docs/adr/0001a).
/// Rejected by every session decoder below; accepted only by
/// [`decode_delegated_jwt`].
const TOKEN_TYPE_DELEGATED: &str = "delegated";

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
    // A delegated token is not a session either: it may only be accepted by
    // the delegated path, which also enforces audience and scopes.
    if c.token_type == TOKEN_TYPE_DELEGATED {
        return Err(AuthError::InvalidToken(
            "delegated tokens cannot authenticate as a session".into(),
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
    if c.token_type == TOKEN_TYPE_DELEGATED {
        return Err(AuthError::InvalidToken(
            "delegated tokens cannot authenticate as a session".into(),
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

// ─── Delegated tokens (ui/docs/adr/0001a-delegation-security-design.md) ───

/// Claims of a delegated token. Distinct struct from [`JwtClaims`] so a session
/// decoder can never accidentally read one as a session and vice versa.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DelegatedClaims {
    pub sub: String,
    pub jti: String,
    pub exp: u64,
    pub iat: u64,
    /// The issuing control plane's own audience string; the verifier compares
    /// it with its own, so a token for one workspace fails elsewhere.
    pub aud: String,
    /// Allowed scopes (a ceiling, intersected with the request at issuance).
    pub scp: Vec<String>,
    /// `jti` of the session (or BFF assertion) this token was derived from.
    pub parent: String,
    pub username: String,
    pub is_superuser: bool,
    pub token_type: String,
}

/// What a verified delegated token proves.
#[derive(Debug, Clone)]
pub struct DelegatedIdentity {
    pub identity: Identity,
    pub jti: String,
    pub scopes: Vec<String>,
    pub parent_jti: String,
    pub expires_at: u64,
}

/// Encode a delegated JWT (token_type = "delegated"). `ttl_secs` is clamped by
/// the caller to the policy maximum; this function only signs what it is given.
pub fn encode_delegated_jwt(
    secret: &str,
    ttl_secs: u64,
    identity: &Identity,
    aud: &str,
    scopes: &[String],
    parent_jti: &str,
) -> Result<(String, String), AuthError> {
    let now = Utc::now().timestamp() as u64;
    let jti = uuid::Uuid::new_v4().to_string();
    let claims = DelegatedClaims {
        sub: identity.user_id.clone(),
        jti: jti.clone(),
        exp: now + ttl_secs,
        iat: now,
        aud: aud.to_owned(),
        scp: scopes.to_vec(),
        parent: parent_jti.to_owned(),
        username: identity.username.clone(),
        is_superuser: identity.is_superuser,
        token_type: TOKEN_TYPE_DELEGATED.to_owned(),
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AuthError::InvalidToken(e.to_string()))?;
    Ok((token, jti))
}

/// Decode a delegated JWT: signature, expiry (no leeway), `token_type`, and
/// `aud == expected_aud`. Returns the identity plus the scope ceiling; the
/// caller still applies revocation, caller-exists and the route/scope gate.
pub fn decode_delegated_jwt(
    secret: &str,
    token: &str,
    expected_aud: &str,
) -> Result<DelegatedIdentity, AuthError> {
    let mut validation = Validation::default();
    validation.required_spec_claims.clear();
    validation.validate_exp = true;
    validation.leeway = 0;
    validation.set_audience(&[expected_aud]);
    let data = decode::<DelegatedClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::Expired,
        jsonwebtoken::errors::ErrorKind::InvalidAudience => {
            AuthError::InvalidToken("delegated token audience mismatch".into())
        }
        _ => AuthError::InvalidToken(e.to_string()),
    })?;
    let c = data.claims;
    if c.token_type != TOKEN_TYPE_DELEGATED {
        return Err(AuthError::InvalidToken("not a delegated token".into()));
    }
    if c.jti.is_empty() || c.parent.is_empty() {
        return Err(AuthError::InvalidToken(
            "delegated token missing jti/parent".into(),
        ));
    }
    Ok(DelegatedIdentity {
        identity: Identity {
            user_id: c.sub,
            username: c.username,
            is_superuser: c.is_superuser,
        },
        jti: c.jti,
        scopes: c.scp,
        parent_jti: c.parent,
        expires_at: c.exp,
    })
}

/// Peek at `token_type` WITHOUT verifying the signature — only to choose
/// which verifier to run. Never trust the value for anything else.
pub fn peek_token_type(token: &str) -> Option<String> {
    use base64::prelude::*;
    let payload = token.split('.').nth(1)?;
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("token_type")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
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

// The per-request MCP delegation token (mint_delegation_token /
// validate_delegation_token) was removed: agents now authenticate to /api/mcp
// with a deploy-time per-agent credential (`MCP_GATEWAY_TOKEN`,
// `agent_gateway_tokens` table) and the user identity is resolved server-side
// from the request's traceparent via the flows record — see
// docs/MCP_GATEWAY_AGENT_AUTH.md.

#[cfg(test)]
mod delegated_tests {
    use super::*;

    fn ident() -> Identity {
        Identity {
            user_id: "11111111-1111-4111-8111-111111111111".into(),
            username: "satya".into(),
            is_superuser: false,
        }
    }

    #[test]
    fn a_delegated_token_is_never_a_session() {
        let (tok, _) =
            encode_delegated_jwt("s", 60, &ident(), "cp-a", &["usage:read".into()], "parent")
                .unwrap();
        assert!(matches!(
            decode_jwt("s", &tok),
            Err(AuthError::InvalidToken(_))
        ));
        assert!(matches!(
            decode_jwt_with_jti("s", &tok),
            Err(AuthError::InvalidToken(_))
        ));
        assert_eq!(peek_token_type(&tok).as_deref(), Some("delegated"));
    }

    #[test]
    fn a_session_is_never_a_delegated_token() {
        let tok = encode_jwt("s", 60, &ident()).unwrap();
        assert!(decode_delegated_jwt("s", &tok, "cp-a").is_err());
    }

    #[test]
    fn audience_must_match_the_verifier() {
        let (tok, jti) =
            encode_delegated_jwt("s", 60, &ident(), "cp-a", &["usage:read".into()], "parent")
                .unwrap();
        let ok = decode_delegated_jwt("s", &tok, "cp-a").unwrap();
        assert_eq!(ok.jti, jti);
        assert_eq!(ok.scopes, vec!["usage:read".to_string()]);
        assert_eq!(ok.parent_jti, "parent");
        assert!(matches!(
            decode_delegated_jwt("s", &tok, "cp-b"),
            Err(AuthError::InvalidToken(m)) if m.contains("audience")
        ));
    }

    #[test]
    fn another_secret_cannot_mint_for_this_verifier() {
        let (tok, _) =
            encode_delegated_jwt("other", 60, &ident(), "cp-a", &["usage:read".into()], "p")
                .unwrap();
        assert!(decode_delegated_jwt("s", &tok, "cp-a").is_err());
    }

    #[test]
    fn expiry_is_literal() {
        let (tok, _) = encode_delegated_jwt("s", 0, &ident(), "cp-a", &[], "p").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(matches!(
            decode_delegated_jwt("s", &tok, "cp-a"),
            Err(AuthError::Expired)
        ));
    }
}
