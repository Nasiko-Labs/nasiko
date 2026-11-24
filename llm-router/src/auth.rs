//! Agent-identity JWT verification, and reading a caller's credential.
//!
//! The agent's `OPENAI_API_KEY` is a Nasiko-issued JWT (NOT a provider key) carrying
//! `{agent_id, owner_id, iat, exp}`. The orchestrator mints it (1-year `exp`, rotated
//! on redeploy); this gateway only *verifies* it. See `RUST_PLAN_V1.md` §3 (auth).
//!
//! A coding agent's session token arrives the same way and is tried first
//! ([`verify_session_credential`]). Both are read from whichever of [`CREDENTIAL_HEADERS`] the
//! caller's SDK sends ([`caller_credential`]). These are public so that a layer in front of the
//! router identifies a call exactly as the router will, never with a copy of the rule.
//!
//! A dev/test minting helper ([`mint_agent_token`]) lives here too — used by the test
//! suite and the `mint_token` example for manual curl smoke runs.

use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use nasiko_session_token::{SessionClaims, SessionTokenError, verify_session_token};
use serde::{Deserialize, Serialize};

use crate::config::GatewayConfig;
use crate::error::GatewayError;

/// Default agent-token TTL when minting: 1 year (matches the orchestrator).
pub const DEFAULT_TTL_SECONDS: u64 = 365 * 24 * 60 * 60;

/// The headers a caller's credential is read from, in this order: the OpenAI SDK's
/// `Authorization`, the Anthropic SDK's `x-api-key` (Claude Code sends its key there), and the
/// Gemini SDK's `x-goog-api-key`.
pub const CREDENTIAL_HEADERS: [HeaderName; 3] = [
    AUTHORIZATION,
    HeaderName::from_static("x-api-key"),
    HeaderName::from_static("x-goog-api-key"),
];

/// The caller's credential: the first of [`CREDENTIAL_HEADERS`] the request carries.
pub fn caller_credential(headers: &HeaderMap) -> Option<&str> {
    CREDENTIAL_HEADERS
        .iter()
        .find_map(|name| headers.get(name).and_then(|value| value.to_str().ok()))
}

/// The coding-agent session token in `credential`, verified and not revoked.
///
/// `Ok(None)` when it is not one of our session tokens, or this server issues none (no
/// `AGENT_JWT_SECRET`): the caller then tries it as an agent-identity token. An expired or
/// revoked session token is an error, never a fall-through (fail closed), and so is a revocation
/// check the store cannot answer.
pub async fn verify_session_credential(
    credential: Option<&str>,
    cfg: &GatewayConfig,
    db: &sqlx::PgPool,
) -> Result<Option<SessionClaims>, GatewayError> {
    let Some(raw) = credential.filter(|_| !cfg.agent_jwt_secret.is_empty()) else {
        return Ok(None);
    };
    let algorithm = parse_algorithm(&cfg.agent_jwt_algorithm);
    match verify_session_token(raw, &cfg.agent_jwt_secret, algorithm) {
        Ok(claims) => {
            if is_session_token_revoked(db, &claims.jti).await? {
                return Err(GatewayError::Forbidden("session token revoked".into()));
            }
            Ok(Some(claims))
        }
        Err(SessionTokenError::Expired) => Err(GatewayError::TokenExpired),
        // Not one of ours, or (with the secret set and a token present, so this cannot really
        // happen) unusable: try it as an agent-identity token rather than hard-fail.
        Err(_) => Ok(None),
    }
}

/// True if this session token's `jti` has been revoked. Fails closed (a store
/// error denies the request) rather than letting a possibly-revoked token through.
async fn is_session_token_revoked(db: &sqlx::PgPool, jti: &str) -> Result<bool, GatewayError> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM revoked_session_tokens WHERE jti = $1")
            .bind(jti)
            .fetch_optional(db)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "llm-router: revocation check failed — failing closed");
                GatewayError::Forbidden("identity store unavailable".into())
            })?;
    Ok(found.is_some())
}

/// Claims we read off an incoming agent token. `agent_id` is required (checked after
/// decode so we can return the precise "missing agent_id" message); `owner_id` is
/// optional and defaults to the empty string. `exp` is validated by `jsonwebtoken`
/// itself, so it need not appear here.
#[derive(Debug, Deserialize)]
struct AgentClaims {
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
    #[serde(default)]
    iat: Option<i64>,
}

