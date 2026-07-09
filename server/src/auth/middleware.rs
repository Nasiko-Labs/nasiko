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
    let claims = match validate_bearer(&state, req.headers()).await {
        Ok(c) => c,
        Err((status, message)) => return (status, message).into_response(),
    };
    req.extensions_mut().insert(claims);
    next.run(req).await
}

/// The bearer-token validation core of [`require_auth`], extracted so other
/// mount points that need to accept a bearer token as ONE of several auth
/// methods (e.g. the OCI registry's Basic-auth-or-bearer mount, see
/// `lib.rs`'s `authenticate_oci_request`) can reuse it without going through
/// the all-or-nothing `middleware::from_fn` wrapper.
pub(crate) async fn validate_bearer(state: &AppState, headers: &axum::http::HeaderMap) -> Result<Claims, (StatusCode, &'static str)> {
    let Some(token) = extract_token(headers) else {
        return Err((StatusCode::UNAUTHORIZED, "missing or invalid token"));
    };

    let identity = match state.auth.validate_token(&token).await {
        Ok(id) => id,
        Err(_) => return Err((StatusCode::UNAUTHORIZED, "invalid token")),
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
        return Err((StatusCode::UNAUTHORIZED, "token missing jti"));
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
            return Err((StatusCode::UNAUTHORIZED, "token validation unavailable"));
        }
    };

    if revoked {
        return Err((StatusCode::UNAUTHORIZED, "token revoked"));
    }

    // Agent-typed tokens (minted by `issue_agent_token`) never reach this
    // point at all — `state.auth.validate_token` above already rejects them
    // via `decode_jwt`/`decode_jwt_with_jti`'s `token_type` check (AUTH-3), so
    // every `identity` here is guaranteed to be a real user session.
    Ok(Claims::from(identity))
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