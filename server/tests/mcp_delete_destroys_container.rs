//! Real end-to-end test for Step 12 of docs/MCP_UPLOAD_ITERATION_PLAN.md —
//! deleting an `uploaded_build` connector must destroy its container, not
//! just remove the DB row (which is otherwise the container's only pointer,
//! leaking it forever). Real Postgres + a real `DockerRuntime` (not
//! `FakeRuntime`), same fixture MCP server Steps 8/10 already use.
//!
//!   cargo test -p nasiko-server --test mcp_delete_destroys_container -- --test-threads=1

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use nasiko_runtime::{ContainerId, ContainerRuntime, DockerRuntime, DockerRuntimeConfig};
use nasiko_server::mcp::build::{BuildSource, execute_mcp_server_build};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const TEST_MCP_NETWORK: &str = "nasiko-mcp-servers-net-test";

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

fn zip_fixture() -> std::path::PathBuf {
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp-echo-server");
    let zip_path = std::env::temp_dir().join(format!("mcp-echo-server-{}.zip", Uuid::new_v4()));
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for name in [
        "Dockerfile",
        "requirements.txt",
        "pyproject.toml",
        "server.py",
    ] {
        let contents = std::fs::read(fixture_dir.join(name)).unwrap();
        zw.start_file(name, opts).unwrap();
        zw.write_all(&contents).unwrap();
    }
    zw.finish().unwrap();
    zip_path
}

async fn ensure_test_network(runtime: &DockerRuntime) {
    runtime
        .ensure_network(TEST_MCP_NETWORK)
        .await
        .expect("create test mcp network");
}

async fn insert_pending_upload(db: &sqlx::PgPool, owner_id: Uuid, name: &str) -> (Uuid, Uuid) {
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind, build_status, is_active) \
         VALUES ('mcp_server', $1, $2, 'uploaded_build', 'pending', false) RETURNING id",
    )
    .bind(owner_id)
    .bind(name)
    .fetch_one(db)
    .await
    .unwrap();

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, source_key) \
         VALUES ($1, $2, 'v1', NULL) RETURNING id",
    )
    .bind(connector_id)
    .bind(owner_id)
    .fetch_one(db)
    .await
    .unwrap();

    (connector_id, build_id)
}

#[tokio::test]
#[serial]
async fn deleting_an_uploaded_connector_destroys_its_container() {
    let docker = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("Docker must be running");
    ensure_test_network(&docker).await;
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(docker);

    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;
    let admin = init_admin(&server).await;
    let owner_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let name = format!("del-{}", Uuid::new_v4().simple());
    let (connector_id, build_id) = insert_pending_upload(&server.db, owner_id, &name).await;
    let zip_path = zip_fixture();
    let image_tag = format!("mcp-del-test-{}:v1", Uuid::new_v4().simple());

    execute_mcp_server_build(
        runtime.clone(),
        server.db.clone(),
        reqwest::Client::new(),
        build_id,
        connector_id,
        owner_id,
        name,
        BuildSource::Zip(zip_path),
        image_tag,
        HashMap::new(),
        TEST_MCP_NETWORK.to_string(),
        8080,
        vec![],
        "local".to_string(),
        String::new(),
        1,
        nasiko_orchestrator::providers::LLMProvider::from_env(reqwest::Client::new()),
        "gpt-4o-mini".to_string(),
    )
    .await;

    // Confirm the build actually landed as a real, running container before
    // testing deletion — otherwise this test would prove nothing.
    let build_status: String =
        sqlx::query_scalar("SELECT build_status FROM mcp_connectors WHERE id = $1")
            .bind(connector_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        build_status, "running",
        "fixture build must succeed for this test to be meaningful"
    );

    let expected_id = ContainerId::from_uuid(connector_id);
    let list_before = runtime.list().await.unwrap();
    assert!(
        list_before.iter().any(|s| s.container_id == expected_id),
        "container must exist right after a successful build"
    );

    // Delete via the real HTTP endpoint, as the owner.
    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{connector_id}"))),
        &owner_id.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // The container must be gone — not just the DB row.
    let list_after = runtime.list().await.unwrap();
    assert!(
        list_after.iter().all(|s| s.container_id != expected_id),
        "deleting an uploaded_build connector must destroy its container, not just the DB row"
    );

    let row_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_connectors WHERE id = $1)")
            .bind(connector_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!row_exists, "the DB row itself must also be gone");

    server.cleanup().await;
}
