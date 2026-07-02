use axum::{
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use nasiko_auth::{
    Identity, Role,
    HEADER_USER_ID, HEADER_USERNAME, HEADER_IS_SUPERUSER, HEADER_USER_ROLE,
};

use super::Claims;
use crate::state::AppState;

/// Auth middleware for the server.
///
/// Reads identity exclusively from gateway-injected X-User-* headers.
/// The gateway validates JWTs and strips any client-supplied X-User-* headers
/// before injecting its own, so the server can unconditionally trust them.
pub async fn require_auth(
    _state: State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(user_id) = req.headers().get(HEADER_USER_ID).and_then(|v| v.to_str().ok()).map(str::to_owned) else {
        return (StatusCode::UNAUTHORIZED, "missing identity headers").into_response();
    };

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

    let identity = Identity {
        user_id,
        username,
        is_superuser,
        role,
    };

    req.extensions_mut().insert(Claims::from(identity));
    next.run(req).await
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
