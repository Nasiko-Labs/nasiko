use axum::{
    extract::{FromRequestParts, Request},
    http::{StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use nasiko_auth::Identity;

use crate::state::GatewayState;

/// Extract identity from a validated request (set by auth middleware).
#[derive(Debug, Clone)]
pub struct AuthIdentity(pub Identity);

impl<S> FromRequestParts<S> for AuthIdentity
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Identity>()
            .cloned()
            .map(AuthIdentity)
            .ok_or(StatusCode::UNAUTHORIZED)
    }
}

/// Auth middleware: validates JWT and injects Identity into request extensions.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<GatewayState>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = extract_token(req.headers());

    let Some(token) = token else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    match state.auth.validate_token(&token).await {
        Ok(identity) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        Err(_) => StatusCode::UNAUTHORIZED.into_response(),
    }
}

fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    // Try Authorization: Bearer <token>
    if let Some(auth) = headers.get(header::AUTHORIZATION)
        && let Ok(value) = auth.to_str()
        && let Some(token) = value.strip_prefix("Bearer ")
    {
        return Some(token.to_string());
    }

    // Try cookie
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
