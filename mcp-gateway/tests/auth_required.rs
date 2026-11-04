//! End-to-end integration test for M3's AuthRequired runtime wiring —
//! needs infra up (`just infra` from the repo root; override the admin
//! connection with `TEST_PG_URL`), same convention `oss/hitl/tests/repo.rs`
//! and `oss/server/tests` use. Each test creates and drops its own scratch
//! database so tests can run concurrently without colliding.
//!
//! `resolved: ResolvedSession` is constructed directly (bypassing
//! `session::resolve_session`'s real connector/credential resolution,
//! already covered by `credentials.rs`'s own tests) so these tests exercise
//! exactly the new code: `protocol::handle_tools_call`'s AuthRequired
//! branch, `session::resolve_context_id`'s `session_traces` lookup, and
//! `nasiko_hitl::repo::create_pending_auth_required`'s real persistence.

use std::collections::HashMap;

use nasiko_mcp_gateway::OssConnectorAuthorizer;
use nasiko_mcp_gateway::permissions::PermissionContext;
use nasiko_mcp_gateway::protocol::handle_tools_call;
use nasiko_mcp_gateway::provider::ComposioProvider;
use nasiko_mcp_gateway::session::ResolvedSession;
use nasiko_mcp_gateway::types::{
    ConnectorUnusable, MCPServerConfig, ServerType, UnusableConnector, codes,
};
use serde_json::json;
use uuid::Uuid;

mod common;
use common::TestDb;

/// A resolved session with an empty `servers` list and one connector
/// recorded as unusable for `reason` — mirrors what a real
/// `session::resolve_session` produces for a connector whose credential is
/// missing/expired.
fn unusable_session(connector_id: Uuid, reason: ConnectorUnusable, name: &str) -> ResolvedSession {
    ResolvedSession {
        servers: Vec::<MCPServerConfig>::new(),
        connected_toolkits: vec![],
        toolkit_to_connector: HashMap::new(),
        unusable_connectors: HashMap::from([(
            connector_id,
            UnusableConnector {
                reason,
                name: name.to_string(),
            },
        )]),
    }
}

fn connector_tool_name(connector_id: Uuid, tool: &str) -> String {
    format!(
        "{}__{tool}",
        nasiko_mcp_gateway::types::connector_prefix(connector_id)
    )
}