/// A verified agent token's identity: who it speaks for, and when it was issued (`iat`, Unix
/// seconds; `None` for a token minted without one), which the router compares against the
/// agent's `llm_tokens_revoked_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToken {
    pub agent_id: String,
    pub owner_id: String,
    pub issued_at: Option<i64>,
}

/// Verify an `Authorization` header value and extract `(agent_id, owner_id)`.
///
/// - `None` header ⇒ 401 "Missing Authorization header".
/// - Empty `agent_jwt_secret` ⇒ fail closed: 503 "LLM router misconfigured: AGENT_JWT_SECRET is
///   not set on the server." (a server fault, so not a 401 an SDK reads as a bad key).
/// - Expired ⇒ 401 "Agent token expired". Other signature/format errors ⇒ 401
///   "Invalid agent token: <err>". Absent/empty `agent_id` ⇒ 401 "Token missing
///   agent_id claim".
pub fn verify_agent_jwt(
    authorization: Option<&str>,
    cfg: &GatewayConfig,
) -> Result<(String, String), GatewayError> {
    let token = verify_agent_token(authorization, cfg)?;
    Ok((token.agent_id, token.owner_id))
}

/// [`verify_agent_jwt`], also returning the token's issue time.
pub fn verify_agent_token(
    authorization: Option<&str>,
    cfg: &GatewayConfig,
) -> Result<AgentToken, GatewayError> {
    let header = authorization.ok_or(GatewayError::MissingAuthHeader)?;

    // Fail closed on misconfiguration — never fail open.
    if cfg.agent_jwt_secret.is_empty() {
        return Err(GatewayError::JwtSecretNotConfigured);
    }

    let token = strip_bearer(header);

    let mut validation = Validation::new(parse_algorithm(&cfg.agent_jwt_algorithm));
    validation.validate_aud = false; // no audience claim in our tokens

    let key = DecodingKey::from_secret(cfg.agent_jwt_secret.as_bytes());
    let data = decode::<AgentClaims>(token, &key, &validation).map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => GatewayError::TokenExpired,
        _ => GatewayError::InvalidToken(e.to_string()),
    })?;

    let agent_id = data
        .claims
        .agent_id
        .filter(|s| !s.is_empty())
        .ok_or(GatewayError::MissingAgentId)?;
    let owner_id = data.claims.owner_id.unwrap_or_default();
    Ok(AgentToken {
        agent_id,
        owner_id,
        issued_at: data.claims.iat,
    })
}

/// Strip a case-insensitive `Bearer ` prefix; tolerate a raw token with no prefix.
fn strip_bearer(header: &str) -> &str {
    let trimmed = header.trim();
    match trimmed.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("bearer ") => trimmed[7..].trim(),
        _ => trimmed,
    }
}

/// Map an `AGENT_JWT_ALGORITHM` string to an [`Algorithm`]. Only HMAC variants are
/// supported (the token is signed with a shared secret); unknown ⇒ HS256.
pub fn parse_algorithm(name: &str) -> Algorithm {
    match name.trim().to_ascii_uppercase().as_str() {
        "HS384" => Algorithm::HS384,
        "HS512" => Algorithm::HS512,
        _ => Algorithm::HS256,
    }
}

// ── Minting (dev/test helper) ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct MintClaims {
    agent_id: String,
    owner_id: String,
    iat: usize,
    exp: usize,
}

/// Mint an agent-identity JWT valid for `ttl_seconds` from now. Dev/test only — the
/// orchestrator owns minting in production.
pub fn mint_agent_token(
    agent_id: &str,
    owner_id: &str,
    secret: &str,
    ttl_seconds: u64,
    algorithm: Algorithm,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = now_unix();
    mint_with_times(
        agent_id,
        owner_id,
        secret,
        now,
        now + ttl_seconds as usize,
        algorithm,
    )
}

