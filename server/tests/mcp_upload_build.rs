//! Real end-to-end integration tests for `mcp::build::execute_mcp_server_build`
//! (Step 8 of docs/MCP_UPLOAD_ITERATION_PLAN.md) — real Postgres + a real
//! `DockerRuntime` (not `FakeRuntime`) building and deploying a genuine minimal
//! MCP server fixture (`tests/fixtures/mcp-echo-server/`).
//!
//! Step 9 (build-worker wiring) and Step 10 (HTTP handlers) don't exist yet, so
//! this calls `execute_mcp_server_build` directly rather than going through a
//! real upload HTTP endpoint — it still exercises every real dependency
//! (Postgres, Docker build, Docker deploy, a live MCP handshake) that Step 8
//! itself is responsible for.
//!
//!   cargo test -p nasiko-server --test mcp_upload_build -- --test-threads=1

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use nasiko_mcp_gateway::provider::GenericMcpProvider;
use nasiko_mcp_gateway::types::{MCPServerConfig, ServerType};
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

/// Zips the checked-in fixture MCP server directory into a temp file, returning
/// its path. Mirrors what a real multipart upload handler (Step 10) would have
/// already streamed to disk before calling `execute_mcp_server_build`.
fn zip_fixture() -> std::path::PathBuf {
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp-echo-server");
    let zip_path = std::env::temp_dir().join(format!("mcp-echo-server-{}.zip", Uuid::new_v4()));
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for name in ["Dockerfile", "requirements.txt", "pyproject.toml", "server.py"] {
        let contents = std::fs::read(fixture_dir.join(name)).unwrap();
        zw.start_file(name, opts).unwrap();
        zw.write_all(&contents).unwrap();
    }
    zw.finish().unwrap();
    zip_path
}

/// Corrupts the fixture by omitting the Dockerfile — used by the failure-path test.
fn zip_fixture_without_dockerfile() -> std::path::PathBuf {
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp-echo-server");
    let zip_path = std::env::temp_dir().join(format!("mcp-echo-server-broken-{}.zip", Uuid::new_v4()));
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for name in ["requirements.txt", "server.py"] {
        let contents = std::fs::read(fixture_dir.join(name)).unwrap();
        zw.start_file(name, opts).unwrap();
        zw.write_all(&contents).unwrap();
    }
    zw.finish().unwrap();
    zip_path
}

async fn real_docker_runtime() -> Arc<dyn ContainerRuntime> {
    let docker = DockerRuntime::new(DockerRuntimeConfig::default()).await.expect("Docker must be running");
    Arc::new(docker)
}

/// Idempotently creates the MCP-servers Docker network — mirrors what
/// `oss/server/src/runtime.rs::build_docker_runtime` does once at real
/// server startup (Step 5).
async fn ensure_test_network() {
    let docker = DockerRuntime::new(DockerRuntimeConfig::default()).await.expect("Docker must be running");
    docker.ensure_network(TEST_MCP_NETWORK).await.expect("create test mcp network");
}

/// Every Docker network the given connector's container is currently attached
/// to — used to prove network segmentation (§4.3 of the original plan): an
/// uploaded MCP server's container must be reachable only via
/// `mcp_servers_network`, never the platform's own default network
/// (Postgres/Redis/agents). `container_networks` is a `DockerRuntime`
/// inherent method, not part of `ContainerRuntime`, so this needs its own
/// concrete `DockerRuntime` handle rather than the `Arc<dyn ContainerRuntime>`
/// the rest of this test file uses.
async fn container_networks(connector_id: Uuid) -> Vec<String> {
    let docker = DockerRuntime::new(DockerRuntimeConfig::default()).await.expect("Docker must be running");
    docker.container_networks(&ContainerId::from_uuid(connector_id)).await.expect("inspect container networks")
}

