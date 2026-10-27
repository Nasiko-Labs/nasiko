//! `POST /api/auth/delegate` and the request gate for delegated tokens.
//!
//! Design: ui/docs/adr/0001a-delegation-security-design.md. A delegated
//! token is minted FROM a live session for a server-side renderer (the Next
//! app) that needs to read workspace data on the user's behalf. It is
//! short-lived, bound to this control plane's audience, limited to the
//! read-only scopes in [`nasiko_auth::DELEGATION_SCOPES`], recorded in
//! `auth_tokens` (so every existing revocation path covers it), and — the
//! part handlers never have to think about — only accepted for `GET`/`HEAD`
//! requests on the paths its scopes map to ([`request_allowed`]). Anything
//! else is refused in the middleware before a handler runs.

use axum::{
    Extension, Json,
    extract::State,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Claims;
use crate::state::AppState;

/// Attached to the request by `require_auth` when the caller presented a
/// delegated token. Handlers that must never serve delegated callers (none
/// today — the gate is global) can also inspect it.
#[derive(Debug, Clone)]
pub struct Delegation {
    pub scopes: Vec<String>,
    pub jti: String,
    pub parent_jti: String,
}

#[derive(Debug, Deserialize)]
pub struct DelegateRequest {
    pub scopes: Vec<String>,
    /// Seconds; clamped to `DELEGATION_MAX_TTL_SECS`, never raised.
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct DelegateResponse {
    pub token: String,
    pub expires_in: u64,
    pub scopes: Vec<String>,
    pub aud: String,
}

pub(crate) fn envelope(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({ "data": serde_json::Value::Null, "status_code": status.as_u16(), "message": message })),
    )
        .into_response()
}

/// Mint a delegated token for the calling session.
///
/// Refuses: a delegated caller (no chaining), unknown scopes (400), the kill
/// switch (503). The scope set is intersected by policy inside the auth
/// service; the TTL is clamped there too. The session's `jti` becomes the
/// token's `parent` for audit.
pub async fn delegate(
    State(state): State<AppState>,
    claims: Claims,
    delegated: Option<Extension<Delegation>>,
    parent: Option<Extension<SessionJti>>,
    Json(req): Json<DelegateRequest>,
) -> Response {
    if !state.config.delegation_enabled {
        return envelope(
            StatusCode::SERVICE_UNAVAILABLE,
            "delegation is disabled on this deployment",
        );
    }
    if delegated.is_some() {
        tracing::warn!(user = %claims.sub, "delegate: refused — delegated tokens cannot delegate");
        return envelope(StatusCode::FORBIDDEN, "delegated tokens cannot delegate");
    }
    let identity: nasiko_auth::Identity = claims.clone().into();
    let request = nasiko_auth::DelegationRequest {
        scopes: req.scopes,
        ttl_secs: req.ttl_secs.unwrap_or(nasiko_auth::DELEGATION_MAX_TTL_SECS),
        parent_jti: parent.map(|p| p.0.0).unwrap_or_default(),
        audience: state.config.delegation_audience.clone(),
    };
    match state.auth.issue_delegated_token(&identity, &request).await {
        Ok(issued) => {
            tracing::info!(
                user = %claims.sub,
                jti = %issued.jti,
                parent = %request.parent_jti,
                scopes = ?issued.scopes,
                expires_in = issued.expires_in,
                aud = %request.audience,
                "delegate: issued"
            );
            Json(DelegateResponse {
                token: issued.token,
                expires_in: issued.expires_in,
                scopes: issued.scopes,
                aud: request.audience,
            })
            .into_response()
        }
        Err(nasiko_auth::AuthError::InvalidToken(m)) => {
            tracing::warn!(user = %claims.sub, reason = %m, "delegate: refused");
            envelope(StatusCode::BAD_REQUEST, &m)
        }
        Err(nasiko_auth::AuthError::Unsupported) => envelope(
            StatusCode::NOT_IMPLEMENTED,
            "delegation is not supported by this auth backend",
        ),
        Err(e) => {
            tracing::error!(error = %e, "delegate: failed to issue");
            envelope(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not issue a delegated token",
            )
        }
    }
}

