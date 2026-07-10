//! HTTP-level tests for the per-connector OAuth 2.1 management routes' guard
//! paths (the full authorize→callback flow needs a live authorization server and
//! is covered by crate unit tests).
//!
//!   cargo test -p nasiko-server --test mcp_oauth -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

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
