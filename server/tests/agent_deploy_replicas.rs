//! Replica bounds on the deploy paths in OSS: every registered agent deploys at one replica,
//! whatever the runtime can do, and nothing touches an autoscaler.
//!
//! `just infra` then
//! `cargo test -p nasiko-server --test agent_deploy_replicas -- --test-threads=1`

mod common;

use std::time::Duration;

use nasiko_runtime::{ContainerId, RuntimeState};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> (String, Uuid) {
    let v: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = v["user_id"].as_str().unwrap().to_string();
    (id.clone(), Uuid::parse_str(&id).unwrap())
}

/// An agent with a semver version and a recorded image, as a restart needs. Resident: these
/// tests are about the replica bounds of a running workload, which a parked agent has none of.
async fn seed_versioned_agent(server: &common::TestServer, owner_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, version, image, status, url, is_public, parking_policy) \
         VALUES ($1, $2, '1.0.0', 'x:1.0.0', 'running', 'http://127.0.0.1:9', false, 'resident') RETURNING id",
    )
    .bind(format!("replica-agent-{}", Uuid::new_v4()))
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn set_column(server: &common::TestServer, agent_id: Uuid, assignment: &str) {
    sqlx::query(&format!("UPDATE agents SET {assignment} WHERE id = $1"))
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();
}

