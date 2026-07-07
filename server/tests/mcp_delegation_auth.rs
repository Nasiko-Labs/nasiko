//! `POST /api/mcp` auth gate — `require_delegation` (see `oss/server/src/mcp/gateway.rs`).
//!
//! This route is deliberately NOT behind `require_auth`: a deployed agent
//! never holds the calling user's real session JWT (`agent_proxy.rs` strips
//! `Authorization`/`Cookie` before forwarding to a container on purpose), so
//! its only credential is the delegation token. These tests lock in the two
//! failure modes that matter most: a real agent's actual headers must work,
//! and a real user's session JWT alone must NOT work (that would defeat the
//! whole point of stripping it).

mod common;

use common::TestServer;
use nasiko_auth::jwt::mint_delegation_token;
use serial_test::serial;
use uuid::Uuid;

fn deleg_token(user_id: &str, agent_id: &str) -> String {
    mint_delegation_token(common::TEST_JWT_SECRET, user_id, agent_id).expect("mint delegation token")
}

async fn mcp_initialize(server: &TestServer, token: Option<&str>) -> reqwest::Response {
    let mut req = server
        .client
        .post(server.url("/api/mcp"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}));
    if let Some(t) = token {
        req = req.header("x-nasiko-agent-token", t);
    }
    req.send().await.unwrap()
}

#[tokio::test]
#[serial]
async fn valid_delegation_token_alone_is_accepted() {
    let server = TestServer::start().await;
    let token = deleg_token(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string());

    let res = mcp_initialize(&server, Some(&token)).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "MCP Gateway");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn missing_delegation_token_is_rejected() {
    let server = TestServer::start().await;

    let res = mcp_initialize(&server, None).await;
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_real_user_session_jwt_alone_does_not_work() {
    // The core regression this file exists to guard: a real, valid session
    // JWT via Authorization — with NO delegation token — must still be
    // rejected. If this ever passes, it means `/api/mcp` silently started
    // trusting `require_auth` again, which an agent can never satisfy.
    let server = TestServer::start().await;
    let user_token = common::sign_token(&Uuid::new_v4().to_string(), "someuser", false, "member");

    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&user_token)
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "a session JWT alone must not satisfy the delegation auth gate");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delegation_token_signed_with_wrong_secret_is_rejected() {
    let server = TestServer::start().await;
    let forged = mint_delegation_token("attacker-controlled-secret", &Uuid::new_v4().to_string(), &Uuid::new_v4().to_string())
        .expect("mint delegation token");

    let res = mcp_initialize(&server, Some(&forged)).await;
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn garbage_delegation_token_is_rejected_not_panicking() {
    let server = TestServer::start().await;

    for bad in ["", "not-a-jwt", "a.b.c", &"A".repeat(5000)] {
        let res = mcp_initialize(&server, Some(bad)).await;
        assert_eq!(res.status(), 401, "{bad:?} must be a clean 401, not a panic/500");
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_typed_service_account_jwt_does_not_satisfy_delegation_auth() {
    // `issue_agent_token`-style tokens (token_type="agent") are a different
    // credential entirely — confirm they don't accidentally slip through as
    // a delegation token (they lack `act`/`aud` claims, so this should already
    // be impossible, but it's the exact confusion this route must never allow).
    let server = TestServer::start().await;
    let agent_service_token = common::sign_agent_token(&Uuid::new_v4().to_string());

    let res = mcp_initialize(&server, Some(&agent_service_token)).await;
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}
