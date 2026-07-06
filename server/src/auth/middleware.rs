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

    // Revocation check — O(1) indexed lookup on token_hash
    if let Some(jti) = nasiko_auth::jwt::extract_jti(&token) {
        let hash = nasiko_auth::jwt::hash_jti(&jti);
        let revoked: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM auth_tokens
                WHERE token_hash = $1 AND revoked_at IS NOT NULL
            )",
        )
        .bind(&hash)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);

        if revoked {
            return (StatusCode::UNAUTHORIZED, "token revoked").into_response();
        }
    }

    req.extensions_mut().insert(Claims::from(identity));
    next.run(req).await
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