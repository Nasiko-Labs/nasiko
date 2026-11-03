//! NAS-1112: the embedded registry decided who owns a repository by comparing
//! the repo path segment with `agents.name`. Real repository names do not match
//! that column:
//! - an agent's image goes to `nasiko/<slug of its name>` (lowercased, spaces
//!   to dashes), so a name with uppercase or spaces never matched;
//! - an uploaded MCP connector's image goes to `nasiko/mcp-<name>`, which is not
//!   an agent name at all;
//! - agent names are unique per owner only, so two owners' `foo` share one repo.
//!
//! An unmatched repo counted as "unclaimed", and any signed-in user could push
//! to it, overwriting the tag the owner's next pod start pulls.
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
    sqlx::query(
        "INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', 'img:1')",
    )
    .bind(name)
    .bind(owner)
    .execute(db)
    .await
    .unwrap();
}

async fn insert_uploaded_connector(db: &PgPool, name: &str, owner: Uuid) {
    sqlx::query(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind, build_status, is_active) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build', 'pending', true)",
    )
    .bind(owner)
    .bind(name)
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

/// The registry finds a repository's owners with a SQL copy of
/// `image_name_slug`. Pushing as a stranger to the repository the Rust rule
/// names must be refused for every one of these names; a drift between the two
/// copies shows up as a 201 here.
#[tokio::test]
#[serial]
async fn the_registry_and_the_server_agree_on_every_repository_name() {
    let server = common::TestServer::start().await;
    let owner = insert_user(&server.db, "dummy-owner").await;
    let stranger = insert_user(&server.db, "dummy-stranger").await;
    let names = [
        "Infrastructure Manager",
        "infrastructure_manager",
        "Code Reviewer 2.0",
        "  Leading and trailing  ",
        "--dashes--",
        "Ünïcode Agent",
        "---",
    ];
    for name in names {
        insert_agent(&server.db, name, owner).await;
        let repo = nasiko_server::agents::image_name_slug(name);
        assert_eq!(
            push_status(&server, stranger, &repo).await,
            StatusCode::FORBIDDEN,
            "{name:?} → {repo}"
        );
    }

    server.cleanup().await;
}
