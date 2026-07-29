//! Agent-identity JWT verification.
//!
//! The agent's `OPENAI_API_KEY` is a Nasiko-issued JWT (NOT a provider key) carrying
//! `{agent_id, owner_id, iat, exp}`. The orchestrator mints it (1-year `exp`, rotated
//! on redeploy); this gateway only *verifies* it. See `RUST_PLAN_V1.md` §3 (auth).
//!
//! A dev/test minting helper ([`mint_agent_token`]) lives here too — used by the test
//! suite and the `mint_token` example for manual curl smoke runs.

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::config::GatewayConfig;
use crate::error::GatewayError;

/// Default agent-token TTL when minting: 1 year (matches the orchestrator).
pub const DEFAULT_TTL_SECONDS: u64 = 365 * 24 * 60 * 60;

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
}

/// Verify an `Authorization` header value and extract `(agent_id, owner_id)`.
///
/// - `None` header ⇒ 401 "Missing Authorization header".
/// - Empty `agent_jwt_secret` ⇒ fail closed: 401 "Gateway JWT secret not configured".
/// - Expired ⇒ 401 "Agent token expired". Other signature/format errors ⇒ 401
///   "Invalid agent token: <err>". Absent/empty `agent_id` ⇒ 401 "Token missing
///   agent_id claim".
pub fn verify_agent_jwt(
    authorization: Option<&str>,
    cfg: &GatewayConfig,
) -> Result<(String, String), GatewayError> {
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
    Ok((agent_id, owner_id))
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
