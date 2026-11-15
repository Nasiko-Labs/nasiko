//! Seed agents get the platform's Composio toolkits turned on, with deletes and Gmail sends blocked;
//! an access row an admin already set is kept, and another user's agent of the same name is not
//! touched.
//!
//!   cargo test -p nasiko-server --test seed_toolkit_access -- --test-threads=1

mod common;

use nasiko_server::seed::grant_toolkits_to_seed_agents;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> Uuid {
    let body: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["user_id"].as_str().unwrap().parse().unwrap()
}

async fn agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) VALUES ($1, $2, 'x:1', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn toolkit(server: &common::TestServer, name: &str, active: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, name, auth_config_id, is_active) \
         VALUES ('composio', $1, 'ac', $2) RETURNING id",
    )
    .bind(name)
    .bind(active)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn access(server: &common::TestServer, agent: Uuid) -> Vec<(Uuid, bool, Value)> {
    sqlx::query_as(
        "SELECT connector_id, enabled, tool_rules FROM mcp_agent_connector_access \
         WHERE agent_id = $1 ORDER BY connector_id",
    )
    .bind(agent)
    .fetch_all(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn seed_agents_get_the_active_toolkits_with_destructive_tools_blocked() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let stranger: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ('stranger', 'stranger@test.local', false) \
         RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    let summarizer = agent(&server, admin, "summarizer").await;
    let switched_off = agent(&server, admin, "translator").await;
    let not_seeded = agent(&server, admin, "my-own-agent").await;
    let strangers = agent(&server, stranger, "summarizer").await;
    let gmail = toolkit(&server, "gmail", true).await;
    let slack = toolkit(&server, "slack", true).await;
    toolkit(&server, "jira", false).await;
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) VALUES ($1, $2, false)",
    )
    .bind(switched_off)
    .bind(gmail)
    .execute(&server.db)
    .await
    .unwrap();

    // SAFETY: tests run serially (#[serial]), so no concurrent env mutation.
    unsafe {
        std::env::set_var(
            "SEED_AGENTS",
            "registry.test/nasiko/summarizer:1.1.0 registry.test/nasiko/translator:1.0.1",
        );
    }
    grant_toolkits_to_seed_agents(&server.state).await;
    // Twice: a restart must not duplicate or reset anything.
    grant_toolkits_to_seed_agents(&server.state).await;
    unsafe {
        std::env::remove_var("SEED_AGENTS");
    }

    let rows = access(&server, summarizer).await;
    let mut granted: Vec<Uuid> = rows.iter().map(|(c, _, _)| *c).collect();
    granted.sort();
    let mut expected = vec![gmail, slack];
    expected.sort();
    assert_eq!(
        granted, expected,
        "both active toolkits, not the inactive one"
    );
    let (_, enabled, rules) = &rows[0];
    assert!(enabled);
    let blocked: Vec<&str> = rules
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["stance"] == "block")
        .map(|r| r["pattern"].as_str().unwrap())
        .collect();
    assert!(
        blocked.contains(&"*delete*") && blocked.contains(&"gmail_send*"),
        "{blocked:?}"
    );

    let kept: Vec<(Uuid, bool)> = access(&server, switched_off)
        .await
        .into_iter()
        .map(|(c, e, _)| (c, e))
        .collect();
    assert!(
        kept.contains(&(gmail, false)),
        "an admin's switch-off is kept: {kept:?}"
    );
    assert!(kept.contains(&(slack, true)));

    assert!(
        access(&server, not_seeded).await.is_empty(),
        "only seed agents"
    );
    assert!(
        access(&server, strangers).await.is_empty(),
        "only the seed's own rows"
    );
    server.cleanup().await;
}
