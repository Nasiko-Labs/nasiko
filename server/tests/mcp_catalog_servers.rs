//! Integration tests for `/api/mcp/catalog`, `/api/mcp/auth-configs`, and
//! `/api/mcp/servers` — the routes left uncovered when the older
//! `mcp_gateway.rs` integration test file was dropped in an earlier merge.
//!
//! Composio-backed flows (`create_auth_config`'s happy path) can't be driven
//! here — the test harness has no `COMPOSIO_API_KEY` and no seam to inject a
//! mock `ToolProvider` into `AppState`, so those paths are exercised instead by
//! `oss/mcp-gateway/tests/http_clients.rs` (mockito) and unit tests. What *is*
//! testable end-to-end through real Postgres — admin gating, duplicate/collision
//! guards, the catalog view merge, server CRUD + validation, the SSRF guard, and
//! the auth-type probe — is covered here.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test mcp_catalog_servers -- --test-threads=1

mod common;

use axum::{Router, http::StatusCode, response::IntoResponse, routing::post};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── helpers ────────────────────────────────────────────────────────────────

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

async fn create_user(server: &common::TestServer, admin_id: &str, username: &str) -> Value {
    common::as_superuser(server.client.post(server.url("/api/users")), admin_id, "admin")
        .json(&json!({"username": username, "email": format!("{username}@test.local")}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

/// Seed a platform Composio auth-config directly in the DB (bypasses the
/// Composio API call `create_auth_config` would otherwise make).
async fn seed_platform_auth_config(server: &common::TestServer, toolkit: &str, display_name: Option<&str>) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_auth_configs (auth_config_id, toolkit, is_platform, display_name)
         VALUES ($1, $2, true, $3) RETURNING id",
    )
    .bind(format!("ac_{toolkit}"))
    .bind(toolkit)
    .bind(display_name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_agent(server: &common::TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, is_public) VALUES ($1, $2, 'x:1.0.0', 'stopped', false) RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// Opt the process into the SSRF-guard's dev/test bypass (loopback stub
/// servers). `#[serial]` tests run one at a time in this binary, so process-wide
/// env mutation is safe as long as each test restores it.
fn allow_private_urls() {
    // SAFETY: serialized by `#[serial]` — no concurrent env access.
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
}

fn disallow_private_urls() {
    // SAFETY: serialized by `#[serial]` — no concurrent env access.
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
}

/// Start a stub "MCP server" that always answers `status` with an optional
/// `WWW-Authenticate` header, for probe/auto-register tests. Returns its
/// `http://127.0.0.1:{port}` base URL.
async fn start_stub_mcp_server(status: StatusCode, www_authenticate: Option<&'static str>) -> String {
    async fn respond(status: StatusCode, www_authenticate: Option<&'static str>) -> impl IntoResponse {
        let mut res = (status, "{}").into_response();
        if let Some(v) = www_authenticate {
            res.headers_mut().insert(axum::http::header::WWW_AUTHENTICATE, v.parse().unwrap());
        }
        res
    }
    let app = Router::new().route(
        "/",
        post(move || async move { respond(status, www_authenticate).await }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/")
}

// ─── catalog ────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn catalog_is_empty_by_default() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_member(server.client.get(server.url("/api/mcp/catalog")), uid, "admin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["services"].as_array().unwrap().len(), 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn catalog_merges_composio_and_mcp_servers() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    seed_platform_auth_config(&server, "gmail", None).await;
    sqlx::query(
        "INSERT INTO mcp_servers (name, url, auth_type, is_platform) VALUES ('serpapi', 'https://example.com', 'bearer', true)",
    )
    .execute(&server.db)
    .await
    .unwrap();

    let res = common::as_member(server.client.get(server.url("/api/mcp/catalog")), uid, "admin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let services = body["services"].as_array().unwrap();
    assert_eq!(services.len(), 2);

    let gmail = services.iter().find(|s| s["name"] == "gmail").unwrap();
    assert_eq!(gmail["type"], "composio");
    assert_eq!(gmail["display_name"], "Gmail", "no display_name set — must fall back to capitalize()");
    assert_eq!(gmail["auth_flow"], "oauth");

    let serpapi = services.iter().find(|s| s["name"] == "serpapi").unwrap();
    assert_eq!(serpapi["type"], "mcp");
    assert_eq!(serpapi["auth_flow"], "api_key", "bearer auth_type must map to api_key auth_flow");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_auth_config_requires_admin() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "acfg-member").await;
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(server.client.post(server.url("/api/mcp/auth-configs")), member_id, "acfg-member")
        .json(&json!({"toolkit": "gmail"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_auth_config_fails_without_composio_configured() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/auth-configs")), uid, "admin")
        .json(&json!({"toolkit": "gmail"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 503, "test harness has no COMPOSIO_API_KEY — must fail with NotConfigured");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_auth_config_conflicts_on_duplicate_toolkit() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    seed_platform_auth_config(&server, "slack", None).await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/auth-configs")), uid, "admin")
        .json(&json!({"toolkit": "slack"}))
        .send()
        .await
        .unwrap();
    // The duplicate check runs before the Composio call, so this is 409, not 503.
    assert_eq!(res.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_auth_config_conflicts_with_existing_mcp_server_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    sqlx::query("INSERT INTO mcp_servers (name, url, is_platform) VALUES ('discord', 'https://example.com', true)")
        .execute(&server.db)
        .await
        .unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/auth-configs")), uid, "admin")
        .json(&json!({"toolkit": "discord"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409, "toolkit name collides with an existing platform MCP server name");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_auth_configs_is_open_to_any_authed_user() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "acfg-lister").await;
    let member_id = member["id"].as_str().unwrap();
    seed_platform_auth_config(&server, "notion", Some("Notion")).await;

    let res = common::as_member(server.client.get(server.url("/api/mcp/auth-configs")), member_id, "acfg-lister")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["data"][0]["toolkit"], "notion");
    assert_eq!(body["data"][0]["display_name"], "Notion");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_auth_config_requires_admin_and_checks_existence() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "acfg-deleter").await;
    let member_id = member["id"].as_str().unwrap();
    let id = seed_platform_auth_config(&server, "trello", None).await;
    let auth_config_id = "ac_trello";

    // Non-admin: 403.
    let res = common::as_member(
        server.client.delete(server.url(&format!("/api/mcp/auth-configs/{auth_config_id}"))),
        member_id,
        "acfg-deleter",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    // Admin, missing id: 404.
    let res = common::as_superuser(server.client.delete(server.url("/api/mcp/auth-configs/ac_missing")), uid, "admin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // Admin, real id: 204, and it's gone.
    let res = common::as_superuser(
        server.client.delete(server.url(&format!("/api/mcp/auth-configs/{auth_config_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_auth_configs WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);

    server.cleanup().await;
}

// ─── servers ────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn create_platform_server_requires_admin() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "srv-member").await;
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(server.client.post(server.url("/api/mcp/servers")), member_id, "srv-member")
        .json(&json!({"name": "shared-tool", "url": "https://example.com", "is_platform": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_platform_server_succeeds_as_admin() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&json!({"name": "platform-tool", "url": "https://example.com", "is_platform": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["name"], "platform-tool");
    assert_eq!(body["auth_type"], "none");
    assert_eq!(body["is_platform"], true);
    assert_eq!(body["oauth_configured"], false);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_user_scoped_server_succeeds_and_is_owned() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "srv-owner").await;
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(server.client.post(server.url("/api/mcp/servers")), member_id, "srv-owner")
        .json(&json!({"name": "my-tool", "url": "https://example.com", "is_platform": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    let server_id = body["id"].as_str().unwrap();

    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM mcp_servers WHERE id = $1")
        .bind(Uuid::parse_str(server_id).unwrap())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(owner.to_string(), member_id);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_server_duplicate_name_same_scope_conflicts() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let body = json!({"name": "dup-tool", "url": "https://example.com", "is_platform": true});
    let res1 = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res1.status(), 201);

    let res2 = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res2.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_server_url_param_without_name_is_bad_request() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&json!({
            "name": "url-param-tool", "url": "https://example.com",
            "auth_type": "url_param", "is_platform": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_server_invalid_auth_type_is_bad_request() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&json!({
            "name": "bad-auth-tool", "url": "https://example.com",
            "auth_type": "bogus", "is_platform": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_server_collides_with_composio_toolkit_name() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    seed_platform_auth_config(&server, "asana", None).await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&json!({"name": "asana", "url": "https://example.com", "is_platform": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_server_rejects_private_url_without_bypass() {
    disallow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&json!({"name": "ssrf-tool", "url": "http://127.0.0.1:9999", "is_platform": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400, "loopback URL must be rejected by the SSRF guard");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_servers_combines_platform_and_own_but_not_others() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "list-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let bob = create_user(&server, uid, "list-bob").await;
    let bob_id = bob["id"].as_str().unwrap();

    common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&json!({"name": "shared", "url": "https://example.com", "is_platform": true}))
        .send()
        .await
        .unwrap();
    common::as_member(server.client.post(server.url("/api/mcp/servers")), alice_id, "list-alice")
        .json(&json!({"name": "alice-private", "url": "https://example.com", "is_platform": false}))
        .send()
        .await
        .unwrap();
    common::as_member(server.client.post(server.url("/api/mcp/servers")), bob_id, "list-bob")
        .json(&json!({"name": "bob-private", "url": "https://example.com", "is_platform": false}))
        .send()
        .await
        .unwrap();

    let res = common::as_member(server.client.get(server.url("/api/mcp/servers")), alice_id, "list-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let names: Vec<&str> = body["data"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"shared"), "{names:?}");
    assert!(names.contains(&"alice-private"), "{names:?}");
    assert!(!names.contains(&"bob-private"), "must not see bob's private server: {names:?}");

    disallow_private_urls();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_platform_server_requires_admin() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "del-member").await;
    let member_id = member["id"].as_str().unwrap();

    let create_res = common::as_superuser(server.client.post(server.url("/api/mcp/servers")), uid, "admin")
        .json(&json!({"name": "del-platform-tool", "url": "https://example.com", "is_platform": true}))
        .send()
        .await
        .unwrap();
    let server_id = create_res.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string();

    let res = common::as_member(
        server.client.delete(server.url(&format!("/api/mcp/servers/{server_id}"))),
        member_id,
        "del-member",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_user_server_forbidden_for_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "del-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let bob = create_user(&server, uid, "del-bob").await;
    let bob_id = bob["id"].as_str().unwrap();

    let create_res = common::as_member(server.client.post(server.url("/api/mcp/servers")), alice_id, "del-alice")
        .json(&json!({"name": "alice-tool", "url": "https://example.com", "is_platform": false}))
        .send()
        .await
        .unwrap();
    let server_id = create_res.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string();

    let res = common::as_member(
        server.client.delete(server.url(&format!("/api/mcp/servers/{server_id}"))),
        bob_id,
        "del-bob",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_server_not_found_is_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(
        server.client.delete(server.url(&format!("/api/mcp/servers/{}", Uuid::new_v4()))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_user_server_succeeds_and_cleans_up_agent_permissions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner = create_user(&server, uid, "cleanup-owner").await;
    let owner_id = owner["id"].as_str().unwrap();
    let owner_uuid = Uuid::parse_str(owner_id).unwrap();

    let create_res = common::as_member(server.client.post(server.url("/api/mcp/servers")), owner_id, "cleanup-owner")
        .json(&json!({"name": "cleanup-tool", "url": "https://example.com", "is_platform": false}))
        .send()
        .await
        .unwrap();
    let server_id = create_res.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string();

    let agent_id = seed_agent(&server, owner_uuid, "cleanup-agent").await;
    sqlx::query(
        "INSERT INTO mcp_agent_server_access (user_id, agent_id, server_name, server_type, enabled)
         VALUES ($1, $2, 'cleanup-tool', 'mcp', true)",
    )
    .bind(owner_uuid)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let res = common::as_member(
        server.client.delete(server.url(&format!("/api/mcp/servers/{server_id}"))),
        owner_id,
        "cleanup-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    let remaining_access: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mcp_agent_server_access WHERE server_name = 'cleanup-tool'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(remaining_access, 0, "deleting the server must clean up its permission rows");

    let remaining_server: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_servers WHERE name = 'cleanup-tool'")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(remaining_server, 0);

    server.cleanup().await;
}

// ─── probe ──────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn probe_detects_no_auth() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let stub_url = start_stub_mcp_server(StatusCode::OK, None).await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers/probe")), uid, "admin")
        .json(&json!({"url": stub_url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["auth_type"], "none");
    assert_eq!(body["requires"], "nothing");

    disallow_private_urls();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn probe_detects_bearer() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let stub_url = start_stub_mcp_server(StatusCode::UNAUTHORIZED, Some("Bearer realm=\"mcp\"")).await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers/probe")), uid, "admin")
        .json(&json!({"url": stub_url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["auth_type"], "bearer");
    assert_eq!(body["requires"], "api_key_input");
    assert!(body["hint"].as_str().unwrap().contains("Bearer token"));

    disallow_private_urls();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn probe_detects_oauth2() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let stub_url = start_stub_mcp_server(
        StatusCode::UNAUTHORIZED,
        Some("Bearer resource_metadata=\"https://as.example.com/.well-known/x\""),
    )
    .await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers/probe")), uid, "admin")
        .json(&json!({"url": stub_url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["auth_type"], "oauth2");
    assert_eq!(body["requires"], "oauth_flow");

    disallow_private_urls();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn probe_other_status_defaults_to_bearer_with_status_in_hint() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let stub_url = start_stub_mcp_server(StatusCode::INTERNAL_SERVER_ERROR, None).await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers/probe")), uid, "admin")
        .json(&json!({"url": stub_url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["auth_type"], "bearer");
    assert!(body["hint"].as_str().unwrap().contains("500"), "{body}");

    disallow_private_urls();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn probe_rejects_private_url_without_bypass() {
    disallow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/servers/probe")), uid, "admin")
        .json(&json!({"url": "http://127.0.0.1:9999"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}