/// Mint with explicit `iat`/`exp` — lets tests forge expired tokens.
fn mint_with_times(
    agent_id: &str,
    owner_id: &str,
    secret: &str,
    iat: usize,
    exp: usize,
    algorithm: Algorithm,
) -> Result<String, jsonwebtoken::errors::Error> {
    let claims = MintClaims {
        agent_id: agent_id.to_string(),
        owner_id: owner_id.to_string(),
        iat,
        exp,
    };
    encode(
        &Header::new(algorithm),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

fn now_unix() -> usize {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as usize)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    const SECRET: &str = "test-shared-secret";

    fn cfg(secret: &str) -> GatewayConfig {
        GatewayConfig {
            agent_jwt_secret: secret.to_string(),
            ..Default::default()
        }
    }

    fn bearer(token: &str) -> String {
        format!("Bearer {token}")
    }

    #[test]
    fn valid_token_yields_agent_and_owner() {
        let token =
            mint_agent_token("agent-123", "owner-abc", SECRET, 3600, Algorithm::HS256).unwrap();
        let (agent_id, owner_id) = verify_agent_jwt(Some(&bearer(&token)), &cfg(SECRET)).unwrap();
        assert_eq!(agent_id, "agent-123");
        assert_eq!(owner_id, "owner-abc");
    }

    #[test]
    fn verified_token_carries_its_issue_time() {
        let now = now_unix();
        let token =
            mint_with_times("a", "o", SECRET, now - 30, now + 3600, Algorithm::HS256).unwrap();
        let verified = verify_agent_token(Some(&bearer(&token)), &cfg(SECRET)).unwrap();
        assert_eq!(verified.issued_at, Some((now - 30) as i64));
        assert_eq!(verified.agent_id, "a");
        assert_eq!(verified.owner_id, "o");
    }

    #[test]
    fn missing_header_is_401() {
        let err = verify_agent_jwt(None, &cfg(SECRET)).unwrap_err();
        assert!(matches!(err, GatewayError::MissingAuthHeader));
        assert_eq!(err.to_string(), "Missing Authorization header");
    }

    #[test]
    fn empty_secret_fails_closed() {
        // Even with a present header, an unconfigured secret rejects everything.
        let token = mint_agent_token("a", "o", SECRET, 3600, Algorithm::HS256).unwrap();
        let err = verify_agent_jwt(Some(&bearer(&token)), &cfg("")).unwrap_err();
        assert!(matches!(err, GatewayError::JwtSecretNotConfigured));
    }

    #[test]
    fn expired_token_says_expired() {
        // exp well in the past (beyond jsonwebtoken's default 60s leeway).
        let now = now_unix();
        let token = mint_with_times(
            "a",
            "o",
            SECRET,
            now - 10_000,
            now - 5_000,
            Algorithm::HS256,
        )
        .unwrap();
        let err = verify_agent_jwt(Some(&bearer(&token)), &cfg(SECRET)).unwrap_err();
        assert!(matches!(err, GatewayError::TokenExpired));
        assert!(err.to_string().to_lowercase().contains("expired"));
    }

    #[test]
    fn forged_token_is_invalid() {
        let token = mint_agent_token("a", "o", "wrong-secret", 3600, Algorithm::HS256).unwrap();
        let err = verify_agent_jwt(Some(&bearer(&token)), &cfg(SECRET)).unwrap_err();
        assert!(matches!(err, GatewayError::InvalidToken(_)));
        assert!(err.to_string().starts_with("Invalid agent token:"));
    }

    #[test]
    fn missing_agent_id_claim_is_rejected() {
        // A well-signed token that simply has no agent_id.
        #[derive(Serialize)]
        struct NoAgent {
            owner_id: String,
            iat: usize,
            exp: usize,
        }
        let now = now_unix();
        let token = encode(
            &Header::new(Algorithm::HS256),
            &NoAgent {
                owner_id: "o".into(),
                iat: now,
                exp: now + 3600,
            },
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap();
        let err = verify_agent_jwt(Some(&bearer(&token)), &cfg(SECRET)).unwrap_err();
        assert!(matches!(err, GatewayError::MissingAgentId));
    }

    #[test]
    fn owner_id_defaults_to_empty() {
        let token = mint_agent_token("agent-1", "", SECRET, 3600, Algorithm::HS256).unwrap();
        let (agent_id, owner_id) = verify_agent_jwt(Some(&bearer(&token)), &cfg(SECRET)).unwrap();
        assert_eq!(agent_id, "agent-1");
        assert_eq!(owner_id, "");
    }

    #[test]
    fn accepts_case_insensitive_bearer_and_raw_token() {
        let token = mint_agent_token("agent-1", "o", SECRET, 3600, Algorithm::HS256).unwrap();
        for header in [
            format!("Bearer {token}"),
            format!("bearer {token}"),
            format!("BEARER {token}"),
            token.clone(), // raw, no prefix
        ] {
            let (agent_id, _) = verify_agent_jwt(Some(&header), &cfg(SECRET)).unwrap();
            assert_eq!(agent_id, "agent-1");
        }
    }
}
