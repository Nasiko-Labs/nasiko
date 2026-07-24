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

async fn seed_connection(server: &common::TestServer, user: Uuid, connector: Uuid) {
    sqlx::query(
        "INSERT INTO mcp_user_connections (user_id, connector_id, status, encrypted_credential)
         VALUES ($1, $2, 'ACTIVE', 'enc')",
    )
    .bind(user)
    .bind(connector)
    .execute(&server.db)
    .await
    .unwrap();
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

/// A real, live MCP backend that answers any JSON-RPC call with an empty
/// `tools/list` result regardless of what it's asked — used wherever a test
/// needs the live setup-verification step (`verify_connector_live`) to
/// actually succeed, as opposed to `start_stub_mcp_server` above, which is
/// for testing the auth-type *detection* heuristics against error responses.
async fn start_stub_mcp_server_ok() -> String {
    async fn respond() -> impl IntoResponse {
        axum::Json(serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}}))
    }
    let app = Router::new().route("/", post(respond));
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
    assert_eq!(body["data"]["services"].as_array().unwrap().len(), 0);

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
    let services = body["data"]["services"].as_array().unwrap();
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
    // A custom mcp_server can't report a tool count before you connect —
    // `null` means "unknown until connected", never 0.
    assert!(
        serpapi["tool_count"].is_null(),
        "mcp_server tool_count must be null pre-connect: {}",
        serpapi["tool_count"]
    );

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
    let names: Vec<&str> = body["data"]["services"]
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
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(body["data"]["connectors"][0]["toolkit"], "notion");

    // Delete: non-admin 403, admin 200 (envelope, not 204).
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
    assert_eq!(res.status(), 200);

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
    assert_eq!(body["data"]["provider_type"], "mcp_server");
    assert_eq!(body["data"]["auth_type"], "none");
    assert_eq!(body["data"]["owner_id"], member_id);
    let connector_id = body["data"]["connector_id"].as_str().unwrap();

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
async fn setup_status_active_for_none_auth_pending_then_active_for_bearer() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // auth_type='none' (the default) needs nothing further — active immediately,
    // no live verification call at all (there's no credential to prove works).
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        uid,
        "admin",
    )
    .json(&json!({"name": "ss-none-tool", "url": "https://example.com"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["data"]["setup_status"], "active");
    assert!(body["data"]["setup_error"].is_null());

    // auth_type='bearer' needs a credential — pending until one is registered.
    // Points at a real, live stub backend (not example.com) since registering
    // the credential now triggers a genuine verification call.
    let bearer_url = start_stub_mcp_server_ok().await;
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        uid,
        "admin",
    )
    .json(&json!({"name": "ss-bearer-tool", "url": bearer_url, "auth_type": "bearer"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["data"]["setup_status"], "pending");
    let cid = body["data"]["connector_id"].as_str().unwrap();

    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.json::<Value>().await.unwrap()["data"]["setup_status"],
        "pending"
    );

    // Register a credential — the connector flips to active.
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        uid,
        "admin",
    )
    .json(&json!({"value": "sk-test"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(
        body["data"]["setup_status"], "active",
        "registering a credential must flip setup_status to active: {body:?}"
    );

    disallow_private_urls();
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
    let created_by_you: Vec<&str> = body["data"]["created_by_you"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    let shared_with_you: Vec<&str> = body["data"]["shared_with_you"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(created_by_you.contains(&"alice-tool"), "{created_by_you:?}");
    assert!(
        !created_by_you.contains(&"bob-tool") && !shared_with_you.contains(&"bob-tool"),
        "must not see bob's connector: created_by_you={created_by_you:?} shared_with_you={shared_with_you:?}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_single_connector_by_id_and_404_when_unreachable() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "gs-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let alice_uuid = Uuid::parse_str(alice_id).unwrap();
    let bob = create_user(&server, uid, "gs-bob").await;
    let bob_uuid = Uuid::parse_str(bob["id"].as_str().unwrap()).unwrap();

    let alice_cid = seed_custom_connector(&server, alice_uuid, "gs-alice-tool", "none").await;
    let bob_cid = seed_custom_connector(&server, bob_uuid, "gs-bob-tool", "none").await;

    // Owner can fetch their own connector directly.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{alice_cid}"))),
        alice_id,
        "gs-alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["data"]["name"], "gs-alice-tool");
    assert_eq!(body["data"]["is_owner"], true);

    // A non-owner with no grant gets 404 (not 403) — existence isn't leaked.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{bob_cid}"))),
        alice_id,
        "gs-alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    // A random unknown id also 404s, indistinguishably.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{}", Uuid::new_v4()))),
        alice_id,
        "gs-alice",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn get_single_connector_reports_has_credential_only_after_a_connection_is_stored() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let alice = create_user(&server, uid, "hc-alice").await;
    let alice_id = alice["id"].as_str().unwrap();
    let alice_uuid = Uuid::parse_str(alice_id).unwrap();

    let cid = seed_custom_connector(&server, alice_uuid, "hc-tool", "bearer").await;

    let res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/connectors/{cid}"))),
        alice_id,
        "hc-alice",
    )
    .send()
    .await
    .unwrap();
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["data"]["has_credential"], false,
        "no connection row yet: {body:?}"
    );

    seed_connection(&server, alice_uuid, cid).await;

    let res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/connectors/{cid}"))),
        alice_id,
        "hc-alice",
    )
    .send()
    .await
    .unwrap();
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["data"]["has_credential"], true,
        "connection row with encrypted_credential now exists: {body:?}"
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

    // Owner → 200 (envelope, not 204).
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
    assert_eq!(res.status(), 200);

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
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) VALUES ($1, $2, false)",
    )
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
    assert_eq!(res.status(), 200);

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