#[tokio::test]
async fn auth_required_persists_hitl_row_and_returns_auth_required_code() {
    let db = TestDb::new(
        "mcp_auth_required_test",
        std::sync::Arc::new(OssConnectorAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "0af7651916cd43dd8448eb211c80319c";
    let session_id = "ses_test_auth_required";
    db.seed_session_trace(session_id, trace_id).await;

    let resolved = unusable_session(connector_id, ConnectorUnusable::AuthRequired, "github");
    // Enabled for this agent: `handle_tools_call`'s Layer-2 gate (`protocol.rs`) returns
    // TOOL_BLOCKED before `handle_auth_required` can run for a connector this agent may not use,
    // so an empty set never reaches the code under test here.
    let perms = db.perms(&[connector_id], vec![]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let res = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &perms,
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_eq!(
        res["error"]["code"],
        json!(codes::AUTH_REQUIRED),
        "must return the new AUTH_REQUIRED code, not the generic INVALID_PARAMS: {res}"
    );
    let hitl_request_id = res["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("response data must carry the hitl_request_id");

    let row =
        nasiko_hitl::repo::get_by_id(&db.state.db, hitl_request_id.parse().expect("valid uuid"))
            .await
            .expect("get_by_id")
            .expect("row must exist");

    assert_eq!(row.kind, nasiko_hitl::HitlKind::AuthRequired);
    assert_eq!(row.origin, nasiko_hitl::HitlOrigin::McpTool);
    assert_eq!(row.status, nasiko_hitl::HitlStatus::Pending);
    assert_eq!(row.agent_id, db.agent_id);
    assert_eq!(row.owner_user_id, db.owner_user_id);
    assert_eq!(row.connector_id, Some(connector_id));
    // The trace_id resolves to the seeded chat session — the A2A contextId —
    // not the raw trace_id, proving the session_traces lookup actually ran.
    assert_eq!(row.context_id.as_deref(), Some(session_id));
}

#[tokio::test]
async fn auth_required_falls_back_to_raw_trace_id_when_no_session_trace_exists() {
    let db = TestDb::new(
        "mcp_auth_required_test",
        std::sync::Arc::new(OssConnectorAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "1bf7651916cd43dd8448eb211c80319d";

    let resolved = unusable_session(connector_id, ConnectorUnusable::AuthRequired, "github");
    // Enabled for this agent — see the Layer-2 note in the test above.
    let perms = db.perms(&[connector_id], vec![]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let res = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &perms,
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_eq!(res["error"]["code"], json!(codes::AUTH_REQUIRED), "{res}");
    let hitl_request_id: Uuid = res["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");
    let row = nasiko_hitl::repo::get_by_id(&db.state.db, hitl_request_id)
        .await
        .expect("get_by_id")
        .expect("row must exist");

    assert_eq!(row.context_id.as_deref(), Some(trace_id));
}

#[tokio::test]
async fn repeated_calls_for_the_same_connector_and_conversation_reuse_the_same_hitl_row() {
    let db = TestDb::new(
        "mcp_auth_required_test",
        std::sync::Arc::new(OssConnectorAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "2cf7651916cd43dd8448eb211c80319e";

    let resolved = unusable_session(connector_id, ConnectorUnusable::AuthRequired, "github");
    // Enabled for this agent — see the Layer-2 note in the first test. Load-bearing here in a way
    // it isn't there: with an empty set both calls returned TOOL_BLOCKED, whose error body carries
    // no `data` at all, so the assertion below compared `Null == Null` and passed without a single
    // `hitl_requests` row ever existing — green while proving nothing.
    let perms = db.perms(&[connector_id], vec![]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &perms,
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let second = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &perms,
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    // Pinned before the equality below, which on its own is satisfied by two absent ids just as
    // well as by two matching ones.
    let first_id = first["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("the first call must persist a row and return its id");
    assert_eq!(
        first_id, second["error"]["data"]["hitl_request_id"],
        "a retried call against the same still-unusable connector must not create a second pending row"
    );
}

#[tokio::test]
async fn missing_credential_reason_never_persists_a_hitl_row() {
    let db = TestDb::new(
        "mcp_auth_required_test",
        std::sync::Arc::new(OssConnectorAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "3df7651916cd43dd8448eb211c80319f";

    let resolved = unusable_session(connector_id, ConnectorUnusable::MissingCredential, "github");
    let perms = db.perms(&[], vec![]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let res = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &perms,
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_eq!(
        res["error"]["code"],
        json!(codes::INVALID_PARAMS),
        "MissingCredential must keep today's generic error, not AUTH_REQUIRED: {res}"
    );

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hitl_requests")
        .fetch_one(&db.state.db)
        .await
        .expect("count rows");
    assert_eq!(
        count, 0,
        "no hitl_requests row should exist for a non-AuthRequired reason"
    );
}

// ─── Composio's own AuthRequired detection ──────────────────────────────────
//
// Unlike a generic connector (whose broken credential is caught before the
// call ever goes out, at `build_generic_servers` time), Composio aggregates
// every connected toolkit into one shared Tool Router session with no
// per-toolkit pre-check — so this only fires once an actual `tools/call`
// fails and `protocol::detect_composio_auth_required` re-verifies the
// specific toolkit's live status via Composio's own `/api/v3/connected_accounts`.

/// A resolved session with one Composio backend at `url` and the given
/// `toolkit -> connector_id` mapping — mirrors
/// `tool_approval.rs`'s own `composio_session` helper (private to that file).
fn composio_session(url: &str, toolkit_to_connector: HashMap<String, Uuid>) -> ResolvedSession {
    ResolvedSession {
        servers: vec![MCPServerConfig {
            connector_id: Uuid::nil(),
            kind: ServerType::Composio,
            name: "composio".into(),
            url: url.into(),
            headers: HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
            system: false,
            tool_names: vec![],
            instructions: None,
        }],
        connected_toolkits: toolkit_to_connector.keys().cloned().collect(),
        toolkit_to_connector,
        unusable_connectors: HashMap::new(),
    }
}

#[tokio::test]
async fn composio_tool_call_failure_with_inactive_connection_triggers_auth_required() {
    let db = TestDb::new(
        "mcp_auth_required_test",
        std::sync::Arc::new(OssConnectorAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "4ef7651916cd43dd8448eb211c80319g";
    let session_id = "ses_composio_auth_required";
    db.seed_session_trace(session_id, trace_id).await;

    sqlx::query(
        "INSERT INTO mcp_connectors (id, provider_type, name, auth_config_id) VALUES ($1, 'composio', 'github', 'ac_test_toolkit')",
    )
    .bind(connector_id)
    .execute(&db.state.db)
    .await
    .expect("seed composio connector");

    // Composio's REST API: the connected account for this auth_config_id is
    // no longer ACTIVE — the real signal `check_connection_status` reads,
    // exactly the same call `connect.rs::handle_composio_callback` already
    // makes on the resolve side.
    let mut status_backend = mockito::Server::new_async().await;
    let status_mock = status_backend
        .mock(
            "GET",
            mockito::Matcher::Regex("/api/v3/connected_accounts.*".into()),
        )
        .with_status(200)
        .with_body(
            r#"{"items":[{"id":"ca_dead","status":"EXPIRED","auth_config":{"id":"ac_test_toolkit"}}]}"#,
        )
        .create_async()
        .await;

    // The actual Tool Router MCP endpoint the tool call itself is forwarded
    // to — separate server, separate concern: this just needs to fail, the
    // same way a real expired-token tool call would.
    let mut toolcall_backend = mockito::Server::new_async().await;
    let toolcall_mock = toolcall_backend
        .mock("POST", "/")
        .with_status(200)
        .with_body(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Auth refresh required"}}"#,
        )
        .create_async()
        .await;

    // Cloned, not moved: `TestDb` now drops its scratch database in `Drop`, and moving a field
    // out would forbid that impl from running.
    let mut state = db.state.clone();
    state.providers.composio = Some(std::sync::Arc::new(ComposioProvider::new(
        reqwest::Client::new(),
        "test-composio-key".to_string(),
        status_backend.url(),
    )));

    let resolved = composio_session(
        &toolcall_backend.url(),
        HashMap::from([("github".to_string(), connector_id)]),
    );
    let perms = PermissionContext {
        agent_id: db.agent_id,
        enabled_connectors: [connector_id].into_iter().collect(),
        rules: vec![],
        hash: "h".into(),
    };
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let res = handle_tools_call(
        &state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": "GITHUB_LIST_REPOS", "arguments": {} }),
        &resolved,
        &perms,
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    toolcall_mock.assert_async().await;
    status_mock.assert_async().await;

    assert_eq!(
        res["error"]["code"],
        json!(codes::AUTH_REQUIRED),
        "an inactive Composio connection behind a failed call must surface as AUTH_REQUIRED, \
         not the raw backend error: {res}"
    );
    let hitl_request_id: Uuid = res["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");

    let row = nasiko_hitl::repo::get_by_id(&state.db, hitl_request_id)
        .await
        .expect("get_by_id")
        .expect("row must exist");
    assert_eq!(row.kind, nasiko_hitl::HitlKind::AuthRequired);
    assert_eq!(row.origin, nasiko_hitl::HitlOrigin::McpTool);
    assert_eq!(row.connector_id, Some(connector_id));
    assert_eq!(row.context_id.as_deref(), Some(session_id));
}

#[tokio::test]
async fn composio_tool_call_failure_with_active_connection_passes_through_unchanged() {
    let db = TestDb::new(
        "mcp_auth_required_test",
        std::sync::Arc::new(OssConnectorAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO mcp_connectors (id, provider_type, name, auth_config_id) VALUES ($1, 'composio', 'github', 'ac_test_toolkit')",
    )
    .bind(connector_id)
    .execute(&db.state.db)
    .await
    .expect("seed composio connector");

    let mut status_backend = mockito::Server::new_async().await;
    status_backend
        .mock(
            "GET",
            mockito::Matcher::Regex("/api/v3/connected_accounts.*".into()),
        )
        .with_status(200)
        .with_body(
            r#"{"items":[{"id":"ca_live","status":"ACTIVE","auth_config":{"id":"ac_test_toolkit"}}]}"#,
        )
        .create_async()
        .await;

    let mut toolcall_backend = mockito::Server::new_async().await;
    toolcall_backend
        .mock("POST", "/")
        .with_status(200)
        .with_body(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"invalid repo name"}}"#,
        )
        .create_async()
        .await;

    // Cloned, not moved: `TestDb` now drops its scratch database in `Drop`, and moving a field
    // out would forbid that impl from running.
    let mut state = db.state.clone();
    state.providers.composio = Some(std::sync::Arc::new(ComposioProvider::new(
        reqwest::Client::new(),
        "test-composio-key".to_string(),
        status_backend.url(),
    )));

    let resolved = composio_session(
        &toolcall_backend.url(),
        HashMap::from([("github".to_string(), connector_id)]),
    );
    let perms = PermissionContext {
        agent_id: db.agent_id,
        enabled_connectors: [connector_id].into_iter().collect(),
        rules: vec![],
        hash: "h".into(),
    };

    let res = handle_tools_call(
        &state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": "GITHUB_LIST_REPOS", "arguments": {} }),
        &resolved,
        &perms,
        None,
        None,
    )
    .await;

    assert_eq!(
        res["error"]["code"],
        json!(-32602),
        "a genuine application-level error on a still-ACTIVE connection must \
         pass through unchanged, not be reclassified as AUTH_REQUIRED: {res}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hitl_requests")
        .fetch_one(&state.db)
        .await
        .expect("count rows");
    assert_eq!(
        count, 0,
        "no hitl_requests row should exist when the connection is still active"
    );
}
