mod common;

use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> serde_json::Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&serde_json::json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
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
    let res = server
        .client
        .get(server.url("/api/me"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    server.cleanup().await;
}

// ─── Auth gating ─────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_agent_proxy_route_not_served_by_server() {
    // The A2A agent proxy lives in the gateway, not the server.
    // The server only exposes management routes (deploy, ACL, etc.).
    // Requests to /api/agents/{id}/chat have no matching route on the server,
    // so the fallback fires — 404, regardless of auth.
    let server = common::TestServer::start().await;
    let random_agent_id = Uuid::new_v4();

    let res = server
        .client
        .get(server.url(&format!("/api/agents/{random_agent_id}/chat")))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── Agent proxy paths on the server return 404 ──────────────────────────────

#[tokio::test]
#[serial]
async fn test_agent_proxy_without_traceparent_returns_404_not_400() {
    // Agent proxying is handled entirely by the gateway, not the server.
    // Requests to /api/agents/{id}/* have no matching route on the server,
    // so the fallback fires — 404 — even with valid auth headers.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();
    let nonexistent_agent = Uuid::new_v4();

    let res = server
        .client
        .get(server.url(&format!("/api/agents/{nonexistent_agent}/some/path")))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        // No traceparent header — used to be 400, now should fall back to new_root
        .send()
        .await
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

    let res = server
        .client
        .get(server.url(&format!("/api/agents/{nonexistent_agent}/some/path")))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .header(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}