async fn agent_name(server: &common::TestServer, agent_id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// A stopped deployment of `agent_id` with no recorded workload name, so a restart recreates it.
async fn stopped_deployment(server: &common::TestServer, agent_id: Uuid, owner_id: Uuid) -> Uuid {
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status) \
         VALUES ($1, '1.0.0', 'x:1.0.0', 'success') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO agent_deployments (agent_id, build_id, owner_id, status) \
         VALUES ($1, $2, $3, 'stopped') RETURNING id",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// `POST /api/containers` (`nasiko deploy`): deploys `image` as `name`.
async fn admin_deploy(server: &common::TestServer, admin: &str, name: &str, extra: Value) -> u16 {
    let mut body = json!({"image": "nasiko/adhoc:2", "name": name});
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    common::as_superuser(
        server.client.post(server.url("/api/containers")),
        admin,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap()
    .status()
    .as_u16()
}

/// The (min, max) of every deploy of `agent_id`, once there is one.
async fn wait_for_deploys(server: &common::TestServer, agent_id: Uuid) -> Vec<(u32, u32)> {
    let id = ContainerId::from_uuid(agent_id);
    for _ in 0..100 {
        let deployed: Vec<(u32, u32)> = server
            .runtime
            .deployed_bounds()
            .into_iter()
            .filter(|(c, ..)| *c == id)
            .map(|(_, min, max)| (min, max))
            .collect();
        if !deployed.is_empty() {
            return deployed;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Vec::new()
}

#[tokio::test]
#[serial]
async fn a_restart_that_recreates_the_agent_deploys_it_at_one_replica() {
    let server = common::TestServer::start().await;
    server.runtime.set_supports_autoscaling(true);
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_versioned_agent(&server, admin_id).await;
    set_column(&server, agent, "state_mode = 'durable'").await;
    let deployment_id = stopped_deployment(&server, agent, admin_id).await;

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/agents/deployment/{deployment_id}/restart"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "restart: {}", res.status());

    assert_eq!(wait_for_deploys(&server, agent).await, vec![(1, 1)]);
    assert!(server.runtime.applied_bounds().is_empty());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_ad_hoc_redeploy_of_a_registered_agent_ignores_the_callers_replicas() {
    let server = common::TestServer::start().await;
    server.runtime.set_supports_autoscaling(true);
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_versioned_agent(&server, admin_id).await;
    let name = agent_name(&server, agent).await;

    let status = admin_deploy(&server, &admin, &name, json!({"replicas": 4})).await;

    assert_eq!(status, 201);
    assert_eq!(
        wait_for_deploys(&server, agent).await,
        vec![(1, 1)],
        "a deploy on a runtime that can autoscale is still one replica"
    );
    assert!(
        server.runtime.applied_bounds().is_empty(),
        "OSS never calls apply_replica_bounds"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_ad_hoc_redeploy_keeps_a_writable_agent_on_one_replica() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_versioned_agent(&server, admin_id).await;
    let name = agent_name(&server, agent).await;

    assert_eq!(
        admin_deploy(
            &server,
            &admin,
            &name,
            json!({"writable": true, "replicas": 3})
        )
        .await,
        201
    );

    assert_eq!(wait_for_deploys(&server, agent).await, vec![(1, 1)]);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_ad_hoc_deploy_of_an_unregistered_image_uses_the_callers_replicas() {
    let server = common::TestServer::start().await;
    let (admin, _) = init_admin(&server).await;

    assert_eq!(
        admin_deploy(&server, &admin, "never-registered", json!({"replicas": 2})).await,
        201
    );

    let id = ContainerId::new("never-registered");
    assert_eq!(server.runtime.deployed_bounds(), vec![(id, 2, 2)]);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_manual_scale_is_never_refused_as_autoscaled() {
    let server = common::TestServer::start().await;
    server.runtime.set_supports_autoscaling(true);
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_versioned_agent(&server, admin_id).await;
    set_column(&server, agent, "state_mode = 'durable'").await;

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/containers/{agent}/scale")))
            .json(&json!({"replicas": 2})),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status().as_u16(), 200);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn registering_a_task_store_applies_no_bounds() {
    let server = common::TestServer::start_with(|config| {
        config.mcp_gateway_public_url = Some("http://cp.test/api/mcp".to_owned());
    })
    .await;
    server.runtime.set_supports_autoscaling(true);
    let (_, admin_id) = init_admin(&server).await;
    let agent = seed_versioned_agent(&server, admin_id).await;

    let token = nasiko_server::agent_state::mint_state_token(&server.db, agent)
        .await
        .expect("mint state token");
    let res = server
        .client
        .post(server.url("/api/agent-state/register"))
        .bearer_auth(token)
        .json(&json!({ "capabilities": ["tasks"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 204);

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(server.runtime.applied_bounds().is_empty());
    server.cleanup().await;
}

async fn state_mode(server: &common::TestServer, agent_id: Uuid) -> String {
    sqlx::query_scalar("SELECT state_mode FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// The state API is configured, so a deploy of a new image resets the agent's state mode and
/// the platform credentials are minted, as in production.
async fn start_with_state_api() -> common::TestServer {
    common::TestServer::start_with(|config| {
        config.mcp_gateway_public_url = Some("http://cp.test/api/mcp".to_owned());
    })
    .await
}

/// `POST /api/containers/{agent}/restart[?query]`, the status it answers.
async fn admin_restart(
    server: &common::TestServer,
    admin: &str,
    agent_id: Uuid,
    query: &str,
) -> u16 {
    common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/containers/{agent_id}/restart{query}"))),
        admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .status()
    .as_u16()
}

#[tokio::test]
#[serial]
async fn an_admin_restart_of_a_running_agent_rolls_it_in_place() {
    let server = start_with_state_api().await;
    server.runtime.set_restarts_in_place(true);
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_versioned_agent(&server, admin_id).await;
    set_column(&server, agent, "state_mode = 'durable'").await;
    let id = ContainerId::from_uuid(agent);

    assert_eq!(admin_restart(&server, &admin, agent, "").await, 200);

    assert_eq!(server.runtime.restarted(), vec![id.clone()]);
    assert!(
        server.runtime.deployed_bounds().is_empty(),
        "the workload is not destroyed and recreated"
    );
    assert_eq!(state_mode(&server, agent).await, "durable");
    let env = server
        .runtime
        .last_refreshed_env(&id)
        .expect("the replacements start with the rebuilt env");
    assert!(env.contains_key("NASIKO_AGENT_TOKEN") && env.contains_key("MCP_GATEWAY_TOKEN"));
    assert_eq!(env["PORT"], "8000", "the env is the agent's whole env");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_admin_restart_redeploys_what_cannot_be_rolled_in_place() {
    let server = start_with_state_api().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_versioned_agent(&server, admin_id).await;

    // A runtime whose restart does not apply a changed env.
    assert_eq!(admin_restart(&server, &admin, agent, "").await, 200);
    assert_eq!(wait_for_deploys(&server, agent).await, vec![(1, 1)]);
    assert!(server.runtime.restarted().is_empty());

    // An agent that is not there, and one that is stopped.
    server.runtime.set_restarts_in_place(true);
    server.runtime.set_status_follows_deploys(true);
    let missing = seed_versioned_agent(&server, admin_id).await;
    assert_eq!(admin_restart(&server, &admin, missing, "").await, 200);
    assert_eq!(wait_for_deploys(&server, missing).await.len(), 1);
    server.runtime.set_status_follows_deploys(false);
    server.runtime.set_reported_state(RuntimeState::Stopped);
    let stopped = seed_versioned_agent(&server, admin_id).await;
    assert_eq!(admin_restart(&server, &admin, stopped, "").await, 200);
    assert_eq!(wait_for_deploys(&server, stopped).await.len(), 1);
    assert!(server.runtime.restarted().is_empty());

    // A restart that pulls the image fresh.
    server.runtime.set_reported_state(RuntimeState::Running);
    let before = server.runtime.deployed_bounds().len();
    assert_eq!(
        admin_restart(&server, &admin, agent, "?refresh=true").await,
        200
    );
    assert_eq!(server.runtime.deployed_bounds().len(), before + 1);
    assert!(server.runtime.restarted().is_empty());
    server.cleanup().await;
}
