mod common;

use common::TestServer;
use nasiko_mcp_gateway::permissions::{invalidate_permission_cache, load_permission_context};
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email) VALUES ($1, $1 || '@test.local') RETURNING id",
    )
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_agent(
    server: &TestServer,
    owner: Uuid,
    name: &str,
    coding_integration: Option<&str>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, coding_agent_integration_id) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running', $3) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(coding_integration)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_toolkit(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, name, auth_config_id, auth_type) \
         VALUES ('composio', $1, 'ac_' || $1, 'oauth2') RETURNING id",
    )
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_mcp_server(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('mcp_server', 'external_url', $1, 'http://server.invalid/mcp', 'oauth2') \
         RETURNING id",
    )
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn connect(
    server: &TestServer,
    user: Uuid,
    connector: Uuid,
    status: &str,
    account: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO mcp_user_connections (user_id, connector_id, status, connected_account_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user)
    .bind(connector)
    .bind(status)
    .bind(account)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn enabled_for(server: &TestServer, agent: Uuid, connector: Uuid) -> bool {
    load_permission_context(&server.state.mcp, agent)
        .await
        .unwrap()
        .enabled_connectors
        .contains(&connector)
}

#[tokio::test]
#[serial]
async fn a_coding_harness_gets_every_toolkit_its_owner_connected_without_an_access_row() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let gmail = seed_toolkit(&server, "gmail").await;
    let calendar = seed_toolkit(&server, "googlecalendar").await;
    connect(&server, owner, gmail, "ACTIVE", Some("ca_gmail")).await;
    connect(&server, owner, calendar, "ACTIVE", Some("ca_calendar")).await;
    let claude = seed_agent(&server, owner, "owner-claude-code", Some("claude")).await;
    let codex = seed_agent(&server, owner, "owner-codex", Some("codex")).await;

    for agent in [claude, codex] {
        assert!(enabled_for(&server, agent, gmail).await);
        assert!(enabled_for(&server, agent, calendar).await);
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deployed_agent_of_the_same_owner_keeps_default_deny() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let gmail = seed_toolkit(&server, "gmail").await;
    connect(&server, owner, gmail, "ACTIVE", Some("ca_gmail")).await;
    let deployed = seed_agent(&server, owner, "deployed", None).await;

    assert!(!enabled_for(&server, deployed, gmail).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_toolkit_connection_that_is_not_finished_is_not_enabled() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let initiated = seed_toolkit(&server, "gmail").await;
    let no_account = seed_toolkit(&server, "googlecalendar").await;
    let expired = seed_toolkit(&server, "slack").await;
    connect(&server, owner, initiated, "INITIATED", None).await;
    connect(&server, owner, no_account, "ACTIVE", None).await;
    connect(&server, owner, expired, "EXPIRED", Some("ca_slack")).await;
    let claude = seed_agent(&server, owner, "owner-claude-code", Some("claude")).await;

    for connector in [initiated, no_account, expired] {
        assert!(!enabled_for(&server, claude, connector).await);
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn another_users_connected_toolkit_is_never_enabled() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let other = seed_user(&server, "other").await;
    let gmail = seed_toolkit(&server, "gmail").await;
    connect(&server, other, gmail, "ACTIVE", Some("ca_other")).await;
    let claude = seed_agent(&server, owner, "owner-claude-code", Some("claude")).await;

    assert!(!enabled_for(&server, claude, gmail).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_access_row_switching_a_toolkit_off_wins_over_the_default() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let gmail = seed_toolkit(&server, "gmail").await;
    let calendar = seed_toolkit(&server, "googlecalendar").await;
    connect(&server, owner, gmail, "ACTIVE", Some("ca_gmail")).await;
    connect(&server, owner, calendar, "ACTIVE", Some("ca_calendar")).await;
    let claude = seed_agent(&server, owner, "owner-claude-code", Some("claude")).await;
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, false)",
    )
    .bind(claude)
    .bind(gmail)
    .execute(&server.db)
    .await
    .unwrap();

    assert!(!enabled_for(&server, claude, gmail).await);
    assert!(enabled_for(&server, claude, calendar).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_toolkit_the_platform_turned_off_is_not_enabled() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let gmail = seed_toolkit(&server, "gmail").await;
    connect(&server, owner, gmail, "ACTIVE", Some("ca_gmail")).await;
    sqlx::query("UPDATE mcp_connectors SET is_active = false WHERE id = $1")
        .bind(gmail)
        .execute(&server.db)
        .await
        .unwrap();
    let claude = seed_agent(&server, owner, "owner-claude-code", Some("claude")).await;

    assert!(!enabled_for(&server, claude, gmail).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn only_toolkits_are_default_on_not_other_servers_the_owner_connected() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let custom = seed_mcp_server(&server, "my-server").await;
    connect(&server, owner, custom, "ACTIVE", Some("acct")).await;
    let claude = seed_agent(&server, owner, "owner-claude-code", Some("claude")).await;

    assert!(!enabled_for(&server, claude, custom).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_toolkit_connected_later_is_enabled_and_changes_the_permission_hash() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "owner").await;
    let gmail = seed_toolkit(&server, "gmail").await;
    let claude = seed_agent(&server, owner, "owner-claude-code", Some("claude")).await;
    let before = load_permission_context(&server.state.mcp, claude)
        .await
        .unwrap();
    assert!(!before.enabled_connectors.contains(&gmail));

    connect(&server, owner, gmail, "ACTIVE", Some("ca_gmail")).await;
    invalidate_permission_cache(&server.state.mcp, claude).await;
    let after = load_permission_context(&server.state.mcp, claude)
        .await
        .unwrap();

    assert!(after.enabled_connectors.contains(&gmail));
    assert_ne!(before.hash, after.hash);

    server.cleanup().await;
}
