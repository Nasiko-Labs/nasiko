//! Targeted test for Step 9's branching status-inference logic
//! (`agents::build_worker::infer_build_status`): a real `agent_builds` row and
//! a real `mcp_connector_builds` row, each independently, must map to the
//! correct `build_jobs.status` outcome.
//!
//!   cargo test -p nasiko-server --test mcp_build_status_inference -- --test-threads=1

mod common;

use nasiko_server::agents::build_worker::infer_build_status;
use serde_json::{Value, json};
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

#[tokio::test]
async fn infers_success_and_failure_for_an_agent_job() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let agent_id: Uuid =
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("agent-{}", Uuid::new_v4().simple()))
            .bind(owner_id)
            .fetch_one(&server.db)
            .await
            .unwrap();

    let success_build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status) \
         VALUES ($1, 'v1', 'img:v1', 'success') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let failed_build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status) \
         VALUES ($1, 'v2', 'img:v2', 'failed') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(
        infer_build_status(&server.db, success_build_id, false).await.as_deref(),
        Some("success")
    );
    assert_eq!(
        infer_build_status(&server.db, failed_build_id, false).await.as_deref(),
        Some("failed")
    );
    // Wrong table for this id (is_mcp_job=true against an agent_builds id) — no row, None.
    assert_eq!(infer_build_status(&server.db, success_build_id, true).await, None);

    server.cleanup().await;
}

#[tokio::test]
async fn infers_success_and_failure_for_an_mcp_connector_job() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind, build_status, is_active) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build', 'building', false) RETURNING id",
    )
    .bind(owner_id)
    .bind(format!("connector-{}", Uuid::new_v4().simple()))
    .fetch_one(&server.db)
    .await
    .unwrap();

    let success_build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, status) \
         VALUES ($1, $2, 'v1', 'success') RETURNING id",
    )
    .bind(connector_id)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let failed_build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, status) \
         VALUES ($1, $2, 'v2', 'failed') RETURNING id",
    )
    .bind(connector_id)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(
        infer_build_status(&server.db, success_build_id, true).await.as_deref(),
        Some("success")
    );
    assert_eq!(
        infer_build_status(&server.db, failed_build_id, true).await.as_deref(),
        Some("failed")
    );
    // Wrong table for this id (is_mcp_job=false against an mcp_connector_builds id) — no row, None.
    assert_eq!(infer_build_status(&server.db, success_build_id, false).await, None);

    server.cleanup().await;
}
