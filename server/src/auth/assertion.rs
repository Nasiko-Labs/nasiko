//! Central-dashboard mode: exchange a BFF-signed assertion for a delegated
//! token — `POST /api/auth/delegate/assertion` (ui/docs/adr/0001a §2.2).
//!
//! The dashboard (EE tenant-server) and a workspace control plane have
//! different secrets by design, so the BFF cannot mint this CP's tokens.
//! Instead it signs an **assertion** with its Ed25519 key; this CP verifies
//! it against the JWKS published at the BFF URL that was configured at
//! provision time (`NASIKO_BFF_URL`), binds it to its own audience, consumes
//! its `jti` once, maps the asserted verified email to an EXISTING local user
//! (never creating one), and only then mints an ordinary delegated token
//! through the same policy as the session path. A workspace can therefore
//! verify the BFF without being able to impersonate it, and no secret is
//! shared across workspaces.
//!
//! Disabled unless both `NASIKO_BFF_URL` and `DELEGATION_ENABLED` are set.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use redis::AsyncCommands;
use serde::Deserialize;

use nasiko_auth::assertion::{Jwks, UNKNOWN_KID, verify_assertion};

use super::delegate::{DelegateRequest, DelegateResponse, envelope};
use crate::state::AppState;

/// Process-wide JWKS cache: refreshed at most every [`JWKS_TTL`], and at most
/// once per [`JWKS_MIN_REFRESH`] on an unknown `kid` (rotation), so a flood of
/// bad assertions cannot turn this CP into a JWKS fetch loop.
static JWKS_CACHE: RwLock<Option<(Instant, Arc<Jwks>)>> = RwLock::new(None);
const JWKS_TTL: Duration = Duration::from_secs(300);
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(30);

pub fn jwks_url(bff_url: &str) -> String {
    format!(
        "{}/.well-known/nasiko-delegation-jwks.json",
        bff_url.trim_end_matches('/')
    )
}

async fn load_jwks(state: &AppState, bff_url: &str, force: bool) -> Result<Arc<Jwks>, String> {
    if let Some((at, keys)) = JWKS_CACHE.read().map_err(|_| "jwks lock")?.clone() {
        let age = at.elapsed();
        if age < JWKS_TTL && !(force && age >= JWKS_MIN_REFRESH) {
            return Ok(keys);
        }
    }
    let resp = state
        .http_client
        .get(jwks_url(bff_url))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("jwks fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("jwks fetch returned {}", resp.status()));
    }
    let keys: Jwks = resp.json().await.map_err(|e| format!("jwks parse: {e}"))?;
    let keys = Arc::new(keys);
    *JWKS_CACHE.write().map_err(|_| "jwks lock")? = Some((Instant::now(), keys.clone()));
    Ok(keys)
}

#[derive(Debug, Deserialize)]
pub struct AssertionExchangeRequest {
    #[serde(flatten)]
    pub delegate: DelegateRequest,
}

