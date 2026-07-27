//! Integration tests for the v2 MCP gateway management surface: `/api/mcp/catalog`,
//! `/api/mcp/auth-configs` (composio), and `/api/mcp/connectors` (custom).
//!
//! Composio happy-paths need a live Composio API + mock ToolProvider seam absent
//! here, so those are covered by `oss/mcp-gateway/tests/http_clients.rs`. What is
//! testable end-to-end through real Postgres — catalog merge, admin gating,
//! ownership, collision/validation guards, SSRF, probe — is covered here.
//!
//!   cargo test -p nasiko-server --test mcp_connectors -- --test-threads=1

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
    common::as_superuser(
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
    .unwrap()
}

/// Seed a Composio connector directly (bypasses the Composio API call).
async fn seed_composio_connector(
    server: &common::TestServer,
    toolkit: &str,
    display_name: Option<&str>,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, name, auth_config_id, display_name)
         VALUES ('composio', $1, $2, $3) RETURNING id",
    )
    .bind(toolkit)
    .bind(format!("ac_{toolkit}"))
    .bind(display_name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// Seed a custom (mcp_server) connector owned by `owner`.
async fn seed_custom_connector(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    auth_type: &str,
) -> Uuid {
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

fn allow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
}
fn disallow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
}

async fn start_stub_mcp_server(
    status: StatusCode,
    www_authenticate: Option<&'static str>,
) -> String {
    async fn respond(
        status: StatusCode,
        www_authenticate: Option<&'static str>,
    ) -> impl IntoResponse {
        let mut res = (status, "{}").into_response();
        if let Some(v) = www_authenticate {
            res.headers_mut()
                .insert(axum::http::header::WWW_AUTHENTICATE, v.parse().unwrap());
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

    let res = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        uid,
        "admin",
    )
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
async fn catalog_merges_composio_and_owned_connectors() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid = Uuid::parse_str(uid).unwrap();

    seed_composio_connector(&server, "gmail", None).await;
    seed_custom_connector(&server, uuid, "serpapi", "bearer").await;

    let res = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    let body: Value = res.json().await.unwrap();
    let services = body["services"].as_array().unwrap();
    assert_eq!(services.len(), 2);

    let gmail = services.iter().find(|s| s["name"] == "gmail").unwrap();
    assert_eq!(gmail["type"], "composio");
    assert_eq!(
        gmail["display_name"], "Gmail",
        "no display_name → capitalize()"
    );
    assert_eq!(gmail["auth_flow"], "oauth");

    let serpapi = services.iter().find(|s| s["name"] == "serpapi").unwrap();
    assert_eq!(serpapi["type"], "mcp_server");
    assert_eq!(serpapi["auth_flow"], "api_key");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn catalog_hides_other_users_private_connectors() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "cat-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let bob = create_user(&server, uid, "cat-bob").await;
    let bob_uuid = Uuid::parse_str(bob["id"].as_str().unwrap()).unwrap();

    seed_custom_connector(&server, bob_uuid, "bob-secret", "none").await;

    let res = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        alice_id,
        "cat-alice",
    )
    .send()
    .await
    .unwrap();
    let body: Value = res.json().await.unwrap();
    let names: Vec<&str> = body["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"bob-secret"),
        "must not see bob's private connector: {names:?}"
    );

    server.cleanup().await;
}

// ─── composio registration (auth-configs) ────────────────────────────────────

