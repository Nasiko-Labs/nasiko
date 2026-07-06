mod common;

use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> serde_json::Value {
    server.client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&serde_json::json!({"username": "admin", "email": "admin@test.local"}))
        .send().await
        .unwrap()
        .json().await
        .unwrap()
}

// ─── Non-agent routes pass through the middleware unaffected ─────────────────

#[tokio::test]
#[serial]
async fn test_proxy_middleware_does_not_block_health_endpoint() {
    let server = common::TestServer::start().await;

    let res = server.client.get(server.url("/health")).send().await.unwrap();
    assert_eq!(res.status(), 200);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_proxy_middleware_does_not_block_non_agent_protected_routes() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    // /api/me is a protected non-agent route — should pass through middleware
    let res = common::as_superuser(
        server.client.get(server.url("/api/me")),
        user_id,
        "admin",
    )
    .send().await
    .unwrap();

    assert_eq!(res.status(), 200);

    server.cleanup().await;
}

// ─── Auth gating ─────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_agent_proxy_route_requires_auth() {
    let server = common::TestServer::start().await;
    let random_agent_id = Uuid::new_v4();

    // Use a registered agent route — auth middleware fires before the handler.
    // /api/agents/{id}/deployment is a real GET route (deployments::orchestrator).
    let res = server.client
        .get(server.url(&format!("/api/agents/{random_agent_id}/deployment")))
        .send().await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

// ─── Regression: missing traceparent no longer returns 400 ───────────────────

#[tokio::test]
#[serial]
async fn test_agent_proxy_without_traceparent_returns_404_not_400() {
    // Before the fix, a missing traceparent returned 400 (MissingFlowContext).
    // After the fix, the middleware creates a new root trace, proceeds to look
    // up the agent in the DB, and returns 404 when it is not found.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();
    let nonexistent_agent = Uuid::new_v4();

    let res = common::as_superuser(
        server.client.get(server.url(&format!("/api/agents/{nonexistent_agent}/some/path"))),
        user_id,
        "admin",
    )
    // No traceparent header — used to be 400, now should fall back to new_root
    .send().await
    .unwrap();

    // 404 means the middleware passed the FlowContext step and reached agent lookup
    assert_eq!(
        res.status(),
        404,
        "missing traceparent should fall back to new_root, not return 400"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_agent_proxy_with_valid_traceparent_also_returns_404_for_unknown_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();
    let nonexistent_agent = Uuid::new_v4();

    let res = common::as_superuser(
        server.client.get(server.url(&format!("/api/agents/{nonexistent_agent}/some/path"))),
        user_id,
        "admin",
    )
    .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
    .send().await
    .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}
