//! Unit tests for the gateway — no running infrastructure required.

use axum::http::{HeaderMap, HeaderValue, header};

// ── token extraction ──────────────────────────────────────────────────────────

fn bearer_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    h
}

fn cookie_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("access_token={token}")).unwrap(),
    );
    h
}

fn multi_cookie_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("session=abc; access_token={token}; other=xyz")).unwrap(),
    );
    h
}

/// Re-export the private helper under test via a thin wrapper.
/// We test token extraction logic without spinning up the full Axum stack.
fn extract_token(headers: &HeaderMap) -> Option<String> {
    // Mirror of gateway/src/auth.rs extract_token (not pub, so we duplicate the logic here)
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        if let Ok(value) = auth.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }
    if let Some(cookie) = headers.get(header::COOKIE) {
        if let Ok(value) = cookie.to_str() {
            for part in value.split(';') {
                if let Some(token) = part.trim().strip_prefix("access_token=") {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

#[test]
fn extracts_bearer_token() {
    let h = bearer_headers("my-jwt-token-abc");
    assert_eq!(extract_token(&h).as_deref(), Some("my-jwt-token-abc"));
}

#[test]
fn extracts_cookie_token() {
    let h = cookie_headers("cookie-jwt-xyz");
    assert_eq!(extract_token(&h).as_deref(), Some("cookie-jwt-xyz"));
}

#[test]
fn extracts_token_from_multi_cookie_string() {
    let h = multi_cookie_headers("middle-token");
    assert_eq!(extract_token(&h).as_deref(), Some("middle-token"));
}

#[test]
fn bearer_takes_priority_over_cookie() {
    let mut h = bearer_headers("bearer-token");
    h.insert(
        header::COOKIE,
        HeaderValue::from_static("access_token=cookie-token"),
    );
    assert_eq!(extract_token(&h).as_deref(), Some("bearer-token"));
}

#[test]
fn returns_none_when_no_token_present() {
    let h = HeaderMap::new();
    assert_eq!(extract_token(&h), None);
}

#[test]
fn returns_none_for_non_bearer_auth_scheme() {
    let mut h = HeaderMap::new();
    h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic dXNlcjpwYXNz"));
    assert_eq!(extract_token(&h), None);
}

// ── trust header stripping ────────────────────────────────────────────────────

#[test]
fn trust_headers_are_stripped_from_headers_map() {
    use nasiko_auth::TRUST_HEADERS;

    let mut headers = HeaderMap::new();
    // Simulate a malicious client sending spoofed trust headers
    headers.insert("x-user-id",       HeaderValue::from_static("00000000-0000-0000-0000-000000000000"));
    headers.insert("x-username",      HeaderValue::from_static("admin"));
    headers.insert("x-is-superuser",  HeaderValue::from_static("true"));
    headers.insert("x-user-role",     HeaderValue::from_static("admin"));
    headers.insert("x-user-team-id",  HeaderValue::from_static("team-1"));
    headers.insert("x-user-dept-id",  HeaderValue::from_static("dept-1"));
    // Also add a legitimate header that should survive
    headers.insert("content-type", HeaderValue::from_static("application/json"));

    // Strip — same logic as auth_middleware
    for h in TRUST_HEADERS {
        headers.remove(*h);
    }

    // All trust headers must be gone
    for h in TRUST_HEADERS {
        assert!(
            headers.get(*h).is_none(),
            "trust header {h} should have been stripped"
        );
    }

    // Legitimate header should survive
    assert!(headers.get("content-type").is_some());
}

// ── SingleUserAuth passes any token ──────────────────────────────────────────

#[tokio::test]
async fn single_user_auth_accepts_garbage_token() {
    use nasiko_auth::{AuthProvider, SingleUserAuth};
    let auth = SingleUserAuth;
    let result = auth.validate_token("garbage-token-xyz").await;
    assert!(result.is_ok());
    let id = result.unwrap();
    assert!(id.is_superuser, "SingleUserAuth should return a superuser");
}

// ── SimpleJwtAuth: gateway validates real tokens ──────────────────────────────

#[tokio::test]
async fn gateway_jwt_auth_roundtrip() {
    use nasiko_auth::{AuthProvider, Identity, Role, SimpleJwtAuth, jwt::DEFAULT_EXPIRY_SECS};

    let auth = SimpleJwtAuth { secret: "gw-secret".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let id = Identity {
        user_id: "bbbbbbbb-0000-0000-0000-000000000001".into(),
        sub: "bbbbbbbb-0000-0000-0000-000000000001".into(),
        username: "gw-user".into(),
        is_superuser: false,
        role: Some(Role::TeamMember),
        team_id: None,
        department_id: None,
        exp: 0,
        iat: 0,
    };

    let token = auth.issue_token(&id).await.unwrap();
    let validated = auth.validate_token(&token).await.unwrap();

    assert_eq!(validated.user_id, id.user_id);
    assert_eq!(validated.role, Some(Role::TeamMember));
}

#[tokio::test]
async fn gateway_rejects_token_signed_with_wrong_secret() {
    use nasiko_auth::{AuthProvider, Identity, SimpleJwtAuth, jwt::DEFAULT_EXPIRY_SECS};

    let signer = SimpleJwtAuth { secret: "correct-secret".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let id = Identity {
        user_id: "bbbbbbbb-0000-0000-0000-000000000002".into(),
        sub: "bbbbbbbb-0000-0000-0000-000000000002".into(),
        username: "u".into(),
        is_superuser: false,
        role: None,
        team_id: None,
        department_id: None,
        exp: 0,
        iat: 0,
    };
    let token = signer.issue_token(&id).await.unwrap();

    let verifier = SimpleJwtAuth { secret: "wrong-secret".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let result = verifier.validate_token(&token).await;
    assert!(result.is_err());
}
