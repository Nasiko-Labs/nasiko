//! Registry A2A ACL tests: the `/a2a/v1` endpoint respects authentication and
//! filters agents by ACL (unauthenticated sees only public agents; authenticated
//! user sessions and agent credentials see owned + public + granted + org-granted
//! agents). The registry delegates to `nasiko_orchestrator::agent_registry::get_agents_for_user`,
//! which calls `state.auth.org_granted_agent_ids()` to include org grants (team/department
//! in EE). In OSS, org_granted_agent_ids returns empty, so these tests verify the
//! owned + public + user grants path; EE tests would exercise org grants.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test registry_a2a_acl -- --test-threads=1
mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

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

async fn seed_user(server: &common::TestServer, username: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users (username, email, role) VALUES ($1, $2, 'member') RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@test.local"))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_agent(
    server: &common::TestServer,
    owner_id: Uuid,
    name: &str,
    is_public: bool,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, is_public) \
         VALUES ($1, $2, 'img:1.0.0', 'running', $3) RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .bind(is_public)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn grant_agent_to_user(server: &common::TestServer, agent_id: Uuid, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grantee_id, grant_type) \
         VALUES ($1, $2, 'user')",
    )
    .bind(agent_id)
    .bind(user_id.to_string())
    .execute(&server.db)
    .await
    .unwrap();
}

async fn registry_call(client: &reqwest::Client, url: &str) -> Value {
    client
        .post(url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "test-req",
            "method": "message/send",
            "params": {"message": {"messageId": "1", "role": "user", "parts": []}},
        }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn registry_call_with_token(client: &reqwest::Client, url: &str, token: &str) -> Value {
    client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "test-req",
            "method": "message/send",
            "params": {"message": {"messageId": "1", "role": "user", "parts": []}},
        }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

fn extract_agent_names(response: &Value) -> Vec<String> {
    response
        .pointer("/result/artifacts/0/parts/0/data/agents")
        .and_then(Value::as_array)
        .map(|agents| {
            agents
                .iter()
                .filter_map(|a| a["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
#[serial]
async fn unauthenticated_sees_only_public_agents() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    let user_a = seed_user(&server, "user_a").await;
    let user_b = seed_user(&server, "user_b").await;

    // User A creates a private and a public agent.
    let _agent_a_private = seed_agent(&server, user_a, "agent-a-private", false).await;
    let _agent_a_public = seed_agent(&server, user_a, "agent-a-public", true).await;
    // User B creates a private agent.
    let _agent_b_private = seed_agent(&server, user_b, "agent-b-private", false).await;

    // Unauthenticated call should see only public agents.
    let resp = registry_call(&server.client, &server.url("/a2a/v1")).await;
    let names = extract_agent_names(&resp);

    assert!(
        names.contains(&"agent-a-public".to_string()),
        "unauthenticated should see public agent"
    );
    assert!(
        !names.contains(&"agent-a-private".to_string()),
        "unauthenticated should not see private agent"
    );
    assert!(
        !names.contains(&"agent-b-private".to_string()),
        "unauthenticated should not see other private agent"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn authenticated_user_sees_owned_public_and_granted() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    let user_a = seed_user(&server, "user_a").await;
    let user_b = seed_user(&server, "user_b").await;

    // Create user A's token.
    let token_a = common::sign_token(&user_a.to_string(), "user_a", false, "member");

    // User A creates agents.
    let _agent_a_private = seed_agent(&server, user_a, "agent-a-private", false).await;
    let _agent_a_public = seed_agent(&server, user_a, "agent-a-public", true).await;
    // User B creates agents.
    let agent_b_private = seed_agent(&server, user_b, "agent-b-private", false).await;
    let _agent_b_public = seed_agent(&server, user_b, "agent-b-public", true).await;

    // User B grants agent_b_private to user A.
    grant_agent_to_user(&server, agent_b_private, user_a).await;

    // User A's authenticated call should see: owned + public + granted.
    let resp = registry_call_with_token(&server.client, &server.url("/a2a/v1"), &token_a).await;
    let names = extract_agent_names(&resp);

    assert!(
        names.contains(&"agent-a-private".to_string()),
        "user A should see own private agent"
    );
    assert!(
        names.contains(&"agent-a-public".to_string()),
        "user A should see own public agent"
    );
    assert!(
        names.contains(&"agent-b-public".to_string()),
        "user A should see other's public agent"
    );
    assert!(
        names.contains(&"agent-b-private".to_string()),
        "user A should see granted private agent"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn authenticated_agent_sees_what_owner_sees() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    let user_a = seed_user(&server, "user_a").await;
    let user_b = seed_user(&server, "user_b").await;

    // User A creates agents.
    let agent_a1 = seed_agent(&server, user_a, "agent-a1", false).await;
    let _agent_a2 = seed_agent(&server, user_a, "agent-a2", true).await;
    // User B creates agents.
    let _agent_b1 = seed_agent(&server, user_b, "agent-b1", false).await;
    let _agent_b2 = seed_agent(&server, user_b, "agent-b2", true).await;

    // Mint an agent gateway token for agent_a1.
    let agent_token = nasiko_mcp_gateway::agent_tokens::mint(&server.db, agent_a1)
        .await
        .unwrap();

    // Agent A1's authenticated call should see what user A sees.
    let resp = registry_call_with_token(&server.client, &server.url("/a2a/v1"), &agent_token).await;
    let names = extract_agent_names(&resp);

    assert!(
        names.contains(&"agent-a1".to_string()),
        "agent should see itself"
    );
    assert!(
        names.contains(&"agent-a2".to_string()),
        "agent should see owner's other agent"
    );
    assert!(
        names.contains(&"agent-b2".to_string()),
        "agent should see other's public agent"
    );
    assert!(
        !names.contains(&"agent-b1".to_string()),
        "agent should not see other's private ungranted agent"
    );

    server.cleanup().await;
}
