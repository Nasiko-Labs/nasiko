//! Whether a deploy asks the runtime for a parkable workload (`DeploymentSpec::parkable`): a
//! framework agent whose parking policy is `auto`, read from its row at deploy time. Its keeping
//! task state in memory (`state_mode = 'memory'`, as every agent here does) changes nothing. Every
//! deploy below runs a workload; a parkable one runs one replica, so a manual scale past one is
//! refused.
//!
//! `just infra` then
//! `cargo test -p nasiko-server --test agent_deploy_parkable -- --test-threads=1`

mod common;

use std::time::Duration;

use nasiko_runtime::{ContainerId, DeploymentSpec};
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

/// A running framework agent with in-memory task state and the given parking policy.
async fn seed_agent(server: &common::TestServer, owner_id: Uuid, parking_policy: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, version, image, status, url, is_public, \
         parking_policy, state_mode) \
         VALUES ($1, $2, '1.0.0', 'x:1.0.0', 'running', 'http://127.0.0.1:9', false, $3, 'memory') \
         RETURNING id",
    )
    .bind(format!("parkable-agent-{}", Uuid::new_v4()))
    .bind(owner_id)
    .bind(parking_policy)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn agent_name(server: &common::TestServer, agent_id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// `POST /api/containers` (`nasiko deploy`): deploys an image as `name`.
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

/// `POST /api/containers/{name}/scale` (`nasiko scale`): the status and the body's `code`.
async fn admin_scale(
    server: &common::TestServer,
    admin: &str,
    name: &str,
    replicas: u32,
) -> (u16, Option<String>) {
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/containers/{name}/scale"))),
        admin,
        "admin",
    )
    .json(&json!({ "replicas": replicas }))
    .send()
    .await
    .unwrap();
    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    (status, body["code"].as_str().map(str::to_owned))
}

/// The last spec deployed for `agent_id`, once there is one.
async fn wait_for_spec(server: &common::TestServer, agent_id: Uuid) -> DeploymentSpec {
    let id = ContainerId::from_uuid(agent_id);
    for _ in 0..100 {
        if let Some(spec) = server.runtime.last_spec_for(&id) {
            return spec;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("agent {agent_id} was never deployed");
}

#[tokio::test]
#[serial]
async fn an_agent_left_at_auto_deploys_parkable() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_agent(&server, admin_id, "auto").await;
    let name = agent_name(&server, agent).await;

    assert_eq!(admin_deploy(&server, &admin, &name, json!({})).await, 201);

    let spec = wait_for_spec(&server, agent).await;
    assert!(spec.parkable, "an `auto` agent's workload is parkable");
    assert_eq!((spec.min_replicas, spec.max_replicas), (1, 1));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_resident_agent_deploys_an_always_running_workload() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_agent(&server, admin_id, "resident").await;
    let name = agent_name(&server, agent).await;

    assert_eq!(admin_deploy(&server, &admin, &name, json!({})).await, 201);

    assert!(!wait_for_spec(&server, agent).await.parkable);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deploy_as_a_coding_agent_is_never_parkable() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_agent(&server, admin_id, "auto").await;
    let name = agent_name(&server, agent).await;

    let status = admin_deploy(
        &server,
        &admin,
        &name,
        json!({"workload_kind": "acp_coding_agent"}),
    )
    .await;

    assert_eq!(status, 201);
    assert!(!wait_for_spec(&server, agent).await.parkable);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_unregistered_image_is_never_parkable() {
    let server = common::TestServer::start().await;
    let (admin, _) = init_admin(&server).await;

    assert_eq!(
        admin_deploy(&server, &admin, "never-registered", json!({})).await,
        201
    );

    let spec = server
        .runtime
        .last_spec_for(&ContainerId::new("never-registered"))
        .expect("deployed");
    assert!(!spec.parkable);
    server.cleanup().await;
}

/// A parkable agent's workload runs one replica (a Sandbox cannot run more), so a count past one is
/// refused rather than silently running one; zero and one still stop and start it. A resident
/// agent may run more.
#[tokio::test]
#[serial]
async fn scaling_a_parkable_agent_past_one_replica_is_refused() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let parkable = agent_name(&server, seed_agent(&server, admin_id, "auto").await).await;
    let resident = agent_name(&server, seed_agent(&server, admin_id, "resident").await).await;

    assert_eq!(
        admin_scale(&server, &admin, &parkable, 3).await,
        (409, Some("parkable".to_owned()))
    );
    assert_eq!(admin_scale(&server, &admin, &parkable, 1).await.0, 200);
    assert_eq!(admin_scale(&server, &admin, &parkable, 0).await.0, 200);
    assert_eq!(admin_scale(&server, &admin, &resident, 3).await.0, 200);
    server.cleanup().await;
}
