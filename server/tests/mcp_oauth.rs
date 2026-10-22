//! HTTP-level tests for the per-connector OAuth 2.1 management routes' guard
//! paths, plus the public `GET /api/mcp/oauth/callback` browser redirect target
//! (`oss/server/src/mcp/handlers/oauth.rs:46`, `nasiko_mcp_gateway::oauth::handle_callback`).
//!
//!   cargo test -p nasiko-server --test mcp_oauth -- --test-threads=1

mod common;

use std::collections::HashMap;

use axum::{
    Router,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{Duration as ChronoDuration, Utc};
use nasiko_mcp_gateway::oauth::{OAuthState, sign_state};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// The signing key used by tests that want deterministic, explicit control over
/// `OAUTH_STATE_SIGNING_KEY` (as opposed to the default-key test below, which
/// deliberately leaves it unset).
const SIGNING_KEY: &str = "test-oauth-signing-key-for-mcp-oauth-rs";

fn allow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
}
fn disallow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
}

/// A real, live MCP backend that answers any JSON-RPC call with an empty
/// `tools/list` result — needed so that a successful OAuth exchange's
/// follow-up live verification call (`verify_connector_live`) actually
/// succeeds, instead of hitting the connector's placeholder resource URL.
async fn start_stub_mcp_server_ok() -> String {
    async fn respond() -> impl IntoResponse {
        axum::Json(json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}}))
    }
    let app = Router::new().route("/", post(respond));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/")
}

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
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
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
        axum::Json(
            json!({"access_token": "at_from_stub", "refresh_token": "rt_from_stub", "expires_in": 3600}),
        )
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

async fn seed_connector(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    auth_type: &str,
) -> Uuid {
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

/// An Entra-shaped protected MCP server: `POST /mcp` answers 401 pointing at
/// resource metadata whose issuer publishes only OpenID Connect discovery
/// (no RFC 8414 document). Returns the MCP URL.
async fn start_stub_openid_connect_protected_mcp() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let challenge =
        format!(r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp""#);
    let resource_metadata = json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [format!("{base}/tenant/v2.0")],
        "scopes_supported": ["api://app/.default"],
    });
    let openid_configuration = json!({
        "authorization_endpoint": "https://idp.test.local/authorize",
        "token_endpoint": "https://idp.test.local/token",
        "scopes_supported": ["openid", "offline_access"],
    });
    let app = Router::new()
        .route(
            "/mcp",
            post(move || {
                let challenge = challenge.clone();
                async move {
                    (
                        StatusCode::UNAUTHORIZED,
                        [(header::WWW_AUTHENTICATE, challenge)],
                    )
                }
            }),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(move || {
                let document = resource_metadata.clone();
                async move { axum::Json(document) }
            }),
        )
        .route(
            "/tenant/v2.0/.well-known/openid-configuration",
            get(move || {
                let document = openid_configuration.clone();
                async move { axum::Json(document) }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("{base}/mcp")
}

/// POST `/oauth/authorize` as the admin and return the authorization URL's
/// host plus its query parameters.
async fn authorize_url_query(
    server: &common::TestServer,
    uid: &str,
    cid: Uuid,
) -> (String, HashMap<String, String>) {
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/oauth/authorize"))),
        uid,
        "admin",
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let url = reqwest::Url::parse(
        body["data"]["authorization_url"]
            .as_str()
            .expect("authorization_url in response"),
    )
    .expect("authorization_url parses");
    let host = url.host_str().unwrap_or_default().to_string();
    (host, url.query_pairs().into_owned().collect())
}

#[tokio::test]
#[serial]
async fn authorize_against_openid_connect_only_issuer_requests_scope_and_keeps_client_secret() {
    allow_private_urls();
    set_gateway_public_url();
    set_signing_key(SIGNING_KEY);
    let mcp_url = start_stub_openid_connect_protected_mcp().await;
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "entra-protected", "oauth2").await;
    sqlx::query(
        "UPDATE mcp_connectors SET url = $2, oauth_client_id = 'entra-app', \
         oauth_client_secret = 'encrypted-entra-secret' WHERE id = $1",
    )
    .bind(cid)
    .bind(&mcp_url)
    .execute(&server.db)
    .await
    .unwrap();

    let (host, query) = authorize_url_query(&server, &uid, cid).await;

    assert_eq!(host, "idp.test.local");
    assert_eq!(
        query.get("client_id").map(String::as_str),
        Some("entra-app")
    );
    assert_eq!(
        query.get("scope").map(String::as_str),
        Some("api://app/.default offline_access")
    );
    assert!(
        !query.contains_key("resource"),
        "an OpenID Connect-only issuer (Entra ID) rejects a mismatched resource"
    );
    let (token_endpoint, client_secret): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT oauth_token_endpoint, oauth_client_secret FROM mcp_connectors WHERE id = $1",
    )
    .bind(cid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        token_endpoint.as_deref(),
        Some("https://idp.test.local/token")
    );
    assert_eq!(
        client_secret.as_deref(),
        Some("encrypted-entra-secret"),
        "the user's client secret must survive first authorization"
    );

    server.cleanup().await;
    clear_signing_key();
    clear_gateway_public_url();
    disallow_private_urls();
}