/// `POST /api/auth/delegate/assertion` — `Authorization: Bearer <assertion>`.
pub async fn exchange_assertion(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AssertionExchangeRequest>,
) -> Response {
    if !state.config.delegation_enabled {
        return envelope(
            StatusCode::SERVICE_UNAVAILABLE,
            "delegation is disabled on this deployment",
        );
    }
    let Some(bff_url) = state
        .config
        .nasiko_bff_url
        .as_deref()
        .filter(|u| !u.trim().is_empty())
    else {
        return envelope(
            StatusCode::NOT_FOUND,
            "this control plane is not attached to a dashboard",
        );
    };
    // A fleet of workspaces on the shared default audience would accept each
    // other's assertions. Refuse rather than mint — this path is only ever
    // reached by dashboard-attached workspaces, which have a public origin.
    if state.config.delegation_audience == nasiko_config::DEFAULT_DELEGATION_AUDIENCE {
        tracing::error!(
            "assertion exchange refused: NASIKO_CP_AUDIENCE/CP_DOMAIN unset, audience is the shared default"
        );
        return envelope(
            StatusCode::SERVICE_UNAVAILABLE,
            "delegation audience is not configured on this deployment",
        );
    }
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return envelope(StatusCode::UNAUTHORIZED, "missing assertion");
    };
    let expected_aud = state.config.delegation_audience.as_str();
    // First try the cached keys; on an unknown kid refresh once (rotation).
    let verified = match load_jwks(&state, bff_url, false).await {
        Ok(jwks) => {
            match verify_assertion(token, &jwks, bff_url.trim_end_matches('/'), expected_aud) {
                Ok(v) => Ok(v),
                Err(e) if e == UNKNOWN_KID => match load_jwks(&state, bff_url, true).await {
                    Ok(jwks) => {
                        verify_assertion(token, &jwks, bff_url.trim_end_matches('/'), expected_aud)
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    };
    let verified = match verified {
        Ok(v) => v,
        Err(reason) => {
            tracing::warn!(reason = %reason, "assertion exchange refused");
            return envelope(StatusCode::UNAUTHORIZED, "invalid assertion");
        }
    };

    // Single use: the first exchange wins; a replay within the TTL is refused.
    let mut conn = match state.redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "assertion exchange: redis unavailable (fail closed)");
            return envelope(
                StatusCode::SERVICE_UNAVAILABLE,
                "delegation temporarily unavailable",
            );
        }
    };
    let marker = format!(
        "delegation:assertion:{}",
        nasiko_auth::jwt::hash_jti(&verified.jti)
    );
    let fresh: bool = match conn
        .set_options(
            &marker,
            1u8,
            redis::SetOptions::default()
                .conditional_set(redis::ExistenceCheck::NX)
                .with_expiration(redis::SetExpiry::EX(
                    nasiko_auth::assertion::ASSERTION_MAX_TTL_SECS,
                )),
        )
        .await
    {
        Ok(redis::Value::Okay) => true,
        Ok(_) => false,
        Err(e) => {
            tracing::error!(error = %e, "assertion exchange: redis error (fail closed)");
            return envelope(
                StatusCode::SERVICE_UNAVAILABLE,
                "delegation temporarily unavailable",
            );
        }
    };
    if !fresh {
        tracing::warn!(jti = %verified.jti, "assertion exchange: replay refused");
        return envelope(StatusCode::UNAUTHORIZED, "assertion already used");
    }

    // Map the verified email to an EXISTING member. No auto-create: the
    // assertion proves who the dashboard authenticated, not that they belong here.
    let row: Option<(uuid::Uuid, String, bool)> = match sqlx::query_as(
        "SELECT id, username, is_superuser FROM users
         WHERE LOWER(email) = $1 AND deleted_at IS NULL AND is_active",
    )
    .bind(&verified.email)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "assertion exchange: user lookup failed");
            return envelope(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not resolve the member",
            );
        }
    };
    let Some((id, username, is_superuser)) = row else {
        tracing::warn!(workspace = %verified.workspace, issuer = %verified.issuer, "assertion exchange: not a member");
        return envelope(StatusCode::FORBIDDEN, "not a member of this workspace");
    };
    let identity = nasiko_auth::Identity {
        user_id: id.to_string(),
        username,
        is_superuser,
    };
    let request = nasiko_auth::DelegationRequest {
        scopes: req.delegate.scopes,
        ttl_secs: req
            .delegate
            .ttl_secs
            .unwrap_or(nasiko_auth::DELEGATION_MAX_TTL_SECS),
        parent_jti: verified.jti.clone(),
        audience: state.config.delegation_audience.clone(),
    };
    match state.auth.issue_delegated_token(&identity, &request).await {
        Ok(issued) => {
            tracing::info!(user = %identity.user_id, jti = %issued.jti, parent = %verified.jti, portal_subject = %verified.portal_subject, scopes = ?issued.scopes, "assertion exchange: issued");
            Json(DelegateResponse {
                token: issued.token,
                expires_in: issued.expires_in,
                scopes: issued.scopes,
                aud: request.audience,
            })
            .into_response()
        }
        Err(nasiko_auth::AuthError::InvalidToken(m)) => envelope(StatusCode::BAD_REQUEST, &m),
        Err(nasiko_auth::AuthError::Unsupported) => envelope(
            StatusCode::NOT_IMPLEMENTED,
            "delegation is not supported by this auth backend",
        ),
        Err(e) => {
            tracing::error!(error = %e, "assertion exchange: failed to issue");
            envelope(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not issue a delegated token",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwks_url_is_derived_from_the_configured_bff_url_only() {
        assert_eq!(
            jwks_url("https://nasiko.dev/"),
            "https://nasiko.dev/.well-known/nasiko-delegation-jwks.json"
        );
    }
}
