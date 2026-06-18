use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine;

use crate::AppState;

pub struct AdminAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !header.starts_with("Basic ") {
            return Err((
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", "Basic realm=\"registry\"")],
                "authentication required",
            )
                .into_response());
        }

        let encoded = &header["Basic ".len()..];
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap_or_default();
        let creds = String::from_utf8(decoded).unwrap_or_default();
        let mut parts_iter = creds.splitn(2, ':');
        let username = parts_iter.next().unwrap_or("");
        let password = parts_iter.next().unwrap_or("");

        if username == state.config.admin_username
            && password == state.config.admin_password
        {
            Ok(AdminAuth)
        } else {
            Err((
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", "Basic realm=\"registry\"")],
                "invalid credentials",
            )
                .into_response())
        }
    }
}
