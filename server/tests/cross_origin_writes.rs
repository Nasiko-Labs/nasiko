//! NAS-1239: every workspace runs at `<sub>.<platform domain>`, so all
//! workspaces are the same site, and the browser sends one workspace's session
//! cookie on a request that a page on another workspace started. Writes
//! authenticated only by that cookie now have to come from this origin or
//! from an origin in `CORS_ALLOWED_ORIGINS`.
//!
//!   cargo test -p nasiko-server --test cross_origin_writes -- --test-threads=1
mod common;

use reqwest::StatusCode;
use serde_json::{Value, json};
use serial_test::serial;

const DASHBOARD: &str = "https://app.nasiko.com";
const SIBLING: &str = "https://acme.app.nasiko.com";

/// Creates the admin; returns their session token.
async fn admin_token(server: &common::TestServer) -> String {
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    admin["token"].as_str().unwrap().to_owned()
}

/// Creates a user to be deleted; returns its id.
async fn create_user(server: &common::TestServer, token: &str, name: &str) -> String {
    let user: Value = server
        .client
        .post(server.url("/api/users"))
        .bearer_auth(token)
        .json(&json!({"username": name, "email": format!("{name}@test.local")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    user["id"].as_str().unwrap().to_owned()
}

/// `DELETE /api/users/{id}` has no body, so a page on another origin can send
/// it without a CORS preflight. Only the session cookie authenticates it.
async fn delete_with_cookie(
    server: &common::TestServer,
    token: &str,
    user: &str,
    fetch_site: &str,
    origin: &str,
) -> StatusCode {
    server
        .client
        .delete(server.url(&format!("/api/users/{user}")))
        .header("cookie", format!("access_token={token}"))
        .header("sec-fetch-site", fetch_site)
        .header("origin", origin)
        .send()
        .await
        .unwrap()
        .status()
}

async fn user_exists(server: &common::TestServer, user: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1::uuid AND deleted_at IS NULL)",
    )
    .bind(user)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn a_sibling_workspace_cannot_write_with_the_cookie() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;
    let victim = create_user(&server, &token, "dummy-user").await;

    assert_eq!(
        delete_with_cookie(&server, &token, &victim, "same-site", SIBLING).await,
        StatusCode::FORBIDDEN
    );
    assert!(user_exists(&server, &victim).await);

    // The same request with the token as a bearer header is not ambient, so
    // it is not refused: only the browser's own cookie is.
    let bearer = server
        .client
        .delete(server.url(&format!("/api/users/{victim}")))
        .bearer_auth(&token)
        .header("sec-fetch-site", "same-site")
        .header("origin", SIBLING)
        .send()
        .await
        .unwrap()
        .status();
    assert!(bearer.is_success(), "{bearer}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn same_origin_and_the_allowlisted_dashboard_can_write() {
    let server = common::TestServer::start_with(|config| {
        config.cors_allowed_origins = vec![DASHBOARD.to_owned()];
    })
    .await;
    let token = admin_token(&server).await;
    let victim = create_user(&server, &token, "dummy-user").await;

    // The allowlisted dashboard is a different subdomain, so the browser says
    // same-site; its origin is what lets it through.
    assert_ne!(
        delete_with_cookie(&server, &token, &victim, "same-site", DASHBOARD).await,
        StatusCode::FORBIDDEN
    );
    assert!(!user_exists(&server, &victim).await);

    let second = create_user(&server, &token, "dummy-user-2").await;
    let own_origin = server.base_url.clone();
    assert_ne!(
        delete_with_cookie(&server, &token, &second, "same-origin", &own_origin).await,
        StatusCode::FORBIDDEN
    );
    assert!(!user_exists(&server, &second).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_sibling_workspace_cannot_delete_an_image_tag_with_the_cookie() {
    let server = common::TestServer::start().await;
    let token = admin_token(&server).await;

    let status = server
        .client
        .delete(server.url("/v2/admin/dummy-agent/manifests/latest"))
        .header("cookie", format!("access_token={token}"))
        .header("sec-fetch-site", "same-site")
        .header("origin", SIBLING)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);

    // With the token as a bearer header the request reaches the registry,
    // which answers for the tag itself (it does not exist).
    let bearer = server
        .client
        .delete(server.url("/v2/admin/dummy-agent/manifests/latest"))
        .bearer_auth(&token)
        .header("sec-fetch-site", "same-site")
        .header("origin", SIBLING)
        .send()
        .await
        .unwrap()
        .status();
    assert_ne!(bearer, StatusCode::FORBIDDEN);

    server.cleanup().await;
}
