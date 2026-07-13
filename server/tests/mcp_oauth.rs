//! HTTP-level tests for the per-connector OAuth 2.1 management routes' guard
//! paths, plus the public `GET /api/mcp/oauth/callback` browser redirect target
//! (`oss/server/src/mcp/handlers/oauth.rs:46`, `nasiko_mcp_gateway::oauth::handle_callback`).
//!
//!   cargo test -p nasiko-server --test mcp_oauth -- --test-threads=1

mod common;

use axum::{Router, response::IntoResponse, routing::post};
use chrono::{Duration as ChronoDuration, Utc};
use nasiko_mcp_gateway::oauth::{OAuthState, sign_state};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// The signing key used by tests that want deterministic, explicit control over
/// `OAUTH_STATE_SIGNING_KEY` (as opposed to the default-key test below, which
/// deliberately leaves it unset).
const SIGNING_KEY: &str = "test-oauth-signing-key-for-mcp-oauth-rs";

/// The public literal `oss/mcp-gateway/src/config.rs:49` falls back to when
/// `OAUTH_STATE_SIGNING_KEY` is unset — Vuln 2.
const DEFAULT_SIGNING_KEY: &str = "mcp-gateway-state";

fn set_signing_key(key: &str) {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe { std::env::set_var("OAUTH_STATE_SIGNING_KEY", key) };
}
fn clear_signing_key() {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe { std::env::remove_var("OAUTH_STATE_SIGNING_KEY") };
}

/// `handle_callback`'s `exchange_code` step needs `oauth_redirect_uri()` to be
/// `Some` (it's sent as the `redirect_uri` form field) — set via the test
/// harness's `TEST_MCP_GATEWAY_PUBLIC_URL` override (see `common::test_config`).
fn set_gateway_public_url() {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe { std::env::set_var("TEST_MCP_GATEWAY_PUBLIC_URL", "https://gateway.test.local") };
}
fn clear_gateway_public_url() {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe { std::env::remove_var("TEST_MCP_GATEWAY_PUBLIC_URL") };
}

/// A client that does NOT follow redirects, so a 3xx response can be inspected
/// directly instead of chasing `Location` into a real (possibly nonexistent) host.
fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

async fn seed_oauth_connector_with_token_endpoint(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    token_endpoint: &str,
) -> Uuid {
    let cid = seed_connector(server, owner, name, "oauth2").await;
    sqlx::query(
        "UPDATE mcp_connectors SET oauth_token_endpoint = $2, \
         oauth_authorization_endpoint = 'https://as.example.com/authorize', oauth_client_id = 'test-client' \
         WHERE id = $1",
    )
    .bind(cid)
    .bind(token_endpoint)
    .execute(&server.db)
    .await
    .unwrap();
    cid
}

/// A stub OAuth token endpoint returning a fixed token response for any POST.
async fn start_stub_token_server() -> String {
    async fn respond() -> impl IntoResponse {
        axum::Json(json!({"access_token": "at_from_stub", "refresh_token": "rt_from_stub", "expires_in": 3600}))
    }
    let app = Router::new().route("/token", post(respond));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/token")
}

async fn init_admin(server: &common::TestServer) -> (String, Uuid) {
    let v = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let id = v["user_id"].as_str().unwrap().to_string();
    (id.clone(), Uuid::parse_str(&id).unwrap())
}

