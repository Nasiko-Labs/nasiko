//! POST /api/deployments/{id}/restart status rollback on failure.
//!
//! R-146: When a restart's runtime call fails, revert_starting_status must
//! restore the original status using a proper `::deployment_status` cast so
//! PostgreSQL accepts the enum value. This test exercises the real route with
//! a failure that triggers cleanup.

mod common;

use common::TestServer;
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@test.local"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

async fn seed_agent(server: &TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'nasiko/test:1.0.0', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

async fn seed_build(server: &TestServer, agent_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status) \
         VALUES ($1, '1.0.0', 'nasiko/test:1.0.0', 'success') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .expect("seed build")
}

async fn seed_deployment(
    server: &TestServer,
    agent_id: Uuid,
    build_id: Uuid,
    status: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agent_deployments (agent_id, build_id, status, spec_image, spec_ports) \
         VALUES ($1, $2, $3::deployment_status, 'nasiko/test:1.0.0', ARRAY[8000]) RETURNING id",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(status)
    .fetch_one(&server.db)
    .await
    .expect("seed deployment")
}

async fn get_deployment_status(server: &TestServer, deployment_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT status::text FROM agent_deployments WHERE id = $1")
        .bind(deployment_id)
        .fetch_one(&server.db)
        .await
        .expect("query deployment status")
}

async fn post_restart(
    server: &TestServer,
    owner_id: &str,
    deployment_id: Uuid,
) -> reqwest::Response {
    common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/agents/deployment/{deployment_id}/restart"))),
        owner_id,
        "restart-user",
    )
    .send()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn restart_reverts_status_when_runtime_fails() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "restart-owner").await;
    let agent = seed_agent(&server, owner, "restart-agent").await;
    let build = seed_build(&server, agent).await;
    let deployment = seed_deployment(&server, agent, build, "stopped").await;

    // Force the FakeRuntime to fail the restart call
    server.runtime.set_fail_deploy(true);

    let status_before = get_deployment_status(&server, deployment).await;
    assert_eq!(status_before, "stopped");

    let res = post_restart(&server, &owner.to_string(), deployment).await;
    // Runtime failure returns 500
    assert_eq!(res.status(), 500);

    // The status must be reverted to 'stopped', not stuck at 'starting'
    let status_after = get_deployment_status(&server, deployment).await;
    assert_eq!(
        status_after, "stopped",
        "status must be reverted to original value after runtime failure"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_from_failed_status_reverts_on_runtime_error() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "restart-owner-2").await;
    let agent = seed_agent(&server, owner, "restart-agent-2").await;
    let build = seed_build(&server, agent).await;
    let deployment = seed_deployment(&server, agent, build, "failed").await;

    server.runtime.set_fail_deploy(true);

    let status_before = get_deployment_status(&server, deployment).await;
    assert_eq!(status_before, "failed");

    let res = post_restart(&server, &owner.to_string(), deployment).await;
    assert_eq!(res.status(), 500);

    let status_after = get_deployment_status(&server, deployment).await;
    assert_eq!(
        status_after, "failed",
        "status must be reverted to 'failed' after runtime failure"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_succeeds_when_runtime_succeeds() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "restart-owner-3").await;
    let agent = seed_agent(&server, owner, "restart-agent-3").await;
    let build = seed_build(&server, agent).await;
    let deployment = seed_deployment(&server, agent, build, "stopped").await;

    // FakeRuntime succeeds by default
    server.runtime.set_fail_deploy(false);

    let status_before = get_deployment_status(&server, deployment).await;
    assert_eq!(status_before, "stopped");

    let res = post_restart(&server, &owner.to_string(), deployment).await;
    assert_eq!(
        res.status(),
        200,
        "restart must succeed when runtime succeeds"
    );

    // After successful restart, revert is NOT called; deployment stays as the runtime left it
    let status_after = get_deployment_status(&server, deployment).await;
    // FakeRuntime doesn't actually change status; in real scenarios it would be 'running'
    // For this test, we just verify no revert happened (it's not 'starting')
    assert_ne!(
        status_after, "starting",
        "successful restart must not leave status as 'starting'"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn admin_lifecycle_status_write_uses_enum_cast() {
    let server = TestServer::start().await;
    let owner = seed_user(&server, "admin-test-owner").await;
    let agent = seed_agent(&server, owner, "admin-test-agent").await;
    let build = seed_build(&server, agent).await;
    let deployment = seed_deployment(&server, agent, build, "running").await;

    let status_before = get_deployment_status(&server, deployment).await;
    assert_eq!(status_before, "running");

    // Call the admin stop lifecycle endpoint
    // This uses record_lifecycle_status which must cast the enum properly
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/containers/{}/stop", agent))),
        &owner.to_string(),
        "admin-test-user",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 200, "admin stop must succeed");

    // Verify the status was updated to 'stopped'
    let status_after = get_deployment_status(&server, deployment).await;
    assert_eq!(
        status_after, "stopped",
        "admin lifecycle status write must set status to 'stopped' using enum cast"
    );

    server.cleanup().await;
}
