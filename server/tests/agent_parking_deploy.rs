//! Deploy and lifecycle semantics for parkable agents: a parkable agent deploys without a
//! workload, `stop`/`start`/`restart`/`scale` act on its sandbox and row, and startup
//! reconciliation parks it rather than redeploying it.
//!
//! `just infra` then
//! `cargo test -p nasiko-server --test agent_parking_deploy -- --test-threads=1`

mod common;

use std::collections::HashMap;

use nasiko_runtime::{
    ContainerId, ContainerRuntime, SandboxKey, SandboxProvider, SandboxProviderKind, SandboxSpec,
    SandboxState, TransportKind, UnavailableSandboxProvider,
};
use nasiko_server::agents::parking::{DeploymentMode, deployment_mode};
use nasiko_server::agents::reconcile::reconcile_agents_on_startup;
use nasiko_server::agents::upload::BuildJobPayload;
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

async fn seed_agent(
    server: &common::TestServer,
    owner_id: Uuid,
    policy: &str,
    status: &str,
) -> (Uuid, String) {
    let name = format!("parking-agent-{}", Uuid::new_v4().simple());
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, version, image, status, url, state_mode, parking_policy) \
         VALUES ($1, $2, '1.0.0', 'x:1.0.0', $3, 'http://127.0.0.1:9', 'durable', $4) RETURNING id",
    )
    .bind(&name)
    .bind(owner_id)
    .bind(status)
    .bind(policy)
    .fetch_one(&server.db)
    .await
    .unwrap();
    (id, name)
}

