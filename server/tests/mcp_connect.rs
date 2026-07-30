//! HTTP-level tests for the unified connect / disconnect flow (custom connectors;
//! most Composio paths need a live provider and are covered by crate mockito
//! tests) — plus the public `GET /oauth/callback` Composio browser redirect
//! target (`oss/server/src/mcp/handlers/connect.rs:106`,
//! `nasiko_mcp_gateway::connect::handle_composio_callback`), for which this file
//! now stands up a mockito stub so the ACTIVE-status branch (and the redirect it
//! triggers) is reachable end-to-end.
//!
//!   cargo test -p nasiko-server --test mcp_connect -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// Point the McpState's Composio provider at a mockito stub — the only seam:
/// `Providers::new` builds `ComposioProvider` straight from these two `Config`
/// fields (see `oss/server/tests/common/mod.rs`'s `test_config`).
fn set_composio_provider(base_url: &str) {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe {
        std::env::set_var("TEST_COMPOSIO_API_KEY", "test-composio-key");
        std::env::set_var("TEST_COMPOSIO_BASE_URL", base_url);
    }
}
fn clear_composio_provider() {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe {
        std::env::remove_var("TEST_COMPOSIO_API_KEY");
        std::env::remove_var("TEST_COMPOSIO_BASE_URL");
    }
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn seed_composio_connector(
    server: &common::TestServer,
    name: &str,
    auth_config_id: &str,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, name, auth_config_id) VALUES ('composio', $1, $2) RETURNING id",
    )
    .bind(name)
    .bind(auth_config_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_pending_connection(
    server: &common::TestServer,
    user: Uuid,
    connector: Uuid,
    status: &str,
) {
    sqlx::query(
        "INSERT INTO mcp_user_connections (user_id, connector_id, status) VALUES ($1, $2, $3)",
    )
    .bind(user)
    .bind(connector)
    .bind(status)
    .execute(&server.db)
    .await
    .unwrap();
}

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

async fn seed_connector(
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

#[tokio::test]
#[serial]
async fn connect_none_auth_is_immediately_connected() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "none-tool", "none").await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({"connector_id": cid}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.json::<Value>().await.unwrap()["data"]["status"],
        "connected"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn connect_bearer_requires_and_stores_credential() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "bearer-tool", "bearer").await;

    // Missing credential → 400.
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({"connector_id": cid}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    // With credential → 200 + a connection row is stored (encrypted).
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({"connector_id": cid, "credentials": {"value": "sk-secret"}}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let (status, enc): (String, Option<String>) =
        sqlx::query_as("SELECT status, encrypted_credential FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2")
            .bind(uuid)
            .bind(cid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(status, "ACTIVE");
    assert!(enc.is_some(), "credential must be stored");
    assert_ne!(
        enc.unwrap(),
        "sk-secret",
        "credential must be encrypted at rest"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_and_disconnect_removes_connection() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "dc-tool", "bearer").await;

    common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({"connector_id": cid, "credentials": {"value": "tok"}}))
    .send()
    .await
    .unwrap();

    // list shows it.
    let body: Value = common::as_superuser(
        server.client.get(server.url("/api/mcp/connections")),
        &uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(
        body["data"]["connections"][0]["connector_id"],
        cid.to_string()
    );

    // disconnect.
    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connections/{cid}"))),
        &uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mcp_user_connections WHERE connector_id = $1")
            .bind(cid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(count, 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn connect_without_target_is_bad_request() {
    let server = common::TestServer::start().await;
    let (uid, _) = init_admin(&server).await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn connect_to_unreachable_connector_id_is_not_found() {
    let server = common::TestServer::start().await;
    let (uid, _) = init_admin(&server).await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({"connector_id": Uuid::new_v4()}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

/// `connect --url` must reuse an already-registered connector at that URL,
/// never register a new one — the same "get me using this" contract as the
/// connector-id/toolkit branches.
#[tokio::test]
#[serial]
async fn connect_by_url_reuses_existing_connector_not_registers() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "url-reuse-tool", "none").await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({"url": "https://example.com"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body = res.json::<Value>().await.unwrap();
    assert_eq!(body["data"]["status"], "connected");
    assert_eq!(
        body["data"]["connector_id"].as_str(),
        Some(cid.to_string().as_str()),
        "must reuse the existing connector at this URL, not mint a new one"
    );

    // Confirm no duplicate was created.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mcp_connectors WHERE url = 'https://example.com'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(count, 1, "connect --url must never create a duplicate");

    server.cleanup().await;
}

/// `connect --url` to a URL with no registered connector must fail clearly —
/// never silently auto-register one.
#[tokio::test]
#[serial]
async fn connect_by_url_with_no_existing_connector_is_not_found() {
    let server = common::TestServer::start().await;
    let (uid, _) = init_admin(&server).await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &uid,
        "admin",
    )
    .json(&json!({"url": "https://never-registered.example.com/mcp"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);
    let body = res.json::<Value>().await.unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("connector register"),
        "error should point the caller at `connector register`: {body:?}"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mcp_connectors WHERE url = 'https://never-registered.example.com/mcp'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(count, 0, "a failed connect must never register anything");

    server.cleanup().await;
}

// ─── GET /oauth/callback (Composio) ─────────────────────────────────────────

/// ── Vuln 3 (Medium) regression guard — FIXED in `handle_composio_callback` ──
///
/// `GET /oauth/callback` used to be a self-service open redirect: it took an
/// unauthenticated `success_url` query param and, once Composio reported the
/// caller's OWN connection ACTIVE, did `Redirect::to(success_url)` verbatim.
/// Fix #3 routes it through `net::safe_redirect`, so an off-origin target is
/// neutralized to `/`. This drives the old exploit path and confirms the
/// off-origin `success_url` no longer leaks the browser off-site.
#[tokio::test]
#[serial]
async fn composio_callback_off_origin_success_url_is_neutralized_vuln3() {
    let mut mock_server = mockito::Server::new_async().await;
    let _m = mock_server
        .mock(
            "GET",
            mockito::Matcher::Regex("/api/v3/connected_accounts.*".into()),
        )
        .with_status(200)
        .with_body(
            r#"{"items":[{"id":"ca_attacker","status":"ACTIVE","auth_config":{"id":"ac_evil"}}]}"#,
        )
        .create_async()
        .await;
    set_composio_provider(&mock_server.url());

    let server = common::TestServer::start().await;
    let (attacker_id, attacker_uuid) = init_admin(&server).await;
    let cid = seed_composio_connector(&server, "evil-toolkit", "ac_evil").await;
    seed_pending_connection(&server, attacker_uuid, cid, "INITIATED").await;

    let res = no_redirect_client()
        .get(server.url("/oauth/callback"))
        .query(&[
            ("user_id", attacker_id.as_str()),
            ("connector_id", &cid.to_string()),
            ("success_url", "https://evil.example.com"),
        ])
        .send()
        .await
        .unwrap();

    // The ACTIVE branch still completes with a 303, but safe_redirect rewrites
    // the off-origin target to "/" — never the attacker's domain.
    assert_eq!(res.status(), 303);
    let location = res.headers().get("location").and_then(|v| v.to_str().ok());
    assert_eq!(
        location,
        Some("/"),
        "off-origin success_url must be neutralized to '/', not honored"
    );
    assert_ne!(location, Some("https://evil.example.com"));

    clear_composio_provider();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn composio_callback_missing_params_is_message_not_redirect() {
    let server = common::TestServer::start().await;

    // No user_id/connector_id at all.
    let res = no_redirect_client()
        .get(server.url("/oauth/callback"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "missing params must render the message page, not redirect"
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("Missing user_id or connector_id"), "{body}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn composio_callback_no_pending_connection_is_message() {
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid = seed_composio_connector(&server, "no-pending-toolkit", "ac_no_pending").await;
    // Deliberately no mcp_user_connections row for (uuid, cid) at all.

    let res = no_redirect_client()
        .get(server.url("/oauth/callback"))
        .query(&[
            ("user_id", uuid.to_string().as_str()),
            ("connector_id", &cid.to_string()),
            ("success_url", "https://evil.example.com"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "no pending connection must render the message page, not redirect"
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("No pending connection"), "{body}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn composio_callback_expired_connection_is_message() {
    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid = seed_composio_connector(&server, "expired-toolkit", "ac_expired").await;
    seed_pending_connection(&server, uuid, cid, "EXPIRED").await;

    let res = no_redirect_client()
        .get(server.url("/oauth/callback"))
        .query(&[
            ("user_id", uuid.to_string().as_str()),
            ("connector_id", &cid.to_string()),
            ("success_url", "https://evil.example.com"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "an EXPIRED connection must render the message page, not redirect"
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("No pending connection"), "{body}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn composio_callback_non_active_status_is_finalizing_message_not_redirect() {
    let mut mock_server = mockito::Server::new_async().await;
    let _m = mock_server
        .mock(
            "GET",
            mockito::Matcher::Regex("/api/v3/connected_accounts.*".into()),
        )
        .with_status(200)
        .with_body(r#"{"items":[]}"#) // no matching item -> ConnectionStatus::NOT_FOUND
        .create_async()
        .await;
    set_composio_provider(&mock_server.url());

    let server = common::TestServer::start().await;
    let (_uid, uuid) = init_admin(&server).await;
    let cid = seed_composio_connector(&server, "still-pending-toolkit", "ac_pending").await;
    seed_pending_connection(&server, uuid, cid, "INITIATED").await;

    let res = no_redirect_client()
        .get(server.url("/oauth/callback"))
        .query(&[
            ("user_id", uuid.to_string().as_str()),
            ("connector_id", &cid.to_string()),
            ("success_url", "https://evil.example.com"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "a non-ACTIVE Composio status must not redirect"
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("still finalizing"), "{body}");

    clear_composio_provider();
    server.cleanup().await;
}