#[tokio::test]
#[serial]
async fn create_auth_config_requires_admin() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "acfg-member").await;
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(
        server.client.post(server.url("/api/mcp/auth-configs")),
        member_id,
        "acfg-member",
    )
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

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/auth-configs")),
        uid,
        "admin",
    )
    .json(&json!({"toolkit": "gmail"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 503, "no COMPOSIO_API_KEY → NotConfigured");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_auth_config_conflicts_on_duplicate() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    seed_composio_connector(&server, "slack", None).await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/auth-configs")),
        uid,
        "admin",
    )
    .json(&json!({"toolkit": "slack"}))
    .send()
    .await
    .unwrap();
    // Duplicate check runs before the Composio call → 409, not 503.
    assert_eq!(res.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_and_delete_auth_configs() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "acfg-lister").await;
    let member_id = member["id"].as_str().unwrap();
    let id = seed_composio_connector(&server, "notion", Some("Notion")).await;

    // List is admin-only (finding #9): a non-admin member is forbidden.
    let res = common::as_member(
        server.client.get(server.url("/api/mcp/auth-configs")),
        member_id,
        "acfg-lister",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403, "listing auth-configs must require admin");

    // Admin can list.
    let res = common::as_superuser(
        server.client.get(server.url("/api/mcp/auth-configs")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["data"][0]["toolkit"], "notion");

    // Delete: non-admin 403, admin 204.
    let res = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/mcp/auth-configs/{id}"))),
        member_id,
        "acfg-lister",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/mcp/auth-configs/{id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_connectors WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);

    server.cleanup().await;
}

// ─── custom connectors ────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn register_connector_is_owned_by_caller() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "reg-owner").await;
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(
        server.client.post(server.url("/api/mcp/connectors")),
        member_id,
        "reg-owner",
    )
    .json(&json!({"name": "my-tool", "url": "https://example.com"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["provider_type"], "mcp_server");
    assert_eq!(body["auth_type"], "none");
    assert_eq!(body["owner_id"], member_id);
    let connector_id = body["connector_id"].as_str().unwrap();

    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM mcp_connectors WHERE id = $1")
        .bind(Uuid::parse_str(connector_id).unwrap())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(owner.to_string(), member_id);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn register_connector_duplicate_name_conflicts() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let body = json!({"name": "dup-tool", "url": "https://example.com"});
    let r1 = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        uid,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap();
    assert_eq!(r1.status(), 201);
    let r2 = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        uid,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap();
    assert_eq!(r2.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn register_connector_validation_errors() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // url_param without url_param_name.
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        uid,
        "admin",
    )
    .json(&json!({"name": "up-tool", "url": "https://example.com", "auth_type": "url_param"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    // invalid auth_type.
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        uid,
        "admin",
    )
    .json(&json!({"name": "bad-tool", "url": "https://example.com", "auth_type": "bogus"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn register_connector_rejects_private_url() {
    disallow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        uid,
        "admin",
    )
    .json(&json!({"name": "ssrf-tool", "url": "http://127.0.0.1:9999"}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        400,
        "loopback URL must be rejected by the SSRF guard"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_connectors_shows_own_not_others() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "lc-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let alice_uuid = Uuid::parse_str(alice_id).unwrap();
    let bob = create_user(&server, uid, "lc-bob").await;
    let bob_uuid = Uuid::parse_str(bob["id"].as_str().unwrap()).unwrap();

    seed_custom_connector(&server, alice_uuid, "alice-tool", "none").await;
    seed_custom_connector(&server, bob_uuid, "bob-tool", "none").await;

    let res = common::as_member(
        server.client.get(server.url("/api/mcp/connectors")),
        alice_id,
        "lc-alice",
    )
    .send()
    .await
    .unwrap();
    let body: Value = res.json().await.unwrap();
    let names: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"alice-tool"), "{names:?}");
    assert!(
        !names.contains(&"bob-tool"),
        "must not see bob's connector: {names:?}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_connector_owner_and_non_owner() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "dc-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let alice_uuid = Uuid::parse_str(alice_id).unwrap();
    let bob = create_user(&server, uid, "dc-bob").await;
    let bob_id = bob["id"].as_str().unwrap();

    let cid = seed_custom_connector(&server, alice_uuid, "alice-del", "none").await;

    // Non-owner → 403.
    let res = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{cid}"))),
        bob_id,
        "dc-bob",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    // Owner → 204.
    let res = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{cid}"))),
        alice_id,
        "dc-alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    // 404 for missing.
    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{}", Uuid::new_v4()))),
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
async fn delete_connector_cleans_up_agent_access() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let owner = create_user(&server, uid, "cleanup-owner").await;
    let owner_id = owner["id"].as_str().unwrap();
    let owner_uuid = Uuid::parse_str(owner_id).unwrap();

    let cid = seed_custom_connector(&server, owner_uuid, "cleanup-tool", "none").await;
    let agent_id = seed_agent(&server, owner_uuid, "cleanup-agent").await;
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (user_id, agent_id, connector_id, enabled) VALUES ($1, $2, $3, false)",
    )
    .bind(owner_uuid)
    .bind(agent_id)
    .bind(cid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{cid}"))),
        owner_id,
        "cleanup-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mcp_agent_connector_access WHERE connector_id = $1",
    )
    .bind(cid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(remaining, 0, "CASCADE must remove per-agent access rows");

    server.cleanup().await;
}

// ─── update (PATCH) ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn update_connector_edits_fields() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid = Uuid::parse_str(uid).unwrap();
    let cid = seed_custom_connector(&server, uuid, "edit-tool", "none").await;

    let res = common::as_superuser(
        server
            .client
            .patch(server.url(&format!("/api/mcp/connectors/{cid}"))),
        uid,
        "admin",
    )
    .json(&json!({"description": "now documented", "is_active": false}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["description"], "now documented");
    assert_eq!(body["is_active"], false);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_connector_non_owner_forbidden_and_invalid_auth_rejected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "up-alice").await;
    let alice_uuid = Uuid::parse_str(alice["id"].as_str().unwrap()).unwrap();
    let bob = create_user(&server, uid, "up-bob").await;
    let bob_id = bob["id"].as_str().unwrap();
    let cid = seed_custom_connector(&server, alice_uuid, "up-tool", "none").await;

    // Non-owner → 403.
    let res = common::as_member(
        server
            .client
            .patch(server.url(&format!("/api/mcp/connectors/{cid}"))),
        bob_id,
        "up-bob",
    )
    .json(&json!({"description": "hijack"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    // Owner, invalid auth_type → 400.
    let res = common::as_member(
        server
            .client
            .patch(server.url(&format!("/api/mcp/connectors/{cid}"))),
        alice["id"].as_str().unwrap(),
        "up-alice",
    )
    .json(&json!({"auth_type": "bogus"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_composio_metadata_admin_only() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let member = create_user(&server, uid, "cm-member").await;
    let member_id = member["id"].as_str().unwrap();
    let cid = seed_composio_connector(&server, "gmail", None).await;

    // Non-admin → 403.
    let res = common::as_member(
        server
            .client
            .patch(server.url(&format!("/api/mcp/auth-configs/{cid}"))),
        member_id,
        "cm-member",
    )
    .json(&json!({"display_name": "Gmail Pro"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    // Admin → 200, metadata updated, auth_config_id untouched.
    let res = common::as_superuser(
        server
            .client
            .patch(server.url(&format!("/api/mcp/auth-configs/{cid}"))),
        uid,
        "admin",
    )
    .json(&json!({"display_name": "Gmail Pro", "logo_url": "https://x/logo.png"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.json::<Value>().await.unwrap()["display_name"],
        "Gmail Pro"
    );

    let ac: String = sqlx::query_scalar("SELECT auth_config_id FROM mcp_connectors WHERE id = $1")
        .bind(cid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        ac, "ac_gmail",
        "editing metadata must not mint a new auth_config_id"
    );

    // PATCHing a custom connector via the composio route → 404.
    let custom = seed_custom_connector(
        &server,
        Uuid::parse_str(uid).unwrap(),
        "not-composio",
        "none",
    )
    .await;
    let res = common::as_superuser(
        server
            .client
            .patch(server.url(&format!("/api/mcp/auth-configs/{custom}"))),
        uid,
        "admin",
    )
    .json(&json!({"display_name": "x"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── probe ──────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn probe_detects_auth_types() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let none_url = start_stub_mcp_server(StatusCode::OK, None).await;
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors/probe")),
        uid,
        "admin",
    )
    .json(&json!({"url": none_url}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["auth_type"], "none");

    let bearer_url =
        start_stub_mcp_server(StatusCode::UNAUTHORIZED, Some("Bearer realm=\"mcp\"")).await;
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors/probe")),
        uid,
        "admin",
    )
    .json(&json!({"url": bearer_url}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["auth_type"], "bearer");

    let oauth_url = start_stub_mcp_server(
        StatusCode::UNAUTHORIZED,
        Some("Bearer resource_metadata=\"https://as/x\""),
    )
    .await;
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors/probe")),
        uid,
        "admin",
    )
    .json(&json!({"url": oauth_url}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["auth_type"], "oauth2");

    disallow_private_urls();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn probe_rejects_private_url() {
    disallow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors/probe")),
        uid,
        "admin",
    )
    .json(&json!({"url": "http://127.0.0.1:9999"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}
