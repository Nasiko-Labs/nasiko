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

use serde_json::json;
use serial_test::serial;
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
