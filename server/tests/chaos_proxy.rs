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

use serial_test::serial;
use uuid::Uuid;

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
