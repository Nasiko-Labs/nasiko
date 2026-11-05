//! A default-on system connector (the workspace) is enabled for every agent, unless the agent has
//! an access row switching it off.
//!
//!   cargo test -p nasiko-server --test mcp_system_connector_off_switch -- --test-threads=1

mod common;

use common::TestServer;
use nasiko_mcp_gateway::permissions::load_permission_context;
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &TestServer) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email) VALUES ('owner', 'owner@test.local') RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_agent(server: &TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_default_on_connector(server: &TestServer) -> Uuid {
    let connector: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors \
         (provider_type, source_kind, name, url, auth_type, auto_enable) \
         VALUES ('system', 'system', 'workspace', 'http://workspace.invalid/mcp', 'none', true) \
         RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector)
    .execute(&server.db)
    .await
    .unwrap();
    connector
}

async fn set_access(server: &TestServer, agent: Uuid, connector: Uuid, enabled: bool) {
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, $3)",
    )
    .bind(agent)
    .bind(connector)
    .bind(enabled)
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
async fn a_default_on_connector_is_enabled_without_an_access_row() {
    let server = TestServer::start().await;
    let owner = seed_user(&server).await;
    let connector = seed_default_on_connector(&server).await;
    let agent = seed_agent(&server, owner, "no-row").await;

    assert!(enabled_for(&server, agent, connector).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_access_row_switching_it_off_wins_over_the_default() {
    let server = TestServer::start().await;
    let owner = seed_user(&server).await;
    let connector = seed_default_on_connector(&server).await;
    let agent = seed_agent(&server, owner, "switched-off").await;
    set_access(&server, agent, connector, false).await;

    assert!(!enabled_for(&server, agent, connector).await);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn switching_it_off_for_one_agent_leaves_the_others_on() {
    let server = TestServer::start().await;
    let owner = seed_user(&server).await;
    let connector = seed_default_on_connector(&server).await;
    let off = seed_agent(&server, owner, "off").await;
    let untouched = seed_agent(&server, owner, "untouched").await;
    set_access(&server, off, connector, false).await;

    assert!(!enabled_for(&server, off, connector).await);
    assert!(enabled_for(&server, untouched, connector).await);

    server.cleanup().await;
}
