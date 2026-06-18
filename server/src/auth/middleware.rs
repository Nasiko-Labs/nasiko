use axum::{
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::Claims;
use crate::state::AppState;

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| extract_cookie(&req, "access_token"));

    let token = match token {
        Some(t) => t,
        None => return (StatusCode::UNAUTHORIZED, "missing or invalid authorization").into_response(),
    };

    match state.providers.auth.validate_token(token) {
        Ok(auth_claims) => {
            req.extensions_mut().insert(Claims::from(auth_claims));
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
    }
}

fn extract_cookie<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .map(|c| c.trim())
                .find(|c| c.starts_with(name) && c.as_bytes().get(name.len()) == Some(&b'='))
                .map(|c| &c[name.len() + 1..])
        })
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
