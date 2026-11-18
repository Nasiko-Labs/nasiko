//! NAS-1112: who owns a legacy `nasiko/<slug>` repository. Ownership comes from the
//! stored image (an agent's `agents.image`, an uploaded MCP connector's successful
//! build `image_tag`), so a name with uppercase or spaces and a connector's
//! `mcp-<name>` repository are both claimed. Agent names are unique per owner only,
//! so two owners' `foo` share one repository, and then neither may write it.
//!
//!   cargo test -p nasiko-server --test oci_repo_ownership -- --test-threads=1
mod common;

use reqwest::StatusCode;
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_user(db: &PgPool, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@oci-ownership-test.com"))
    .fetch_one(db)
    .await
    .unwrap()
}

async fn insert_agent(db: &PgPool, name: &str, owner: Uuid) {
    let slug = nasiko_server::agents::image_name_slug(name);
    let image = format!("nasiko/{}:1.0.0", slug);
    sqlx::query("INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', $3)")
        .bind(name)
        .bind(owner)
        .bind(image)
        .execute(db)
        .await
        .unwrap();
}

async fn insert_uploaded_connector(db: &PgPool, name: &str, owner: Uuid) {
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind, build_status, is_active) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build', 'pending', true) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(db)
    .await
    .unwrap();

    let image_tag = format!("nasiko/mcp-{}:1.0.0", name);
    sqlx::query(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, image_tag, status) \
         VALUES ($1, $2, '1.0.0', $3, 'success')",
    )
    .bind(connector_id)
    .bind(owner)
    .bind(image_tag)
    .execute(db)
    .await
    .unwrap();
}

/// What a session push of a manifest to `nasiko/<repo>:1.0.0` gets back. The
/// body is not a valid manifest, so a request that passes the ownership check
/// fails later with 400: only a 403 means the registry refused the caller.
async fn push_status(server: &common::TestServer, user: Uuid, repo: &str) -> StatusCode {
    common::as_member(
        server
            .client
            .put(server.url(&format!("/v2/nasiko/{repo}/manifests/1.0.0"))),
        &user.to_string(),
        "dummy-user",
    )
    .header("content-type", "application/vnd.oci.image.manifest.v1+json")
    .body("{}")
    .send()
    .await
    .unwrap()
    .status()
}

async fn pull_status(server: &common::TestServer, user: Uuid, repo: &str) -> StatusCode {
    common::as_member(
        server
            .client
            .get(server.url(&format!("/v2/nasiko/{repo}/tags/list"))),
        &user.to_string(),
        "dummy-user",
    )
    .send()
    .await
    .unwrap()
    .status()
}

#[tokio::test]
#[serial]
async fn only_the_owner_may_push_to_an_agent_repo_whose_name_has_uppercase_or_spaces() {
    let server = common::TestServer::start().await;
    let owner = insert_user(&server.db, "dummy-owner").await;
    let stranger = insert_user(&server.db, "dummy-stranger").await;
    insert_agent(&server.db, "General Assistant", owner).await;

    assert_eq!(
        push_status(&server, stranger, "general-assistant").await,
        StatusCode::FORBIDDEN
    );
    assert_ne!(
        push_status(&server, owner, "general-assistant").await,
        StatusCode::FORBIDDEN
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn only_the_owner_may_push_to_an_uploaded_connector_repo() {
    let server = common::TestServer::start().await;
    let owner = insert_user(&server.db, "dummy-owner").await;
    let stranger = insert_user(&server.db, "dummy-stranger").await;
    insert_uploaded_connector(&server.db, "weather", owner).await;

    assert_eq!(
        push_status(&server, stranger, "mcp-weather").await,
        StatusCode::FORBIDDEN
    );
    assert_ne!(
        push_status(&server, owner, "mcp-weather").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        pull_status(&server, stranger, "mcp-weather").await,
        StatusCode::FORBIDDEN
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_same_named_agent_of_another_owner_gives_no_access() {
    let server = common::TestServer::start().await;
    let first = insert_user(&server.db, "dummy-first").await;
    let second = insert_user(&server.db, "dummy-second").await;
    insert_agent(&server.db, "foo", first).await;
    insert_agent(&server.db, "foo", second).await;

    // Both agents' images would go to `nasiko/foo`. Neither owner may write
    // there: the repository cannot say whose image it holds.
    assert_eq!(
        push_status(&server, second, "foo").await,
        StatusCode::FORBIDDEN
    );

    server.cleanup().await;
}
