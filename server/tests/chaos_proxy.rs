//! Chaos suite for the A2A agent proxy and orchestrator dispatch.
//!
//! Scenarios and invariant IDs live in `docs/chaos/INVARIANTS.md` and
//! `docs/chaos/FINDINGS.md`. Each test here should name the invariant it
//! asserts. Keep new cases in this file so the suite stays one review surface.
//!
//! Requires local infra (Postgres :5432, Redis, S3), same as other server
//! integration tests:
//!   cargo test -p nasiko-server --test chaos_proxy -- --test-threads=1

mod common;

use axum::{Json, Router, routing::get};
use serde_json::json;
use serial_test::serial;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

fn a2a_stream_body(text: &str) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "method": "message/stream",
        "id": Uuid::new_v4().to_string(),
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{ "text": text }]
            }
        }
    })
}

async fn init_admin(server: &common::TestServer) -> serde_json::Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
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

async fn start_slow_stub(delay: Duration) -> String {
    let app = Router::new().route(
        "/slow",
        get(move || async move {
            tokio::time::sleep(delay).await;
            Json(json!({"ok": true}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

/// Accepts one HTTP request, writes a truncated body, then closes the socket.
async fn start_truncate_stub() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 2048];
            let _ = stream.read(&mut buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\n\r\npartial")
                .await;
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// I1 — Sole authenticated ingress: an unauthenticated call on the agent
/// API must not reach handler logic as an anonymous user.
#[tokio::test]
#[serial]
async fn i1_unauthenticated_agent_route_returns_401() {
    let server = common::TestServer::start().await;
    let agent_id = Uuid::new_v4();

    let res = server
        .client
        .get(server.url(&format!("/api/agents/{agent_id}/deployment")))
        .send()
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        401,
        "unauthenticated agent routes must return 401 (I1)"
    );

    server.cleanup().await;
}

/// I7 / C7 — A2A dispatch is limited to 30 requests / 60s per authenticated
/// user. The 31st call in the window must be 429. Empty fleet still counts:
/// rate limiting runs before routing.
#[tokio::test]
#[serial]
async fn i7_a2a_dispatch_returns_429_after_burst() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().expect("initialize-admin returns user_id");

    let mut statuses = Vec::new();
    let mut saw_429 = false;
    for _ in 0..40 {
        let res = common::as_superuser(
            server
                .client
                .post(server.url("/api/orchestrator/a2a"))
                .json(&a2a_stream_body("rate-limit probe")),
            user_id,
            "admin",
        )
        .send()
        .await
        .unwrap();

        let status = res.status();
        statuses.push(status.as_u16());
        if status == 429 {
            let body = res.text().await.unwrap();
            assert!(
                body.contains("rate limit exceeded"),
                "429 body must be visible (I7), got {body:?}"
            );
            saw_429 = true;
            break;
        }
    }

    assert!(
        saw_429,
        "expected HTTP 429 within 40 A2A dispatch calls (limit is 30/60s); statuses={statuses:?}"
    );

    server.cleanup().await;
}

/// I9 / C2 — a slow upstream must not crash the control plane. The proxy
/// waits and returns the stub body; `/health` still succeeds.
#[tokio::test]
#[serial]
async fn i9_slow_upstream_does_not_crash_proxy() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let owner_id = seed_user(&server, "chaos-c2-owner").await;
    let stub_url = start_slow_stub(Duration::from_secs(2)).await;
    let agent_id = seed_running_agent(&server, owner_id, "chaos-slow-agent", &stub_url).await;

    let started = Instant::now();
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent_id}/slow"))),
        &owner_id.to_string(),
        "chaos-c2-owner",
    )
    .send()
    .await
    .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(res.status(), 200, "slow upstream must still complete (I9)");
    assert!(
        elapsed >= Duration::from_secs(2),
        "proxy must wait for the delayed agent, elapsed={elapsed:?}"
    );

    let health = server
        .client
        .get(server.url("/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200, "control plane must stay up (I9)");

    server.cleanup().await;
}

/// I9 / C3 — a truncated upstream must not hang or crash the control plane.
#[tokio::test]
#[serial]
async fn i9_truncated_upstream_does_not_hang_proxy() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let owner_id = seed_user(&server, "chaos-c3-owner").await;
    let stub_url = start_truncate_stub().await;
    let agent_id = seed_running_agent(&server, owner_id, "chaos-trunc-agent", &stub_url).await;

    let send = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/agents/{agent_id}/")))
            .timeout(Duration::from_secs(10)),
        &owner_id.to_string(),
        "chaos-c3-owner",
    )
    .send();

    let timed = tokio::time::timeout(Duration::from_secs(15), send).await;
    assert!(
        timed.is_ok(),
        "proxy must not hang on a truncated upstream (I9)"
    );

    let health = server
        .client
        .get(server.url("/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200, "control plane must stay up (I9)");

    server.cleanup().await;
}

/// I6 / C4 — after the session token is revoked, the same bearer must not
/// invoke the proxy.
#[tokio::test]
#[serial]
async fn i6_revoked_session_cannot_invoke_proxy() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id: Uuid = admin["user_id"]
        .as_str()
        .expect("user_id")
        .parse()
        .expect("user_id uuid");
    let token = admin["token"]
        .as_str()
        .expect("initialize-admin returns token");

    let stub_url = start_slow_stub(Duration::ZERO).await;
    let agent_id = seed_running_agent(&server, user_id, "chaos-c4-agent", &stub_url).await;

    let before = server
        .client
        .get(server.url(&format!("/api/agents/{agent_id}/slow")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(before.status(), 200, "issued session must reach the agent");

    sqlx::query("UPDATE auth_tokens SET revoked_at = now() WHERE user_id = $1")
        .bind(user_id)
        .execute(&server.db)
        .await
        .unwrap();

    let after = server
        .client
        .get(server.url(&format!("/api/agents/{agent_id}/slow")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        401,
        "revoked session must not invoke the proxy (I6)"
    );

    server.cleanup().await;
}