#[tokio::test]
#[serial]
async fn authorize_on_configured_connector_falls_back_when_discovery_fails() {
    allow_private_urls();
    set_gateway_public_url();
    set_signing_key(SIGNING_KEY);
    // Answers 200 to `initialize`, so discovery fails with "expected 401".
    let mcp_url = start_stub_mcp_server_ok().await;
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_oauth_connector_with_token_endpoint(
        &server,
        uuid,
        "configured-oauth",
        "https://as.example.com/token",
    )
    .await;
    sqlx::query("UPDATE mcp_connectors SET url = $2 WHERE id = $1")
        .bind(cid)
        .bind(&mcp_url)
        .execute(&server.db)
        .await
        .unwrap();

    let (host, query) = authorize_url_query(&server, &uid, cid).await;

    assert_eq!(host, "as.example.com");
    assert!(!query.contains_key("scope"));
    assert_eq!(
        query.get("resource").map(String::as_str),
        Some(mcp_url.as_str())
    );

    server.cleanup().await;
    clear_signing_key();
    clear_gateway_public_url();
    disallow_private_urls();
}

#[tokio::test]
#[serial]
async fn authorize_on_non_oauth_connector_is_bad_request() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "not-oauth", "bearer").await;

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/oauth/authorize"))),
        &uid,
        "admin",
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        400,
        "OAuth authorize is only for auth_type='oauth2'"
    );

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

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/oauth/authorize"))),
        &uid,
        "admin",
    )
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

    let body: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/oauth/status"))),
        &uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["authorized"], false);

    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{cid}/oauth/token"))),
        &uid,
        "admin",
    )
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
    let other = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &admin,
        "admin",
    )
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
    let member = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &admin,
        "admin",
    )
    .json(&json!({"username": "oa-member", "email": "oa-member@test.local"}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/oauth/status"))),
        member_id,
        "oa-member",
    )
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
    allow_private_urls();
    set_signing_key(SIGNING_KEY);
    set_gateway_public_url();
    let token_url = start_stub_token_server().await;
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid = seed_oauth_connector_with_token_endpoint(&server, uuid, "cb-tool", &token_url).await;
    // The token exchange succeeds against `token_url` regardless, but a
    // successful exchange now also triggers a live verification call against
    // the connector's own resource `url` (`verify_connector_live`) — point it
    // at a real, working stub instead of the placeholder 'https://example.com'
    // `seed_connector` uses by default, or that call would (correctly) fail.
    let resource_url = start_stub_mcp_server_ok().await;
    sqlx::query("UPDATE mcp_connectors SET url = $2 WHERE id = $1")
        .bind(cid)
        .bind(&resource_url)
        .execute(&server.db)
        .await
        .unwrap();

    // Same-origin as the configured gateway public URL, so fix #3's
    // `safe_redirect` honors it verbatim (an off-origin target would be
    // rewritten to "/" — covered by the connect.rs neutralization test).
    let oauth_state = OAuthState::new(
        uuid,
        cid,
        "verifier123".into(),
        Some("https://gateway.test.local/success".into()),
    );
    let signed = sign_state(&oauth_state, SIGNING_KEY);

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123"), ("state", signed.as_str())])
        .send()
        .await
        .unwrap();
    // `d4a02` (OAuth popup close + eager tool sync) replaced the plain 303
    // redirect with an HTML page that closes the popup and redirects via JS
    // — a success is now 200 with the caller's `redirect_url` embedded in
    // the body, not a `Location` header.
    assert_eq!(
        res.status(),
        200,
        "successful exchange must render the popup-close page"
    );
    let body = res.text().await.unwrap();
    assert!(
        body.contains("https://gateway.test.local/success"),
        "the popup-close page must carry the caller-supplied redirect_url: {body}"
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
    assert!(
        enc.is_some(),
        "the exchanged access token must be persisted"
    );
    assert_ne!(
        enc.unwrap(),
        "at_from_stub",
        "the token must be encrypted at rest, not stored raw"
    );

    // A successful OAuth round-trip must flip the connector's setup_status to
    // 'active' (oss/mcp-gateway/src/oauth.rs).
    let (setup_status, setup_error): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT setup_status, setup_error FROM mcp_connectors WHERE id = $1")
            .bind(cid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        setup_status.as_deref(),
        Some("active"),
        "successful OAuth must mark the connector active"
    );
    assert!(
        setup_error.is_none(),
        "no error on success: {setup_error:?}"
    );

    clear_signing_key();
    clear_gateway_public_url();
    disallow_private_urls();
    server.cleanup().await;
}

/// **Definition of done for the OAuth auto-resolve gap**: a pending `auth_required` row for
/// this exact `(owner_user_id, connector_id)` must resolve itself the moment
/// the real OAuth callback confirms the new credential works — no call to
/// `POST /api/hitl/{id}/resolve` involved — and MCP's own resume dispatcher
/// (already running in `TestServer`, `origin = 'mcp_tool'`-scoped) must then
/// claim it, proving the whole loop closes end to end rather than just the
/// `resolve()` call in isolation.
#[tokio::test]
#[serial]
async fn callback_auto_resolves_pending_auth_required_hitl_row() {
    allow_private_urls();
    set_signing_key(SIGNING_KEY);
    set_gateway_public_url();
    let token_url = start_stub_token_server().await;
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid =
        seed_oauth_connector_with_token_endpoint(&server, uuid, "auto-resolve-tool", &token_url)
            .await;
    let resource_url = start_stub_mcp_server_ok().await;
    sqlx::query("UPDATE mcp_connectors SET url = $2 WHERE id = $1")
        .bind(cid)
        .bind(&resource_url)
        .execute(&server.db)
        .await
        .unwrap();

    let agent_id: Uuid =
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
            .bind("auto-resolve-agent")
            .bind(uuid)
            .fetch_one(&server.db)
            .await
            .unwrap();

    // The row a real failed tool call would have created earlier in the same
    // broken-credential window — created directly via the same `repo`
    // function `protocol::handle_tools_call` uses, exactly like
    // `oss/mcp-gateway/tests/auth_required.rs` does.
    let pending = nasiko_hitl::repo::create_pending_auth_required(
        &server.db,
        nasiko_hitl::NewAuthRequired {
            agent_id,
            owner_user_id: uuid,
            connector_id: cid,
            context_id: "ses_auto_resolve_test".to_string(),
            question: json!({"message": "Tool requires re-authentication.", "connector": "auto-resolve-tool"}),
        },
    )
    .await
    .unwrap();
    assert_eq!(pending.status, nasiko_hitl::HitlStatus::Pending);

    let oauth_state = OAuthState::new(
        uuid,
        cid,
        "verifier123".into(),
        Some("https://gateway.test.local/success".into()),
    );
    let signed = sign_state(&oauth_state, SIGNING_KEY);

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123"), ("state", signed.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the callback itself must still succeed");
    let body = res.text().await.unwrap();
    assert!(
        body.contains("https://gateway.test.local/success"),
        "a successful exchange renders the popup-close page carrying the caller's redirect_url: {body}"
    );

    // (a) The row must resolve itself — no call to POST /api/hitl/{id}/resolve
    // was made anywhere in this test.
    let (status, human_response): (String, Option<Value>) =
        sqlx::query_as("SELECT status, human_response FROM hitl_requests WHERE id = $1")
            .bind(pending.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        status, "resolved",
        "the pending auth_required row must auto-resolve once the callback verifies the new credential"
    );
    assert_eq!(
        human_response.as_ref().and_then(|v| v["decision"].as_str()),
        Some("approve")
    );

    // (b) MCP's resume dispatcher (mcp_tool-scoped `claim_for_resume`) must
    // then pick the resolved row up on its own — poll for `resume_status` to
    // leave `not_started` instead of asserting a fixed sleep.
    let mut resume_status = "not_started".to_string();
    for _ in 0..20 {
        resume_status = sqlx::query_scalar("SELECT resume_status FROM hitl_requests WHERE id = $1")
            .bind(pending.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
        if resume_status != "not_started" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert_ne!(
        resume_status, "not_started",
        "the mcp_tool dispatcher must claim the now-resolved row within its poll interval"
    );

    clear_signing_key();
    clear_gateway_public_url();
    disallow_private_urls();
    server.cleanup().await;
}

/// A stub OAuth token endpoint that rejects every exchange, to drive the
/// failure branch of `handle_callback`.
async fn start_failing_token_server() -> String {
    async fn respond() -> impl IntoResponse {
        (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "invalid_grant"})),
        )
    }
    let app = Router::new().route("/token", post(respond));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/token")
}