#[tokio::test]
#[serial]
async fn consumers_reports_zero_tools_used_when_connector_disabled_for_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid = Uuid::parse_str(uid).unwrap();

    let cid = seed_custom_connector(&server, uuid, "consumer-tool", "none").await;
    sqlx::query(
        "INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, 'echo'), ($1, 'add')",
    )
    .bind(cid)
    .execute(&server.db)
    .await
    .unwrap();
    let agent_id = seed_agent(&server, uuid, "consumer-agent").await;
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) VALUES ($1, $2, false)",
    )
    .bind(agent_id)
    .bind(cid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/consumers"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let agents = body["data"]["agents"].as_array().unwrap();
    let entry = agents.iter().find(|a| a["agent_id"] == agent_id.to_string()).unwrap();
    assert_eq!(entry["total_tools"], 2);
    assert_eq!(
        entry["tools_used"], 0,
        "connector disabled outright for this agent must report 0 usable tools: {entry:?}"
    );

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
    assert_eq!(body["data"]["description"], "now documented");
    assert_eq!(body["data"]["is_active"], false);

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
        res.json::<Value>().await.unwrap()["data"]["display_name"],
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
    assert_eq!(res.json::<Value>().await.unwrap()["data"]["auth_type"], "none");

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
    assert_eq!(res.json::<Value>().await.unwrap()["data"]["auth_type"], "bearer");

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
    assert_eq!(res.json::<Value>().await.unwrap()["data"]["auth_type"], "oauth2");

    disallow_private_urls();
    server.cleanup().await;
}

/// RFC 9728 direct discovery must win over the header-sniffing heuristic even
/// when they'd disagree — the bare `POST /` here returns 200 (which alone
/// would classify as "none"), but a real OAuth-capable server publishing
/// well-known metadata should still be detected as oauth2.
#[tokio::test]
#[serial]
async fn probe_prefers_rfc9728_well_known_discovery_over_header_heuristic() {
    allow_private_urls();
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    async fn well_known() -> impl IntoResponse {
        axum::Json(json!({ "resource": "x", "authorization_servers": ["https://as.example.com"] }))
    }
    let app = Router::new()
        .route("/", post(|| async { StatusCode::OK }))
        .route(
            "/.well-known/oauth-protected-resource",
            axum::routing::get(well_known),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("http://127.0.0.1:{port}/");

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors/probe")),
        uid,
        "admin",
    )
    .json(&json!({"url": url}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["data"]["auth_type"], "oauth2");

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

#[tokio::test]
#[serial]
async fn pin_unpin_and_pinned_filters_out_revoked_access() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid = Uuid::parse_str(uid).unwrap();
    let bob = create_user(&server, uid, "pin-bob").await;
    let bob_uuid = Uuid::parse_str(bob["id"].as_str().unwrap()).unwrap();

    let own_cid = seed_custom_connector(&server, uuid, "pin-own-tool", "none").await;
    let bobs_cid = seed_custom_connector(&server, bob_uuid, "pin-bobs-tool", "none").await;

    // Pin an inaccessible connector → 404, nothing pinned.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{bobs_cid}/pin"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    // Pin the owned one.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{own_cid}/pin"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = common::as_member(
        server.client.get(server.url("/api/mcp/connectors/pinned")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let names: Vec<&str> = body["data"]["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["pin-own-tool"]);

    // Unpin → list is empty again.
    let res = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{own_cid}/pin"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = common::as_member(
        server.client.get(server.url("/api/mcp/connectors/pinned")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(body["data"]["connectors"].as_array().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn recent_reflects_connection_activity_most_recent_first() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uuid = Uuid::parse_str(uid).unwrap();

    let older = seed_custom_connector(&server, uuid, "recent-older-tool", "none").await;
    let newer = seed_custom_connector(&server, uuid, "recent-newer-tool", "none").await;
    seed_connection(&server, uuid, older).await;
    // Ensure a distinct, later updated_at than `older`'s.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    seed_connection(&server, uuid, newer).await;

    let body: Value = common::as_member(
        server.client.get(server.url("/api/mcp/connectors/recent")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let names: Vec<&str> = body["data"]["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["recent-newer-tool", "recent-older-tool"],
        "most recently connected first: {names:?}"
    );

    server.cleanup().await;
}