async fn row(server: &common::TestServer, agent_id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, url FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn post(server: &common::TestServer, admin: &str, path: &str, body: Value) -> (u16, Value) {
    let resp = common::as_superuser(server.client.post(server.url(path)), admin, "admin")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

fn sandbox_spec(agent_id: Uuid) -> SandboxSpec {
    SandboxSpec {
        key: SandboxKey::Agent { agent_id },
        image: "x:1.0.0".to_owned(),
        command: vec![],
        env: HashMap::new(),
        resources: None,
        workspace: None,
        runtime_class: None,
        ports: vec![8000],
        transport: TransportKind::Http,
        image_pull_secret: None,
    }
}

#[tokio::test]
#[serial]
async fn deploying_a_parkable_agent_leaves_it_parked_without_a_workload() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let (agent, name) = seed_agent(&server, admin_id, "auto", "registered").await;

    let (status, body) = post(
        &server,
        &admin,
        "/api/containers",
        json!({"image": "nasiko/adhoc:2", "name": name}),
    )
    .await;

    assert_eq!(status, 201);
    assert_eq!(body["state"], "parked");
    assert!(server.runtime.deployed_specs().is_empty());
    let (row_status, url) = row(&server, agent).await;
    assert_eq!(row_status, "parked");
    assert_eq!(url, None);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deploying_a_resident_agent_starts_a_workload_as_before() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let (agent, name) = seed_agent(&server, admin_id, "resident", "registered").await;

    let (status, body) = post(
        &server,
        &admin,
        "/api/containers",
        json!({"image": "nasiko/adhoc:2", "name": name}),
    )
    .await;

    assert_eq!(status, 201);
    assert_ne!(body["state"], "parked");
    assert!(
        server
            .runtime
            .last_spec_for(&ContainerId::from_uuid(agent))
            .is_some()
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn scaling_a_parked_agent_is_refused() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let (_, name) = seed_agent(&server, admin_id, "auto", "parked").await;

    let (status, body) = post(
        &server,
        &admin,
        &format!("/api/containers/{name}/scale"),
        json!({"replicas": 2}),
    )
    .await;

    assert_eq!(status, 409);
    assert_eq!(body["code"], "agent_parked");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn stopping_a_parked_agent_suspends_its_sandbox() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let (agent, name) = seed_agent(&server, admin_id, "auto", "parked").await;
    let handle = server.sandbox.acquire(&sandbox_spec(agent)).await.unwrap();

    let (status, _) = post(
        &server,
        &admin,
        &format!("/api/containers/{name}/stop"),
        json!({}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert_eq!(row(&server, agent).await.0, "parked");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn starting_a_parked_agent_acquires_nothing() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let (agent, name) = seed_agent(&server, admin_id, "auto", "parked").await;

    let (status, body) = post(
        &server,
        &admin,
        &format!("/api/containers/{name}/start"),
        json!({}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["state"], "parked");
    assert_eq!(
        server
            .sandbox
            .acquire_count(&SandboxKey::Agent { agent_id: agent }),
        0
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restarting_a_parked_agent_releases_its_sandbox() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let (agent, name) = seed_agent(&server, admin_id, "auto", "parked").await;
    let handle = server.sandbox.acquire(&sandbox_spec(agent)).await.unwrap();

    let (status, body) = post(
        &server,
        &admin,
        &format!("/api/containers/{name}/restart"),
        json!({}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["state"], "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    assert!(server.runtime.deployed_specs().is_empty());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reconcile_parks_a_parkable_agent_and_leaves_a_parked_one_alone() {
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    // Reports a workload that was never deployed as missing, like a real runtime.
    server.runtime.set_status_follows_deploys(true);
    let (running, _) = seed_agent(&server, admin_id, "auto", "running").await;
    let (parked, _) = seed_agent(&server, admin_id, "auto", "parked").await;
    let (resident, _) = seed_agent(&server, admin_id, "resident", "running").await;

    reconcile_agents_on_startup(&server.state).await;

    assert_eq!(row(&server, running).await, ("parked".to_owned(), None));
    assert_eq!(row(&server, parked).await.0, "parked");
    let deployed: Vec<_> = server
        .runtime
        .deployed_specs()
        .into_iter()
        .map(|spec| spec.container_id)
        .collect();
    assert_eq!(deployed, vec![ContainerId::from_uuid(resident)]);
    server.cleanup().await;
}

/// Polls until the agent reaches `expected` (up to 30 s) and returns its status then.
async fn settled_status(server: &common::TestServer, agent_id: Uuid, expected: &str) -> String {
    for _ in 0..300 {
        let (status, _) = row(server, agent_id).await;
        if status == expected {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    row(server, agent_id).await.0
}

async fn state_mode(server: &common::TestServer, agent_id: Uuid) -> String {
    sqlx::query_scalar("SELECT state_mode FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

fn dockerfile_zip() -> Vec<u8> {
    common::make_zip(&[
        (
            "Dockerfile",
            b"FROM python:3.11-slim\nCMD [\"python\", \"main.py\"]",
        ),
        ("main.py", b"print('hello')"),
    ])
}

async fn seed_named(
    server: &common::TestServer,
    owner_id: Uuid,
    name: &str,
    version: &str,
    status: &str,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, version, image, status, state_mode, parking_policy) \
         VALUES ($1, $2, $3, 'x:1.0.0', $4, 'durable', 'auto') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .bind(version)
    .bind(status)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn a_zip_upload_of_a_parkable_agent_parks_it() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "parked-upload", "0.9.0", "registered").await;

    let form = reqwest::multipart::Form::new()
        .text("name", "parked-upload")
        .text("version_tag", "1.0.0")
        .part(
            "source",
            reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
        );
    let res = common::as_superuser(
        server.client.post(server.url("/api/agents/upload")),
        &admin,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "upload: {}", res.status());

    assert_eq!(settled_status(&server, agent, "parked").await, "parked");
    assert!(server.runtime.deployed_specs().is_empty());
    assert_eq!(state_mode(&server, agent).await, "durable");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_upload_that_carries_env_keeps_its_workload() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "env-upload", "0.9.0", "registered").await;

    let form = reqwest::multipart::Form::new()
        .text("name", "env-upload")
        .text("version_tag", "1.0.0")
        .text("env", r#"{"FOO":"bar"}"#)
        .part(
            "source",
            reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
        );
    let res = common::as_superuser(
        server.client.post(server.url("/api/agents/upload")),
        &admin,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "upload: {}", res.status());

    assert_eq!(settled_status(&server, agent, "running").await, "running");
    assert_eq!(server.runtime.deployed_specs().len(), 1);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_clone_job_of_a_parkable_agent_parks_it() {
    use std::io::Write;
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "parked-clone", "0.9.0", "deploying").await;
    let image = "nasiko/parked-clone:1.0.0".to_owned();
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', $2) RETURNING id",
    )
    .bind(agent)
    .bind(&image)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for (file, data) in [
            ("AgentCard.json", br#"{"version": "1.0.0"}"#.as_slice()),
            ("Dockerfile", b"FROM scratch".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, file, data).unwrap();
        }
        builder.finish().unwrap();
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tar_bytes).unwrap();
    let tar_path = std::env::temp_dir().join(format!("parking-clone-{}.tar.gz", Uuid::new_v4()));
    std::fs::write(&tar_path, encoder.finish().unwrap()).unwrap();
    let payload = BuildJobPayload::Clone {
        build_id,
        agent_id: agent,
        owner_id: admin_id,
        upload_id: "parking-clone".into(),
        name: "parked-clone".into(),
        tar_gz_path: tar_path.to_string_lossy().into_owned(),
        image_tag: image,
        ports: vec![8000],
        env: HashMap::new(),
    };
    sqlx::query("INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)")
        .bind(agent)
        .bind(admin_id)
        .bind(serde_json::to_value(&payload).unwrap())
        .execute(&server.db)
        .await
        .unwrap();
    let _ = server.state.build_tx.send(()).await;

    assert_eq!(settled_status(&server, agent, "parked").await, "parked");
    assert!(server.runtime.deployed_specs().is_empty());
    assert_eq!(state_mode(&server, agent).await, "durable");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_update_of_a_parkable_agent_parks_it_and_keeps_its_state_mode() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "parked-update", "1.0.0", "running").await;

    let form = reqwest::multipart::Form::new().part(
        "source",
        reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
    );
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent}/update"))),
        &admin,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);

    assert_eq!(settled_status(&server, agent, "parked").await, "parked");
    assert!(server.runtime.deployed_specs().is_empty());
    assert_eq!(state_mode(&server, agent).await, "durable");
    let image: String = sqlx::query_scalar("SELECT image FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(image.ends_with(":1.0.1"), "{image}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_rollback_of_a_parkable_agent_parks_it() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "parked-rollback", "1.0.1", "running").await;
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '1.0.0', 'parked-rollback:1.0.0', false, true, 'archived')",
    )
    .bind(agent)
    .execute(&server.db)
    .await
    .unwrap();

    let (status, _) = post(
        &server,
        &admin,
        &format!("/api/agents/{agent}/rollback"),
        json!({}),
    )
    .await;
    assert_eq!(status, 202);

    assert_eq!(settled_status(&server, agent, "parked").await, "parked");
    assert!(server.runtime.deployed_specs().is_empty());
    let version: String = sqlx::query_scalar("SELECT version FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(version, "1.0.0");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deployment_restart_of_a_parkable_agent_parks_it() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "parked-restart", "1.0.0", "stopped").await;
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status) \
         VALUES ($1, '1.0.0', 'x:1.0.0', 'success') RETURNING id",
    )
    .bind(agent)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let deployment: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_deployments (agent_id, build_id, owner_id, status) \
         VALUES ($1, $2, $3, 'stopped') RETURNING id",
    )
    .bind(agent)
    .bind(build_id)
    .bind(admin_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let (status, _) = post(
        &server,
        &admin,
        &format!("/api/agents/deployment/{deployment}/restart"),
        json!({}),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(row(&server, agent).await, ("parked".to_owned(), None));
    assert!(server.runtime.deployed_specs().is_empty());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn without_a_usable_sandbox_provider_every_agent_deploys_a_workload() {
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    server.runtime.set_status_follows_deploys(true);
    let (agent, _) = seed_agent(&server, admin_id, "auto", "running").await;
    let mut no_sandbox = server.state.clone();
    no_sandbox.sandbox =
        std::sync::Arc::new(UnavailableSandboxProvider::new(SandboxProviderKind::Docker));

    assert_eq!(
        deployment_mode(&no_sandbox, agent).await.unwrap(),
        DeploymentMode::Workload
    );
    reconcile_agents_on_startup(&no_sandbox).await;

    assert_eq!(row(&server, agent).await.0, "running");
    assert_eq!(server.runtime.deployed_specs().len(), 1);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reconcile_leaves_a_woken_agent_serving_from_its_sandbox_alone() {
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    server.runtime.set_status_follows_deploys(true);
    let (agent, _) = seed_agent(&server, admin_id, "auto", "running").await;
    let handle = server.sandbox.acquire(&sandbox_spec(agent)).await.unwrap();
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );

    reconcile_agents_on_startup(&server.state).await;

    assert_eq!(row(&server, agent).await.0, "running");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    assert!(server.runtime.deployed_specs().is_empty());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn stop_and_start_destroy_a_stale_workload_of_a_parkable_agent() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    server.runtime.set_status_follows_deploys(true);
    // A resident agent deploys a workload; its policy then flips to auto.
    let (agent, name) = seed_agent(&server, admin_id, "resident", "registered").await;
    let (status, _) = post(
        &server,
        &admin,
        "/api/containers",
        json!({"image": "nasiko/adhoc:2", "name": name}),
    )
    .await;
    assert_eq!(status, 201);
    for _ in 0..100 {
        if row(&server, agent).await.0 == "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    sqlx::query("UPDATE agents SET parking_policy = 'auto', status = 'stopped' WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    let id = ContainerId::from_uuid(agent);
    assert!(server.runtime.status(&id).await.is_ok());

    let (status, _) = post(
        &server,
        &admin,
        &format!("/api/containers/{name}/stop"),
        json!({}),
    )
    .await;

    assert_eq!(status, 200);
    assert!(server.runtime.status(&id).await.is_err());
    assert_eq!(row(&server, agent).await.0, "parked");

    // The same for `start`.
    let (agent, name) = seed_agent(&server, admin_id, "resident", "registered").await;
    let (status, _) = post(
        &server,
        &admin,
        "/api/containers",
        json!({"image": "nasiko/adhoc:2", "name": name}),
    )
    .await;
    assert_eq!(status, 201);
    for _ in 0..100 {
        if row(&server, agent).await.0 == "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    sqlx::query("UPDATE agents SET parking_policy = 'auto', status = 'stopped' WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    let (status, body) = post(
        &server,
        &admin,
        &format!("/api/containers/{name}/start"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["state"], "parked");
    assert!(
        server
            .runtime
            .status(&ContainerId::from_uuid(agent))
            .await
            .is_err()
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn start_leaves_a_woken_agent_running_and_status_reports_a_parked_one() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    server.runtime.set_status_follows_deploys(true);
    let (_, woken) = seed_agent(&server, admin_id, "auto", "running").await;
    let (parked, parked_name) = seed_agent(&server, admin_id, "auto", "parked").await;

    let (status, body) = post(
        &server,
        &admin,
        &format!("/api/containers/{woken}/start"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["state"], "running");

    let resp = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/containers/{parked_name}"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["state"], "parked");
    assert_eq!(body["container_id"], parked.to_string());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deploy_that_carries_env_keeps_its_workload() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let (agent, name) = seed_agent(&server, admin_id, "auto", "registered").await;

    let (status, body) = post(
        &server,
        &admin,
        "/api/containers",
        json!({"image": "nasiko/adhoc:2", "name": name, "env": {"FOO": "bar"}}),
    )
    .await;

    assert_eq!(status, 201);
    assert_ne!(body["state"], "parked");
    assert!(
        server
            .runtime
            .last_spec_for(&ContainerId::from_uuid(agent))
            .is_some()
    );
    server.cleanup().await;
}

async fn put_metadata(
    server: &common::TestServer,
    admin: &str,
    agent: Uuid,
    body: Value,
) -> reqwest::StatusCode {
    common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent}"))),
        admin,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap()
    .status()
}

async fn policy(server: &common::TestServer, agent: Uuid) -> String {
    sqlx::query_scalar("SELECT parking_policy FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn the_metadata_put_sets_the_parking_policy_and_start_deploys_a_stopped_resident() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    server.runtime.set_status_follows_deploys(true);
    let agent = seed_named(&server, admin_id, "policy-agent", "1.0.0", "parked").await;

    assert_eq!(
        put_metadata(
            &server,
            &admin,
            agent,
            json!({"parking_policy": "sometimes"})
        )
        .await,
        400
    );
    assert_eq!(
        put_metadata(
            &server,
            &admin,
            agent,
            json!({"parking_policy": "resident"})
        )
        .await,
        200
    );
    assert_eq!(policy(&server, agent).await, "resident");
    assert_eq!(row(&server, agent).await.0, "stopped");
    assert!(server.runtime.deployed_specs().is_empty());

    let (status, _) = post(
        &server,
        &admin,
        "/api/containers/policy-agent/start",
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(server.runtime.deployed_specs().len(), 1);
    assert_eq!(settled_status(&server, agent, "running").await, "running");

    // A flip back to `auto` changes nothing at once.
    assert_eq!(
        put_metadata(&server, &admin, agent, json!({"parking_policy": "auto"})).await,
        200
    );
    assert_eq!(row(&server, agent).await.0, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_multipart_update_ignores_a_parking_policy_field() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "ignored-policy", "1.0.0", "parked").await;

    let form = reqwest::multipart::Form::new()
        .text("parking_policy", "resident")
        .part(
            "source",
            reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
        );
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent}/update"))),
        &admin,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 202);
    assert_eq!(policy(&server, agent).await, "auto");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_wake_after_the_row_flips_survives_the_park() {
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    let (agent, _) = seed_agent(&server, admin_id, "auto", "running").await;
    server
        .runtime
        .delay_next_destroy(std::time::Duration::from_millis(600));
    let state = server.state.clone();
    let parking = tokio::spawn(async move {
        nasiko_server::agents::parking::park_agent(&state, agent, None).await
    });
    assert_eq!(settled_status(&server, agent, "parked").await, "parked");

    // The request that arrives after the mark wakes the new image.
    let handle = server.sandbox.acquire(&sandbox_spec(agent)).await.unwrap();
    parking.await.unwrap().unwrap();

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_stale_sandbox_is_released_after_the_row_flips() {
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    let (agent, _) = seed_agent(&server, admin_id, "auto", "running").await;
    let handle = server.sandbox.acquire(&sandbox_spec(agent)).await.unwrap();
    server
        .sandbox
        .delay_next_release(std::time::Duration::from_millis(600));
    let state = server.state.clone();
    let parking = tokio::spawn(async move {
        nasiko_server::agents::parking::park_agent(&state, agent, Some("x:2")).await
    });

    // The row is already parked, on the new image, while the old sandbox is still being released.
    assert_eq!(settled_status(&server, agent, "parked").await, "parked");
    assert_ne!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    parking.await.unwrap().unwrap();

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_failed_release_is_returned_and_keeps_the_credentials() {
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    let (agent, _) = seed_agent(&server, admin_id, "auto", "running").await;
    server.sandbox.acquire(&sandbox_spec(agent)).await.unwrap();
    sqlx::query("INSERT INTO agent_sandbox_credentials (agent_id, env_enc) VALUES ($1, 'x')")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .sandbox
        .fail_next_release(nasiko_runtime::RuntimeError::Internal("boom".into()));

    let err = nasiko_server::agents::parking::park_agent(&server.state, agent, None).await;

    assert!(matches!(
        err,
        Err(nasiko_server::agents::parking::ParkError::Runtime(_))
    ));
    assert_eq!(credential_rows(&server, agent).await, 1);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reconcile_parks_a_suspended_sandbox_agent_and_skips_when_the_list_fails() {
    use nasiko_server::agents::parking::{Unserved, park_if_unserved};
    let server = common::TestServer::start().await;
    let (_, admin_id) = init_admin(&server).await;
    let (suspended, _) = seed_agent(&server, admin_id, "auto", "running").await;
    let handle = server
        .sandbox
        .acquire(&sandbox_spec(suspended))
        .await
        .unwrap();
    server.sandbox.suspend(&handle).await.unwrap();

    assert_eq!(
        park_if_unserved(&server.state, suspended).await.unwrap(),
        Unserved::Parked
    );
    assert_eq!(row(&server, suspended).await.0, "parked");

    let (unreadable, _) = seed_agent(&server, admin_id, "auto", "running").await;
    server
        .sandbox
        .fail_next_list(nasiko_runtime::RuntimeError::Internal("down".into()));
    assert!(park_if_unserved(&server.state, unreadable).await.is_err());
    assert_eq!(row(&server, unreadable).await.0, "running");
    server.cleanup().await;
}

async fn credential_rows(server: &common::TestServer, agent: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM agent_sandbox_credentials WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn an_update_releases_the_sandbox_and_clears_its_credentials_but_stop_keeps_them() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    let agent = seed_named(&server, admin_id, "creds-agent", "1.0.0", "parked").await;
    let handle = server.sandbox.acquire(&sandbox_spec(agent)).await.unwrap();
    sqlx::query("INSERT INTO agent_sandbox_credentials (agent_id, env_enc) VALUES ($1, 'x')")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();

    // Stop suspends the sandbox and keeps the credentials.
    let (status, _) = post(
        &server,
        &admin,
        &format!("/api/containers/{}/stop", "creds-agent"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert_eq!(credential_rows(&server, agent).await, 1);

    // An update to a new image releases the sandbox and clears them.
    let form = reqwest::multipart::Form::new().part(
        "source",
        reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
    );
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent}/update"))),
        &admin,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);
    assert_eq!(settled_status(&server, agent, "parked").await, "parked");

    assert_eq!(credential_rows(&server, agent).await, 0);
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn update_and_rollback_succeed_when_the_sandbox_provider_is_unavailable() {
    let server = common::TestServer::start().await;
    let (admin, admin_id) = init_admin(&server).await;
    server.sandbox.set_unavailable(true);
    let agent = seed_named(&server, admin_id, "no-provider", "1.0.0", "running").await;
    sqlx::query("INSERT INTO agent_sandbox_credentials (agent_id, env_enc) VALUES ($1, 'x')")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();

    let form = reqwest::multipart::Form::new().part(
        "source",
        reqwest::multipart::Part::bytes(dockerfile_zip()).file_name("agent.zip"),
    );
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/agents/{agent}/update"))),
        &admin,
        "admin",
    )
    .multipart(form)
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);
    assert_eq!(settled_status(&server, agent, "running").await, "running");
    assert_eq!(server.runtime.deployed_specs().len(), 1);
    assert_eq!(credential_rows(&server, agent).await, 0);

    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback, status) \
         VALUES ($1, '0.9.0', 'no-provider:0.9.0', false, true, 'archived')",
    )
    .bind(agent)
    .execute(&server.db)
    .await
    .unwrap();
    let (status, _) = post(
        &server,
        &admin,
        &format!("/api/agents/{agent}/rollback"),
        json!({}),
    )
    .await;
    assert_eq!(status, 202);
    for _ in 0..100 {
        if server.runtime.deployed_specs().len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(server.runtime.deployed_specs().len(), 2);
    server.cleanup().await;
}
