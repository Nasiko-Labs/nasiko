use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::Claims;
use crate::state::AppState;

/// Permission checks are resolved through `AuthService` so the role never has to
/// live on the identity: OSS grants everything (single-user), EE resolves the
/// user's role from the DB by `user_id`.
pub async fn require_deployer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match req.extensions().get::<Claims>() {
        Some(claims) => {
            let identity = claims.clone().into();
            if state.auth.can_deploy(&identity).await {
                next.run(req).await
            } else {
                (StatusCode::FORBIDDEN, "requires deployer role or higher").into_response()
            }
        }
        None => (StatusCode::UNAUTHORIZED, "not authenticated").into_response(),
    }
}

pub async fn require_admin(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match req.extensions().get::<Claims>() {
        Some(claims) => {
            let identity = claims.clone().into();
            if state.auth.can_manage_pool(&identity).await {
                next.run(req).await
            } else {
                (StatusCode::FORBIDDEN, "requires admin role or higher").into_response()
            }
        }
        None => (StatusCode::UNAUTHORIZED, "not authenticated").into_response(),
    }
}

pub async fn require_superuser(req: Request, next: Next) -> Response {
    match req.extensions().get::<Claims>() {
        Some(claims) if claims.is_superuser => next.run(req).await,
        Some(_) => (StatusCode::FORBIDDEN, "requires superuser").into_response(),
        None => (StatusCode::UNAUTHORIZED, "not authenticated").into_response(),
    }
}
