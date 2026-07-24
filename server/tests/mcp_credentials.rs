//! HTTP-level tests for per-user credential management on custom connectors.
//!
//!   cargo test -p nasiko-server --test mcp_credentials -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

fn allow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
}
fn disallow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
}

async fn init_admin(server: &common::TestServer) -> String {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["user_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn create_user(
    server: &common::TestServer,
    admin_id: &str,
    username: &str,
) -> (String, Uuid) {
    let v = common::as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": username, "email": format!("{username}@test.local")}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    (id.clone(), Uuid::parse_str(&id).unwrap())
}

/// A real, live MCP backend that answers any JSON-RPC call with an empty
/// `tools/list` result — needed wherever a test registers a credential and
/// expects it to actually verify successfully (`verify_connector_live` makes
/// a genuine call now, unlike before).
async fn start_stub_mcp_server_ok() -> String {
    async fn respond() -> axum::Json<Value> {
        axum::Json(json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}}))
    }
    let app = axum::Router::new().route("/", axum::routing::post(respond));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/")
}

async fn seed_connector(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    auth_type: &str,
    url: &str,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type)
         VALUES ('mcp_server', $1, $2, $3, $4) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(url)
    .bind(auth_type)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn register_status_and_delete_credential() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_uuid = Uuid::parse_str(&admin).unwrap();
    let backend_url = start_stub_mcp_server_ok().await;
    let cid = seed_connector(&server, admin_uuid, "cred-tool", "bearer", &backend_url).await;

    // Register.
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &admin,
        "admin",
    )
    .json(&json!({"value": "sk-abc"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.json::<Value>().await.unwrap()["data"]["connected"], true);

    // Status: connected.
    let body: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/credential/status"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["connected"], true);
    assert_eq!(body["data"]["auth_type"], "bearer");

    // Delete → 200 (envelope), then status: not connected.
    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/credential/status"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["connected"], false);

    disallow_private_urls();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn register_credential_on_inaccessible_connector_forbidden() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (_alice_id, alice_uuid) = create_user(&server, &admin, "cr-alice").await;
    let (bob_id, _) = create_user(&server, &admin, "cr-bob").await;
    let cid = seed_connector(&server, alice_uuid, "alice-cred-tool", "bearer", "https://example.com").await;

    // Bob can't reach alice's private connector.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &bob_id,
        "cr-bob",
    )
    .json(&json!({"value": "x"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn register_credential_on_none_auth_is_bad_request() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_uuid = Uuid::parse_str(&admin).unwrap();
    let cid = seed_connector(&server, admin_uuid, "noauth-tool", "none", "https://example.com").await;

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &admin,
        "admin",
    )
    .json(&json!({"value": "x"}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        400,
        "credentials only apply to bearer/basic/url_param"
    );

    server.cleanup().await;
}
