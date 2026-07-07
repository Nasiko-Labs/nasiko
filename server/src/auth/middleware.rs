use axum::{
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::Claims;
use crate::state::AppState;

/// Auth middleware — validates the JWT from Authorization: Bearer or access_token cookie.
///
/// No gateway required: the server validates tokens directly via AuthService.
/// Revocation is enforced via an O(1) indexed lookup on auth_tokens.token_hash.
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(token) = extract_token(req.headers()) else {
        return (StatusCode::UNAUTHORIZED, "missing or invalid token").into_response();
    };

    let identity = match state.auth.validate_token(&token).await {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
    };

    // Revocation check — O(1) indexed lookup on token_hash.
    // Fail CLOSED (AUTH-5): if the lookup errors we cannot prove the token is
    // still valid, so we deny rather than let a possibly-revoked token through.
    //
    // A missing/empty `jti` must ALSO fail closed rather than silently skip
    // the check — every token this codebase issues (`jwt::encode_jwt`) always
    // sets a real UUID jti, so a signature-valid token with none is either a
    // legacy/malformed token or one crafted outside the normal issuance path;
    // either way it must not bypass revocation entirely.
    let jti = nasiko_auth::jwt::extract_jti(&token).filter(|j| !j.is_empty());
    let Some(jti) = jti else {
        return (StatusCode::UNAUTHORIZED, "token missing jti").into_response();
    };

    let hash = nasiko_auth::jwt::hash_jti(&jti);
    let revoked: bool = match sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM auth_tokens
            WHERE token_hash = $1 AND revoked_at IS NOT NULL
        )",
    )
    .bind(&hash)
    .fetch_one(&state.db)
    .await
    {
        Ok(revoked) => revoked,
        Err(e) => {
            tracing::error!(%e, "revocation lookup failed; failing closed");
            return (StatusCode::UNAUTHORIZED, "token validation unavailable").into_response();
        }
    };

    if revoked {
        return (StatusCode::UNAUTHORIZED, "token revoked").into_response();
    }

    // An agent token (minted by `issue_agent_token`, e.g. injected into an
    // agent's container so it can call other agents) must not pass as a user
    // session on the rest of the API surface — otherwise it's indistinguishable
    // from a real user token everywhere `require_auth` is layered. Only the
    // agent-to-agent calling paths (the direct proxy, and A2A dispatch) accept it.
    if identity.is_agent && !is_agent_reachable_path(req.uri().path()) {
        return (StatusCode::FORBIDDEN, "agent tokens cannot access this endpoint").into_response();
    }

    req.extensions_mut().insert(Claims::from(identity));
    next.run(req).await
}

/// Paths reachable by an agent-typed token: the direct agent proxy
/// (`/agents/{id}/...`) and A2A dispatch (`/orchestrator/a2a`, `/orchestrator/
/// a2a/upload`) — the two routes an agent legitimately uses to call another
/// agent. `require_auth` runs on the router nested under `/api`, so `path` is
/// already stripped of that prefix (mirrors `agent_proxy.rs::parse_agent_path`,
/// which relies on the same stripping).
fn is_agent_reachable_path(path: &str) -> bool {
    path.starts_with("/agents/") || path.starts_with("/orchestrator/a2a")
}

fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    // Prefer Authorization: Bearer <token>
    if let Some(auth) = headers.get(header::AUTHORIZATION)
        && let Ok(value) = auth.to_str()
        && let Some(token) = value.strip_prefix("Bearer ")
    {
        return Some(token.to_string());
    }

    // Fallback: Cookie: access_token=<token>
    if let Some(cookie) = headers.get(header::COOKIE)
        && let Ok(value) = cookie.to_str()
    {
        for part in value.split(';') {
            if let Some(token) = part.trim().strip_prefix("access_token=") {
                return Some(token.to_string());
            }
        }
    }

    None
}

impl<S: Send + Sync> FromRequestParts<S> for Claims {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Claims>()
            .cloned()
            .ok_or((StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}