async fn seed_connector(server: &common::TestServer, owner: Uuid, name: &str, auth_type: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type)
         VALUES ('mcp_server', $1, $2, 'https://example.com', $3) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(auth_type)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn authorize_on_non_oauth_connector_is_bad_request() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "not-oauth", "bearer").await;

    let res = common::as_superuser(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/oauth/authorize"))), &uid, "admin")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400, "OAuth authorize is only for auth_type='oauth2'");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn authorize_without_gateway_url_is_not_configured() {
    // MCP_GATEWAY_PUBLIC_URL is unset in the test env → begin_authorization fails
    // with NotConfigured (503) before attempting discovery.
    // SAFETY: serialized by #[serial].
    unsafe { std::env::remove_var("MCP_GATEWAY_PUBLIC_URL") };
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "oauth-tool", "oauth2").await;

    let res = common::as_superuser(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/oauth/authorize"))), &uid, "admin")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 503);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn status_reports_unauthorized_and_revoke_404_when_no_token() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "oauth-status-tool", "oauth2").await;

    let body: Value = common::as_superuser(server.client.get(server.url(&format!("/api/mcp/connectors/{cid}/oauth/status"))), &uid, "admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["authorized"], false);

    let res = common::as_superuser(server.client.delete(server.url(&format!("/api/mcp/connectors/{cid}/oauth/token"))), &uid, "admin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404, "no token to revoke");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn authorize_on_inaccessible_connector_forbidden() {
    let server = common::TestServer::start().await;
    let (admin, _) = init_admin(&server).await;
    // A connector owned by someone else.
    let other = common::as_superuser(server.client.post(server.url("/api/users")), &admin, "admin")
        .json(&json!({"username": "oa-other", "email": "oa-other@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let other_uuid = Uuid::parse_str(other["id"].as_str().unwrap()).unwrap();
    let cid = seed_connector(&server, other_uuid, "other-oauth", "oauth2").await;

    // A plain member (not admin, not owner, no grant) is denied.
    let member = common::as_superuser(server.client.post(server.url("/api/users")), &admin, "admin")
        .json(&json!({"username": "oa-member", "email": "oa-member@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(server.client.get(server.url(&format!("/api/mcp/connectors/{cid}/oauth/status"))), member_id, "oa-member")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

// ─── GET /api/mcp/oauth/callback ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn callback_round_trips_through_token_exchange() {
    set_signing_key(SIGNING_KEY);
    set_gateway_public_url();
    let token_url = start_stub_token_server().await;
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid = seed_oauth_connector_with_token_endpoint(&server, uuid, "cb-tool", &token_url).await;

    // Same-origin as the configured gateway public URL, so fix #3's
    // `safe_redirect` honors it verbatim (an off-origin target would be
    // rewritten to "/" — covered by the connect.rs neutralization test).
    let oauth_state = OAuthState::new(uuid, cid, "verifier123".into(), Some("https://gateway.test.local/success".into()));
    let signed = sign_state(&oauth_state, SIGNING_KEY);

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123"), ("state", signed.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 303, "successful exchange must redirect to the caller-supplied redirect_url");
    assert_eq!(
        res.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("https://gateway.test.local/success")
    );

    let (status, enc): (String, Option<String>) = sqlx::query_as(
        "SELECT status, encrypted_credential FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2",
    )
    .bind(uuid)
    .bind(cid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(status, "ACTIVE");
    assert!(enc.is_some(), "the exchanged access token must be persisted");
    assert_ne!(enc.unwrap(), "at_from_stub", "the token must be encrypted at rest, not stored raw");

    clear_signing_key();
    clear_gateway_public_url();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn callback_missing_state_is_message_page_not_redirect() {
    set_signing_key(SIGNING_KEY);
    let server = common::TestServer::start().await;

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123")])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "missing state must render the error page, not redirect");
    let body = res.text().await.unwrap();
    assert!(body.contains("Missing code or state"), "{body}");

    clear_signing_key();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn callback_malformed_state_is_clean_error_not_panic() {
    set_signing_key(SIGNING_KEY);
    let server = common::TestServer::start().await;

    for garbage in ["not-base64-!!!@@@", "", "a.b.c", &"X".repeat(4000)] {
        let res = no_redirect_client()
            .get(server.url("/api/mcp/oauth/callback"))
            .query(&[("code", "authcode123"), ("state", garbage)])
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "garbage state {garbage:?} must be a clean error page, not a panic/500");
        let body = res.text().await.unwrap();
        assert!(body.contains("Invalid or expired state"), "{garbage:?} -> {body}");
    }

    clear_signing_key();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn callback_expired_state_is_rejected() {
    set_signing_key(SIGNING_KEY);
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "exp-tool", "oauth2").await;

    let mut oauth_state = OAuthState::new(uuid, cid, "verifier123".into(), None);
    oauth_state.exp = (Utc::now() - ChronoDuration::minutes(1)).timestamp();
    let signed = sign_state(&oauth_state, SIGNING_KEY);

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123"), ("state", signed.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "expired state must render the error page, not redirect");
    let body = res.text().await.unwrap();
    assert!(body.contains("Invalid or expired state"), "{body}");

    clear_signing_key();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn callback_idp_error_renders_message_not_redirect() {
    set_signing_key(SIGNING_KEY);
    let server = common::TestServer::start().await;

    // The user denied consent at the IdP — no code/state at all, just error params.
    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("error", "access_denied"), ("error_description", "User denied access")])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "an IdP error must render the error page, not redirect or crash");
    let body = res.text().await.unwrap();
    assert!(body.contains("Authorization failed"), "{body}");
    assert!(body.contains("access_denied"), "{body}");

    clear_signing_key();
    server.cleanup().await;
}

/// ── Vuln 2 (High) regression guard — FIXED in `oss/mcp-gateway/src/config.rs` ──
///
/// The signing key used to silently default to the public literal
/// `"mcp-gateway-state"` when `OAUTH_STATE_SIGNING_KEY` was unset, letting an
/// attacker forge a valid `state` for any (user, connector). Fix #2 removed that
/// fallback: the key derives from `JWT_SECRET` (domain-separated) when the
/// dedicated var is unset — never a shipped constant. This reproduces the old
/// exploit (unset the dedicated var, forge with the published default key) and
/// confirms the forged state is now REJECTED before any token exchange.
#[tokio::test]
#[serial]
async fn callback_rejects_forged_state_signed_with_old_default_key_vuln2() {
    clear_signing_key(); // unset the dedicated var — key now derives from JWT_SECRET
    set_gateway_public_url();
    let token_url = start_stub_token_server().await;
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid = seed_oauth_connector_with_token_endpoint(&server, uuid, "vuln2-tool", &token_url).await;

    // The old published default literal — no longer the real signing key.
    let forged_state = OAuthState::new(uuid, cid, "attacker-chosen-verifier".into(), Some("https://attacker.example/land".into()));
    let signed = sign_state(&forged_state, DEFAULT_SIGNING_KEY);

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123"), ("state", signed.as_str())])
        .send()
        .await
        .unwrap();

    // State fails HMAC verification against the JWT-derived key → error page,
    // no redirect, no token exchange.
    assert_eq!(res.status(), 200, "a state forged with the old default key must be rejected, not honored");
    let location = res.headers().get("location").and_then(|v| v.to_str().ok());
    assert_ne!(location, Some("https://attacker.example/land"), "forged state must never drive a redirect");
    let body = res.text().await.unwrap();
    assert!(body.contains("Invalid or expired state"), "{body}");

    // Nothing should have been persisted for the forged (user, connector).
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2 AND encrypted_credential IS NOT NULL",
    )
    .bind(uuid)
    .bind(cid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(count, 0, "no token may be stored from a rejected forged state");

    clear_gateway_public_url();
    server.cleanup().await;
}
