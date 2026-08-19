use axum::{
    Json,
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::Claims;
use crate::state::AppState;

/// A 401 rejection that returns the standard JSON envelope instead of plain text.
pub struct AuthRejection(pub StatusCode, pub &'static str);

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let code = self.0.as_u16();
        (
            self.0,
            Json(json!({
                "data": serde_json::Value::Null,
                "status_code": code,
                "message": self.1,
            })),
        )
            .into_response()
    }
}

/// Auth middleware — validates the JWT from Authorization: Bearer or access_token cookie.
///
/// No gateway required: the server validates tokens directly via AuthService.
/// Revocation is enforced via an O(1) indexed lookup on auth_tokens.token_hash.
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let claims = match validate_bearer(&state, req.headers()).await {
        Ok(c) => c,
        Err((status, message)) => return AuthRejection(status, message).into_response(),
    };
    req.extensions_mut().insert(claims);
    next.run(req).await
}

/// A frontend served under a path prefix, with its own login behavior.
///
/// The page gate ([`require_page_auth`]) redirects unauthenticated page
/// navigations to the login page of the mount that owns the requested path,
/// so each frontend keeps its own sign-in flow. Mounts are wired once at the
/// composition root (`AppState.ui_mounts`); OSS serves only [`UiMount::ROOT`],
/// EE adds the Flutter app mount at `/app/`.
#[derive(Clone, Copy, Debug)]
pub struct UiMount {
    /// Path prefix owning the mount, with a trailing slash (`"/"`, `"/app/"`).
    /// The bare prefix without the slash (`/app`) belongs to the mount too.
    pub prefix: &'static str,
    /// The mount's login page — its one ungated page and the redirect target.
    ///
    /// `None` means the mount's pages are never gated server-side. Use this
    /// for a SPA that enforces auth client-side: its HTML shell is a static
    /// bootloader with no user data, and its SSO flows land on it with the
    /// session token in the URL (`?token=` / `#token=`), a handoff a
    /// server-side redirect would destroy.
    pub login_path: Option<&'static str>,
}

impl UiMount {
    /// The vanilla-JS UI at `/` — every edition serves it.
    pub const ROOT: UiMount = UiMount {
        prefix: "/",
        login_path: Some("/login.html"),
    };
}

/// Server-side gate for UI **page navigations** (the static-asset fallback).
///
/// HTML documents (and extensionless paths, which the static handler resolves
/// to HTML) are only served to callers with a valid session — everyone else
/// gets a redirect to the owning mount's login page, so unauthenticated users
/// never see a page render at all. Subresource assets (js/css/fonts/svg) stay
/// public: login pages need them, and they contain no user data.
pub async fn require_page_auth(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    match login_redirect_target(state.ui_mounts, req.uri().path()) {
        Some(login) if validate_bearer(&state, req.headers()).await.is_err() => {
            axum::response::Redirect::to(login).into_response()
        }
        _ => next.run(req).await,
    }
}

