use axum::{
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use nasiko_auth::{
    Identity, Role,
    HEADER_USER_ID, HEADER_USERNAME, HEADER_IS_SUPERUSER, HEADER_USER_ROLE,
    HEADER_TEAM_ID, HEADER_DEPT_ID,
};

use super::Claims;
use crate::state::AppState;

/// Auth middleware for the server.
///
/// Primary path (behind gateway): reads identity from trusted forwarded headers.
/// Fallback path (dev / direct access): validates JWT via AuthProvider.
///
/// The gateway strips incoming client X-User-* headers before injecting its own,
/// so the server can trust these headers when present.
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    // Primary: trust gateway-injected identity headers
    if let Some(user_id) = req.headers().get(HEADER_USER_ID).and_then(|v| v.to_str().ok()).map(str::to_owned) {
        let username = req.headers().get(HEADER_USERNAME)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();

        let is_superuser = req.headers().get(HEADER_IS_SUPERUSER)
            .and_then(|v| v.to_str().ok())
            == Some("true");

        let role: Option<Role> = req.headers().get(HEADER_USER_ROLE)
            .and_then(|v| v.to_str().ok())
            .and_then(|r| serde_json::from_value(serde_json::Value::String(r.to_owned())).ok());

        let team_id = req.headers().get(HEADER_TEAM_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let department_id = req.headers().get(HEADER_DEPT_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let identity = Identity {
            user_id: user_id.clone(),
            sub: user_id,
            username,
            is_superuser,
            role,
            team_id,
            department_id,
            exp: 0,
            iat: 0,
        };

        req.extensions_mut().insert(Claims::from(identity));
        return next.run(req).await;
    }

    // Fallback: direct JWT validation (dev mode or gateway-less deployment)
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

    match state.providers.auth.validate_token(token).await {
        Ok(identity) => {
            req.extensions_mut().insert(Claims::from(identity));
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