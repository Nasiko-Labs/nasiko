use axum::{
    extract::{ FromRequestParts, Request },
    http::{ StatusCode, header, request::Parts },
    middleware::Next,
    response::{ IntoResponse, Response },
};

use nasiko_auth::{Identity, TRUST_HEADERS};

use crate::state::GatewayState;

/// Extract identity from a validated request (set by auth middleware).
#[derive(Debug, Clone)]
pub struct AuthIdentity(pub Identity);

impl<S> FromRequestParts<S> for AuthIdentity where S: Send + Sync {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions
            .get::<Identity>()
            .cloned()
            .map(AuthIdentity)
            .ok_or(StatusCode::UNAUTHORIZED)
    }
}

/// Auth middleware: validates JWT, checks revocation, injects Identity.
///
/// Security contract:
/// 1. Strip any client-supplied X-User-* trust headers first — the gateway is
///    the only issuer. This prevents identity spoofing even if the server is
///    somehow reachable directly.
/// 2. Extract the Bearer token or access_token cookie.
/// 3. Validate the JWT signature and expiry via AuthService.
/// 4. Check the JTI against auth_tokens for revocation (makes logout instant).
/// 5. Inject verified Identity into request extensions for downstream handlers.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<GatewayState>,
    mut req: Request,
    next: Next
) -> Response {
    // Step 1: strip trust headers — no client may forge identity
    for h in TRUST_HEADERS {
        req.headers_mut().remove(*h);
    }

    // Step 2: extract token
    let Some(token) = extract_token(req.headers()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    // Step 3: validate signature + expiry
    let identity = match state.auth.validate_token(&token).await {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Step 4: revocation check — O(1) indexed lookup on token_hash
    if let Some(jti) = nasiko_auth::jwt::extract_jti(&token) {
        let hash = nasiko_auth::jwt::hash_jti(&jti);
        let revoked: Result<bool, _> = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM auth_tokens
                WHERE token_hash = $1 AND revoked_at IS NOT NULL
            )",
        )
        .bind(&hash)
        .fetch_one(&state.db)
        .await;

        // Fail CLOSED: a DB error during the revocation lookup must deny, never
        // default to "not revoked" (AUTH-5) — otherwise a transient DB blip lets
        // revoked/logged-out tokens through.
        match revoked {
            Ok(true) => return StatusCode::UNAUTHORIZED.into_response(),
            Ok(false) => {}
            Err(e) => {
                tracing::error!(%e, "gateway: revocation lookup failed — denying (fail-closed)");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
    }

    // Step 5: inject verified identity
    req.extensions_mut().insert(identity);
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
