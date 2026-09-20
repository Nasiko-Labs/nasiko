//! Regression tests for the direct agent-proxy path (`/api/agents/{id}/...`):
//!
//! - P0-1/P0-1b: the caller's `Authorization`/`Cookie` (platform credentials)
//!   and a client-forged `x-user-id`/`x-username`/`x-is-superuser` must never
//!   reach the (unvetted) agent container; only the server-derived trusted
//!   identity headers should arrive.
//! - P0-2: a caller with no grant on a private agent must be rejected before
//!   the request ever reaches the agent container (previously unauthenticated
//!   IDOR — any authenticated user could invoke any agent by UUID).
//!
//! Uses a real stub HTTP server as the "agent container" (bound to an
//! ephemeral port, echoes every header it receives as JSON) and seeds an
//! `agents` row pointing `url` directly at it, bypassing the runtime/Docker
//! entirely — `nasiko_agent_proxy::resolve` only reads `agents.status`/`url`.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_proxy_authz -- --test-threads=1

mod common;

use axum::{Json, Router, extract::Request, routing::get};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn seed_user(server: &common::TestServer, username: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@test.local"))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// Seed an agent row whose `url` points at a real (stub) HTTP server, so
/// `nasiko_agent_proxy::resolve` finds a running endpoint without needing an
/// actual container.
async fn seed_running_agent(
    server: &common::TestServer,
    owner_id: Uuid,
    name: &str,
    url: &str,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, url, is_public) VALUES ($1, $2, 'x:1.0.0', 'running', $3, false) RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn echo_headers(req: Request) -> Json<Value> {
    let mut out = serde_json::Map::new();
    for (name, value) in req.headers().iter() {
        out.insert(
            name.as_str().to_string(),
            json!(value.to_str().unwrap_or("")),
        );
    }
    Json(Value::Object(out))
}

/// Start a stub "agent container" echoing received headers; returns its
/// `http://127.0.0.1:{port}` base URL.
async fn start_stub_agent() -> String {
    let app = Router::new().route("/echo", get(echo_headers));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
#[serial]
async fn proxy_strips_credentials_and_spoofed_identity_headers() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let owner_id = seed_user(&server, "proxy-owner").await;
    let stub_url = start_stub_agent().await;
    let agent_id = seed_running_agent(&server, owner_id, "proxy-authz-agent", &stub_url).await;

    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent_id}/echo"))),
        &owner_id.to_string(),
        "proxy-owner",
    )
    .header("cookie", "access_token=evil-cookie")
    .header("x-user-id", "attacker-id")
    .header("x-username", "attacker-name")
    .header("x-is-superuser", "true")
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 200);
    let echoed: Value = res.json().await.unwrap();

    assert!(
        echoed.get("authorization").is_none(),
        "Authorization must not reach the agent: {echoed}"
    );
    assert!(
        echoed.get("cookie").is_none(),
        "Cookie must not reach the agent: {echoed}"
    );
    assert_eq!(
        echoed["x-user-id"],
        owner_id.to_string(),
        "x-user-id must be the server-derived owner id, not the spoofed value: {echoed}"
    );
    assert_eq!(
        echoed["x-username"], "proxy-owner",
        "x-username must be server-derived: {echoed}"
    );
    assert_eq!(
        echoed["x-is-superuser"], "false",
        "x-is-superuser must be server-derived, not spoofed true: {echoed}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn proxy_rejects_non_owner_non_grantee_with_404() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let owner_id = seed_user(&server, "proxy-owner-2").await;
    let other_id = seed_user(&server, "proxy-other").await;
    let stub_url = start_stub_agent().await;
    let agent_id =
        seed_running_agent(&server, owner_id, "proxy-authz-private-agent", &stub_url).await;

    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent_id}/echo"))),
        &other_id.to_string(),
        "proxy-other",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(
        res.status(),
        404,
        "non-owner/non-grantee must not reach a private agent"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn proxy_allows_owner_through_to_the_agent() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let owner_id = seed_user(&server, "proxy-owner-3").await;
    let stub_url = start_stub_agent().await;
    let agent_id =
        seed_running_agent(&server, owner_id, "proxy-authz-owner-agent", &stub_url).await;

    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent_id}/echo"))),
        &owner_id.to_string(),
        "proxy-owner-3",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 200, "owner must reach the agent");

    server.cleanup().await;
}