/// The login page to redirect `path` to when the caller has no session —
/// `None` when the request needs no session (an asset, a login page, or any
/// path under an ungated mount).
fn login_redirect_target(mounts: &'static [UiMount], path: &str) -> Option<&'static str> {
    let login = mount_for(mounts, path).login_path?;
    if path == login || !is_gated_page(path) {
        return None;
    }
    Some(login)
}

/// The mount owning `path`: the longest matching prefix, falling back to the
/// root mount. `/app` (no trailing slash) belongs to the `/app/` mount.
fn mount_for(mounts: &'static [UiMount], path: &str) -> UiMount {
    mounts
        .iter()
        .filter(|m| path.starts_with(m.prefix) || m.prefix.strip_suffix('/') == Some(path))
        .max_by_key(|m| m.prefix.len())
        .copied()
        .unwrap_or(UiMount::ROOT)
}

/// A path is a gated page when it serves an HTML document: explicit `.html`
/// paths and extensionless paths (`/`, `/app/`, unknown routes → 404 page).
/// Login pages are exempted by [`login_redirect_target`], not here.
fn is_gated_page(path: &str) -> bool {
    if path.ends_with(".html") {
        return true;
    }
    let last_segment = path.rsplit('/').next().unwrap_or("");
    !last_segment.contains('.')
}

/// The bearer-token validation core of [`require_auth`], extracted so other
/// mount points that need to accept a bearer token as ONE of several auth
/// methods (e.g. the OCI registry's Basic-auth-or-bearer mount, see
/// `lib.rs`'s `authenticate_oci_request`) can reuse it without going through
/// the all-or-nothing `middleware::from_fn` wrapper.
///
/// `pub`, not `pub(crate)`: this is the seam for out-of-crate mounts that sit in
/// front of OSS routes and must authenticate before OSS middleware would.
/// EE's catalog interceptor (`ee/server/src/catalog.rs`) is one, and it used to
/// carry its own transcription of this function — which then silently missed
/// every rule added here, the caller-still-exists check below being the case
/// that exposed it. Any new mount calls this; nothing re-implements it.
pub async fn validate_bearer(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<Claims, (StatusCode, &'static str)> {
    let Some(token) = extract_token(headers) else {
        return Err((StatusCode::UNAUTHORIZED, "missing or invalid token"));
    };
    validate_session_token(state, &token).await
}

/// [`validate_bearer`] minus the header extraction, for the one caller that
/// receives a session token somewhere other than a header: the marketplace SSO
/// landing endpoint (`login::sso_session`), which is handed one in the URL.
/// Split out rather than duplicated so both paths keep applying the same
/// revocation and caller-still-exists rules — the exact drift `validate_bearer`
/// was consolidated to prevent.
pub async fn validate_session_token(
    state: &AppState,
    token: &str,
) -> Result<Claims, (StatusCode, &'static str)> {
    let identity = match state.auth.validate_token(token).await {
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
    let jti = nasiko_auth::jwt::extract_jti(token).filter(|j| !j.is_empty());
    let Some(jti) = jti else {
        return Err((StatusCode::UNAUTHORIZED, "token missing jti"));
    };

    // The caller's own row is checked in the same round trip. A token can be
    // signature-valid, unexpired and unrevoked while naming a user that no
    // longer exists — the database was recreated under it (a squashed migration
    // set, a restored dump), or the row was hard-deleted. Every handler that
    // then looks the caller up fails at a different depth: `fetch_one` on
    // `users` is a 500, and an insert into anything with a `user_id` foreign key
    // (chat_sessions) is a 500 too, so the app reads as broken rather than as
    // logged out. Rejecting here turns all of that into the one thing the
    // frontend already knows how to handle — a 401 sends it to /login.html
    // through the single funnel in common/services/api.js.
    //
    // `is_active` is deliberately NOT part of this: deactivating a user would
    // then kill their live sessions, which is a policy change, not a fix.
    let caller_id = identity.user_id.parse::<uuid::Uuid>().ok();
    let hash = nasiko_auth::jwt::hash_jti(&jti);
    let (revoked, caller_exists): (bool, bool) = match sqlx::query_as(
        "SELECT
            EXISTS(
                SELECT 1 FROM auth_tokens
                WHERE token_hash = $1 AND revoked_at IS NOT NULL
            ),
            EXISTS(
                SELECT 1 FROM users
                WHERE id = $2 AND deleted_at IS NULL
            )",
    )
    .bind(&hash)
    .bind(caller_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(%e, "revocation lookup failed; failing closed");
            return Err((StatusCode::UNAUTHORIZED, "token validation unavailable"));
        }
    };

    if revoked {
        return Err((StatusCode::UNAUTHORIZED, "token revoked"));
    }

    // Only enforced for an identity that names a UUID user: `caller_exists` is
    // false whenever the bind was NULL, and an identity whose `user_id` is not a
    // UUID has no row to find in the first place.
    if caller_id.is_some() && !caller_exists {
        tracing::warn!(
            user_id = %identity.user_id,
            "session token names a user that no longer exists; treating as logged out"
        );
        return Err((StatusCode::UNAUTHORIZED, "session user no longer exists"));
    }

    // Agent-typed tokens (minted by `issue_agent_token`) never reach this
    // point at all — `state.auth.validate_token` above already rejects them
    // via `decode_jwt`/`decode_jwt_with_jti`'s `token_type` check (AUTH-3), so
    // every `identity` here is guaranteed to be a real user session.
    Ok(Claims::from(identity))
}

pub(crate) fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
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
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Claims>()
            .cloned()
            .ok_or(AuthRejection(StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}

#[cfg(test)]
mod tests {
    use super::{UiMount, is_gated_page, login_redirect_target};

    /// OSS wiring: the root mount only.
    const ROOT_ONLY: &[UiMount] = &[UiMount::ROOT];

    /// EE wiring: vanilla UI at `/` plus the ungated Flutter app at `/app/`.
    const WITH_APP: &[UiMount] = &[
        UiMount::ROOT,
        UiMount {
            prefix: "/app/",
            login_path: None,
        },
    ];

    #[test]
    fn gates_html_documents_and_extensionless_paths() {
        assert!(is_gated_page("/"));
        assert!(is_gated_page("/index.html"));
        assert!(is_gated_page("/agents.html"));
        assert!(is_gated_page("/app/"));
        assert!(is_gated_page("/unknown-route"));
    }

    #[test]
    fn passes_subresource_assets() {
        assert!(!is_gated_page("/common/global.css"));
        assert!(!is_gated_page("/navigation.js"));
        assert!(!is_gated_page("/common/mark-nasiko.svg"));
        assert!(!is_gated_page("/common/fonts/departure-mono.woff2"));
    }

    #[test]
    fn root_mount_redirects_pages_to_vanilla_login() {
        assert_eq!(login_redirect_target(ROOT_ONLY, "/"), Some("/login.html"));
        assert_eq!(
            login_redirect_target(ROOT_ONLY, "/agents.html"),
            Some("/login.html")
        );
        assert_eq!(login_redirect_target(ROOT_ONLY, "/login.html"), None);
        assert_eq!(login_redirect_target(ROOT_ONLY, "/common/global.css"), None);
        // Without an /app/ mount, its pages belong to the root mount.
        assert_eq!(
            login_redirect_target(ROOT_ONLY, "/app/"),
            Some("/login.html")
        );
    }

    #[test]
    fn ungated_app_mount_serves_pages_without_a_session() {
        // The Flutter SPA gates itself client-side, and its SSO callbacks
        // land here with the token in the URL — no server-side redirect.
        assert_eq!(login_redirect_target(WITH_APP, "/app/"), None);
        assert_eq!(login_redirect_target(WITH_APP, "/app"), None);
        assert_eq!(login_redirect_target(WITH_APP, "/app/login"), None);
        assert_eq!(login_redirect_target(WITH_APP, "/app/auth/callback"), None);
        assert_eq!(
            login_redirect_target(WITH_APP, "/app/agents/some-uuid"),
            None
        );
        assert_eq!(login_redirect_target(WITH_APP, "/app/main.dart.js"), None);
        // Root-mount pages still go to the vanilla login.
        assert_eq!(
            login_redirect_target(WITH_APP, "/agents.html"),
            Some("/login.html")
        );
        assert_eq!(login_redirect_target(WITH_APP, "/login.html"), None);
    }
}