#[tokio::test]
#[serial]
async fn callback_failed_exchange_marks_connector_setup_failed() {
    set_signing_key(SIGNING_KEY);
    set_gateway_public_url();
    let token_url = start_failing_token_server().await;
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid =
        seed_oauth_connector_with_token_endpoint(&server, uuid, "cb-fail-tool", &token_url).await;

    let oauth_state = OAuthState::new(
        uuid,
        cid,
        "verifier123".into(),
        Some("https://gateway.test.local/success".into()),
    );
    let signed = sign_state(&oauth_state, SIGNING_KEY);

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123"), ("state", signed.as_str())])
        .send()
        .await
        .unwrap();
    // A failed exchange renders the message page (200), never a redirect, and
    // must not echo the token-endpoint body.
    assert_eq!(
        res.status(),
        200,
        "failed exchange must render the error page, not redirect"
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("Token exchange failed"), "{body}");

    // No connection row should have become ACTIVE.
    let conn: Option<String> = sqlx::query_scalar(
        "SELECT status FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2",
    )
    .bind(uuid)
    .bind(cid)
    .fetch_optional(&server.db)
    .await
    .unwrap();
    assert_ne!(
        conn.as_deref(),
        Some("ACTIVE"),
        "a failed exchange must not leave an ACTIVE connection"
    );

    // The connector's setup_status must record the failure.
    let (setup_status, setup_error): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT setup_status, setup_error FROM mcp_connectors WHERE id = $1")
            .bind(cid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        setup_status.as_deref(),
        Some("failed"),
        "failed OAuth must mark the connector setup failed"
    );
    assert!(setup_error.is_some(), "a failure reason must be recorded");

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
    assert_eq!(
        res.status(),
        200,
        "missing state must render the error page, not redirect"
    );
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
        assert_eq!(
            res.status(),
            200,
            "garbage state {garbage:?} must be a clean error page, not a panic/500"
        );
        let body = res.text().await.unwrap();
        assert!(
            body.contains("Invalid or expired state"),
            "{garbage:?} -> {body}"
        );
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
    assert_eq!(
        res.status(),
        200,
        "expired state must render the error page, not redirect"
    );
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
        .query(&[
            ("error", "access_denied"),
            ("error_description", "User denied access"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "an IdP error must render the error page, not redirect or crash"
    );
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
    let cid =
        seed_oauth_connector_with_token_endpoint(&server, uuid, "vuln2-tool", &token_url).await;

    // The old published default literal — no longer the real signing key.
    let forged_state = OAuthState::new(
        uuid,
        cid,
        "attacker-chosen-verifier".into(),
        Some("https://attacker.example/land".into()),
    );
    let signed = sign_state(&forged_state, DEFAULT_SIGNING_KEY);

    let res = no_redirect_client()
        .get(server.url("/api/mcp/oauth/callback"))
        .query(&[("code", "authcode123"), ("state", signed.as_str())])
        .send()
        .await
        .unwrap();

    // State fails HMAC verification against the JWT-derived key → error page,
    // no redirect, no token exchange.
    assert_eq!(
        res.status(),
        200,
        "a state forged with the old default key must be rejected, not honored"
    );
    let location = res.headers().get("location").and_then(|v| v.to_str().ok());
    assert_ne!(
        location,
        Some("https://attacker.example/land"),
        "forged state must never drive a redirect"
    );
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
    assert_eq!(
        count, 0,
        "no token may be stored from a rejected forged state"
    );

    clear_gateway_public_url();
    server.cleanup().await;
}