/// The session's own `jti`, attached by `require_auth` so `delegate` can
/// record which session a delegation derived from.
#[derive(Debug, Clone)]
pub struct SessionJti(pub String);

/// Scope → path-prefix map. Every entry is read-only; the method check in
/// [`request_allowed`] is what makes "read-only" true regardless of handler.
const SCOPE_ROUTES: &[(&str, &[&str])] = &[
    ("me:read", &["/api/me"]),
    ("usage:read", &["/api/usage/"]),
    ("finops:read", &["/api/observability/finops/"]),
    ("agents:read", &["/api/agents"]),
    ("surfaces:read", &["/api/weave/views"]),
];

/// Whether a delegated token with `scopes` may perform `method path`.
///
/// Rules: safe methods only; the path must be (or be under) a prefix of one
/// of the token's scopes; `/api/auth/*` is never allowed (no chaining, no
/// logout/password paths); a prefix ending in `/` matches children only, a
/// prefix without `/` matches itself or children (`/api/agents`,
/// `/api/agents/{id}`, `/api/agents/{id}/stats`).
pub fn request_allowed(method: &Method, path: &str, scopes: &[String]) -> bool {
    if !(method == Method::GET || method == Method::HEAD) {
        return false;
    }
    if path.starts_with("/api/auth") || path.contains("..") {
        return false;
    }
    scopes.iter().any(|scope| {
        SCOPE_ROUTES
            .iter()
            .filter(|(s, _)| *s == scope)
            .flat_map(|(_, prefixes)| prefixes.iter())
            .any(|prefix| {
                if let Some(dir) = prefix.strip_suffix('/') {
                    path.starts_with(prefix) && path.len() > prefix.len() || path == dir
                } else {
                    path == *prefix || path.starts_with(&format!("{prefix}/"))
                }
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn only_safe_methods() {
        assert!(request_allowed(
            &Method::GET,
            "/api/usage/summary",
            &s(&["usage:read"])
        ));
        assert!(request_allowed(
            &Method::HEAD,
            "/api/usage/summary",
            &s(&["usage:read"])
        ));
        assert!(!request_allowed(
            &Method::POST,
            "/api/usage/summary",
            &s(&["usage:read"])
        ));
        assert!(!request_allowed(
            &Method::DELETE,
            "/api/agents/x",
            &s(&["agents:read"])
        ));
    }

    #[test]
    fn scope_prefixes_are_exact_families() {
        let fin = s(&["finops:read"]);
        assert!(request_allowed(
            &Method::GET,
            "/api/observability/finops/dashboard",
            &fin
        ));
        assert!(request_allowed(
            &Method::GET,
            "/api/observability/finops/spend-calendar/day",
            &fin
        ));
        assert!(!request_allowed(
            &Method::GET,
            "/api/observability/session/list",
            &fin
        ));
        assert!(!request_allowed(&Method::GET, "/api/usage/summary", &fin));
        let me = s(&["me:read"]);
        assert!(request_allowed(&Method::GET, "/api/me", &me));
        assert!(request_allowed(
            &Method::GET,
            "/api/me/context-strategy",
            &me
        ));
        assert!(!request_allowed(&Method::GET, "/api/members", &me));
        let ag = s(&["agents:read"]);
        assert!(request_allowed(&Method::GET, "/api/agents", &ag));
        assert!(request_allowed(&Method::GET, "/api/agents/123/stats", &ag));
        assert!(!request_allowed(&Method::GET, "/api/agentsx", &ag));
    }

    #[test]
    fn auth_paths_and_generation_are_never_delegated() {
        let all = s(&[
            "me:read",
            "usage:read",
            "finops:read",
            "agents:read",
            "surfaces:read",
        ]);
        assert!(!request_allowed(&Method::GET, "/api/auth/users/1", &all));
        assert!(!request_allowed(&Method::POST, "/api/auth/delegate", &all));
        assert!(!request_allowed(&Method::POST, "/api/weave/surface", &all));
        assert!(!request_allowed(&Method::GET, "/api/secrets/x", &all));
        assert!(!request_allowed(&Method::GET, "/api/usage/../auth/x", &all));
    }

    #[test]
    fn empty_scopes_allow_nothing() {
        assert!(!request_allowed(&Method::GET, "/api/me", &[]));
    }
}
