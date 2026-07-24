//! Real end-to-end test for Step 13 of docs/MCP_UPLOAD_ITERATION_PLAN.md —
//! self-heal of a stale `uploaded_build` connector address. Real Postgres +
//! a real `DockerRuntime` (not `FakeRuntime`), the same fixture MCP server
//! Steps 8/10/12 already use, driven through the real `/api/mcp` gateway
//! route exactly the way `mcp_e2e_agent_flow.rs` does.
//!
//! ## Deviation from the plan's literal "force a new IP" method
//!
//! The plan suggested `docker restart` or `docker network disconnect`+
//! `connect` to force the container onto a different IP. Both are real but
//! non-deterministic on this host's Docker bridge network (IP reuse can hand
//! the container back the exact same address it just had, making the test
//! flaky through no fault of the code under test). Instead, this test
//! deliberately corrupts the *stored* `mcp_connectors.url` to an unreachable
//! address after a real successful build/deploy — the container itself is
//! never touched and keeps running at its real, live address the whole time.
//! This reproduces the exact condition Step 13 exists to fix (DB says one
//! address, the container's real address is different) deterministically,
//! and is a strictly stronger proof than IP reassignment would have been:
//! it guarantees the first call genuinely fails at the connection level
//! against the address on record, before self-heal corrects it.
//!
//!   cargo test -p nasiko-server --test mcp_endpoint_self_heal -- --test-threads=1

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use nasiko_auth::jwt::mint_delegation_token;
use nasiko_mcp_gateway::types::connector_prefix;
use nasiko_runtime::{ContainerRuntime, DockerRuntime, DockerRuntimeConfig};
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

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, is_public) VALUES ($1, $2, 'x:1', 'stopped', false) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

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
async fn stale_stored_endpoint_self_heals_on_the_next_tool_call() {
    let docker = DockerRuntime::new(DockerRuntimeConfig::default()).await.expect("Docker must be running");
    docker.ensure_network(TEST_MCP_NETWORK).await.expect("create test mcp network");
    let runtime: Arc<dyn ContainerRuntime> = Arc::new(docker);

    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;
    let admin = init_admin(&server).await;
    let owner_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();
    let agent_id = seed_agent(&server, owner_id, "self-heal-agent").await;

    let name = format!("heal-{}", Uuid::new_v4().simple());
    let (connector_id, build_id) = insert_pending_upload(&server.db, owner_id, &name).await;
    let zip_path = zip_fixture();
    let image_tag = format!("mcp-heal-test-{}:v1", Uuid::new_v4().simple());

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
    )
    .await;

    let real_url: String =
        sqlx::query_scalar("SELECT url FROM mcp_connectors WHERE id = $1").bind(connector_id).fetch_one(&server.db).await.unwrap();
    assert!(real_url.ends_with("/mcp"), "fixture build must succeed for this test to be meaningful: {real_url}");

    // Corrupt the stored address — the container itself is never touched and
    // keeps serving at `real_url` for the entire rest of this test. Port 1 is
    // a well-known refused-connection target, guaranteeing the first call
    // genuinely fails at the connection level, not an application error.
    sqlx::query("UPDATE mcp_connectors SET url = 'http://127.0.0.1:1/mcp' WHERE id = $1")
        .bind(connector_id)
        .execute(&server.db)
        .await
        .unwrap();

    let token = mint_delegation_token(common::TEST_JWT_SECRET, &owner_id.to_string(), &agent_id.to_string())
        .expect("mint delegation token");
    let tool = format!("{}__echo", connector_prefix(connector_id));

    let res = server
        .client
        .post(server.url("/api/mcp"))
        .header("x-nasiko-agent-token", &token)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": {"message": "hello"}},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(body.get("error").is_none(), "the call must succeed after self-healing past the stale address: {body:?}");
    let text = body["result"]["structuredContent"]["result"].as_str().unwrap_or_default();
    assert_eq!(text, "hello", "must be the real fixture server's response, not a stub: {body:?}");

    // The stored row must reflect the correction, not just this one request —
    // proves the fix persisted (mark_running's own write path, reused).
    let healed_url: String =
        sqlx::query_scalar("SELECT url FROM mcp_connectors WHERE id = $1").bind(connector_id).fetch_one(&server.db).await.unwrap();
    assert_eq!(healed_url, real_url, "the corrected address must be persisted back to mcp_connectors.url");

    let _ = runtime.destroy(&nasiko_runtime::ContainerId::from_uuid(connector_id)).await;
    server.cleanup().await;
}
