use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use nasiko_auth::Role;
use super::Claims;

pub async fn require_deployer(req: Request, next: Next) -> Response {
    match req.extensions().get::<Claims>() {
        Some(claims) if claims.is_superuser => next.run(req).await,
        Some(claims) => match &claims.role {
            Some(role) if *role >= Role::TeamMember => next.run(req).await,
            _ => (StatusCode::FORBIDDEN, "requires deployer role or higher").into_response(),
        },
        None => (StatusCode::UNAUTHORIZED, "not authenticated").into_response(),
    }
}

pub async fn require_admin(req: Request, next: Next) -> Response {
    match req.extensions().get::<Claims>() {
        Some(claims) if claims.is_superuser => next.run(req).await,
        Some(claims) => match &claims.role {
            Some(role) if *role >= Role::Admin => next.run(req).await,
            _ => (StatusCode::FORBIDDEN, "requires admin role or higher").into_response(),
        },
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