/// Inserts a `mcp_connectors` row + a `mcp_connector_builds` row in the shapes
/// Step 10's (not-yet-built) upload handler would produce, and returns
/// (connector_id, build_id).
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
async fn upload_builds_deploys_and_serves_real_tools() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let runtime = real_docker_runtime().await;
    ensure_test_network().await;

    let name = format!("echo-{}", Uuid::new_v4().simple());
    let (connector_id, build_id) = insert_pending_upload(&server.db, owner_id, &name).await;
    let zip_path = zip_fixture();
    let image_tag = format!("mcp-echo-test-{}:v1", Uuid::new_v4().simple());

    execute_mcp_server_build(
        runtime.clone(),
        server.db.clone(),
        reqwest::Client::new(),
        build_id,
        connector_id,
        owner_id,
        name.clone(),
        BuildSource::Zip(zip_path),
        image_tag.clone(),
        HashMap::new(),
        TEST_MCP_NETWORK.to_string(),
        8080,
        vec![],
    )
    .await;

    // Assert the connector row landed exactly where Step 8 promises.
    let row: (String, Option<String>, bool, Option<String>) = sqlx::query_as(
        "SELECT build_status, url, is_active, container_image_tag FROM mcp_connectors WHERE id = $1",
    )
    .bind(connector_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(row.0, "running", "build_status must be running");
    assert!(row.1.as_deref().unwrap().ends_with("/mcp"), "url must be resolved and end in /mcp, got {:?}", row.1);
    assert!(row.2, "is_active must be true");
    assert_eq!(row.3.as_deref(), Some(image_tag.as_str()));

    let build_row: (String, Option<String>) =
        sqlx::query_as("SELECT status, detected_runtime FROM mcp_connector_builds WHERE id = $1")
            .bind(build_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(build_row.0, "success");
    assert_eq!(build_row.1.as_deref(), Some("python"), "fixture uses FastMCP — must be detected as python");

    // Drive a REAL tools/list and tools/call against the deployed container,
    // exactly like the live gateway would (trusted=true, since this is an
    // uploaded_build connector's platform-resolved address).
    let url = row.1.unwrap();
    let cfg = MCPServerConfig {
        connector_id,
        kind: ServerType::Mcp,
        name: name.clone(),
        url,
        headers: HashMap::new(),
        transport: "streamable_http".to_string(),
        trusted: true,
    };
    let provider = GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new());
    let tools = provider.list_tools(&cfg, std::time::Duration::from_secs(10), None).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "echo");

    let call = provider
        .call_tool(&cfg, &json!(1), "echo", &json!({"message": "hello"}), std::time::Duration::from_secs(10), None)
        .await
        .unwrap();
    let text = call["result"]["structuredContent"]["result"].as_str().unwrap_or_default();
    assert_eq!(text, "hello", "full response: {call}");

    // Step 10's build-logs handler, against the real container this test just
    // deployed — proves it returns genuine stdout/stderr, not a stub.
    let logs = nasiko_server::mcp::build::get_build_logs(&server.db, &runtime, owner_id, false, connector_id, 200)
        .await
        .expect("build-logs should succeed for the owner");
    assert!(
        logs.iter().any(|line| line.contains("Uvicorn running") || line.contains("Application startup complete")),
        "expected real FastMCP/uvicorn startup output in the logs, got: {logs:?}"
    );

    // Network segmentation (original plan §4.3/§8.3's "third test", never
    // written until now): the deployed container must be reachable only via
    // the isolated mcp_servers_network — never the default network
    // Postgres/Redis/agent containers share. A container on the default
    // network too would mean a compromised uploaded server could reach the
    // platform's own infrastructure directly.
    let networks = container_networks(connector_id).await;
    assert_eq!(
        networks,
        vec![TEST_MCP_NETWORK.to_string()],
        "uploaded connector container must be attached to the isolated mcp network only, got: {networks:?}"
    );

    // Cleanup: destroy the container this test deployed.
    let _ = runtime.destroy(&ContainerId::from_uuid(connector_id)).await;
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn corrupt_upload_fails_cleanly_with_no_orphaned_container() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let owner_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let runtime = real_docker_runtime().await;
    ensure_test_network().await;

    let name = format!("broken-{}", Uuid::new_v4().simple());
    let (connector_id, build_id) = insert_pending_upload(&server.db, owner_id, &name).await;
    let zip_path = zip_fixture_without_dockerfile();
    let image_tag = format!("mcp-broken-test-{}:v1", Uuid::new_v4().simple());

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
    )
    .await;

    let row: (String, bool) = sqlx::query_as("SELECT build_status, is_active FROM mcp_connectors WHERE id = $1")
        .bind(connector_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(row.0, "failed");
    assert!(!row.1);

    let build_status: String =
        sqlx::query_scalar("SELECT status FROM mcp_connector_builds WHERE id = $1").bind(build_id).fetch_one(&server.db).await.unwrap();
    assert_eq!(build_status, "failed");

    // No container should ever have been created for this connector, since
    // validation fails before any image build/deploy is attempted.
    let expected_id = ContainerId::from_uuid(connector_id);
    let list = runtime.list().await.unwrap();
    assert!(
        list.iter().all(|s| s.container_id != expected_id),
        "a corrupt upload must never leave a container running"
    );

    server.cleanup().await;
}
