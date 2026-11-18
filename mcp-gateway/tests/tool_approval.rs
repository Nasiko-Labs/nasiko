//! End-to-end integration test for M4's ToolApproval persistence — needs
//! infra up (`just infra` from the repo root; override the admin connection
//! with `TEST_PG_URL`), same convention `oss/mcp-gateway/tests/auth_required.rs`
//! and `oss/hitl/tests/repo.rs` use. Each test creates and drops its own
//! scratch database so tests can run concurrently without colliding.
//!
//! Exercises exactly the new M4 code: `protocol::handle_tools_call`'s
//! `ToolAccess::Ask` branch (generic MCP), `protocol::create_tool_approval_id`,
//! and `nasiko_hitl::repo::create_pending_tool_approval`'s real persistence —
//! reusing `session::resolve_context_id`, already proven by
//! `auth_required.rs`.

use std::collections::HashMap;

use nasiko_mcp_gateway::permissions::{EffectiveAccess, PermissionRule};
use nasiko_mcp_gateway::protocol::handle_tools_call;
use nasiko_mcp_gateway::session::ResolvedSession;
use nasiko_mcp_gateway::types::{MCPServerConfig, ServerType, Stance, codes};
use serde_json::json;
use uuid::Uuid;

mod common;
use common::{AllowAllAuthorizer, TestDb};

fn rule(connector_id: Uuid, pattern: &str, stance: Stance) -> PermissionRule {
    PermissionRule {
        connector_id,
        tool_pattern: pattern.into(),
        stance,
    }
}

/// A resolved session with one live generic MCP backend at a (deliberately
/// unreachable) URL — `Stance::Ask`/`Block` both return before any backend
/// call, and the `Allow` test below expects a connection failure, not a
/// success, so no mock server is needed.
fn mcp_session(connector_id: Uuid) -> ResolvedSession {
    ResolvedSession {
        servers: vec![MCPServerConfig {
            connector_id,
            kind: ServerType::Mcp,
            name: "test-connector".into(),
            url: "http://127.0.0.1:9/mcp".into(),
            headers: HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
            system: false,
            tool_names: vec![],
            instructions: None,
        }],
        connected_toolkits: vec![],
        toolkit_to_connector: HashMap::new(),
        unusable_connectors: HashMap::new(),
    }
}

fn connector_tool_name(connector_id: Uuid, tool: &str) -> String {
    format!(
        "{}__{tool}",
        nasiko_mcp_gateway::types::connector_prefix(connector_id)
    )
}

#[tokio::test]
async fn ask_persists_tool_approval_row_and_returns_tool_ask_with_id() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "4ef7651916cd43dd8448eb211c8031a0";
    let session_id = "ses_test_tool_approval";
    db.seed_session_trace(session_id, trace_id).await;

    let resolved = mcp_session(connector_id);
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let res = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_eq!(
        res["error"]["code"],
        json!(codes::TOOL_ASK),
        "must still return TOOL_ASK, unchanged from before M4: {res}"
    );
    let hitl_request_id: Uuid = res["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("response data must carry the hitl_request_id")
        .parse()
        .expect("valid uuid");

    let row = nasiko_hitl::repo::get_by_id(&db.state.db, hitl_request_id)
        .await
        .expect("get_by_id")
        .expect("row must exist");

    assert_eq!(row.kind, nasiko_hitl::HitlKind::ToolApproval);
    assert_eq!(row.origin, nasiko_hitl::HitlOrigin::McpTool);
    assert_eq!(row.status, nasiko_hitl::HitlStatus::Pending);
    assert_eq!(row.agent_id, db.agent_id);
    assert_eq!(row.owner_user_id, db.owner_user_id);
    assert_eq!(row.connector_id, Some(connector_id));
    assert_eq!(row.tool_name.as_deref(), Some("list_repos"));
    // The trace_id resolves to the seeded chat session — the A2A contextId.
    assert_eq!(row.context_id.as_deref(), Some(session_id));
}

#[tokio::test]
async fn repeated_ask_for_the_same_tool_and_conversation_reuses_the_same_row() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "5ff7651916cd43dd8448eb211c8031a1";

    let resolved = mcp_session(connector_id);
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
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
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_eq!(
        first["error"]["data"]["hitl_request_id"], second["error"]["data"]["hitl_request_id"],
        "a retried ask for the same (agent, connector, tool, conversation) must not create a second pending row"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hitl_requests WHERE kind = 'tool_approval' AND agent_id = $1",
    )
    .bind(db.agent_id)
    .fetch_one(&db.state.db)
    .await
    .expect("count rows");
    assert_eq!(count, 1, "exactly one row must exist for this identity");
}

#[tokio::test]
async fn different_tool_on_the_same_connector_creates_a_distinct_row() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "60f7651916cd43dd8448eb211c8031a2";

    let resolved = mcp_session(connector_id);
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": connector_tool_name(connector_id, "list_repos"), "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let second = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &json!({ "name": connector_tool_name(connector_id, "delete_repo"), "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_ne!(
        first["error"]["data"]["hitl_request_id"], second["error"]["data"]["hitl_request_id"],
        "two different tools on the same connector/conversation must not collide on one row"
    );
}

#[tokio::test]
async fn blocked_and_disabled_connector_never_persist_a_row() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "71f7651916cd43dd8448eb211c8031a3";

    let resolved = mcp_session(connector_id);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let blocked = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool.clone(), "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&db.perms(
            &[connector_id],
            vec![rule(connector_id, "*", Stance::Block)],
        )),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        blocked["error"]["code"],
        json!(codes::TOOL_BLOCKED),
        "{blocked}"
    );

    let disabled = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&db.perms(&[], vec![])), // connector never enabled
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        disabled["error"]["code"],
        json!(codes::TOOL_BLOCKED),
        "{disabled}"
    );

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hitl_requests")
        .fetch_one(&db.state.db)
        .await
        .expect("count rows");
    assert_eq!(
        count, 0,
        "Denied decisions (blocked or disabled connector) must never persist a hitl_requests row"
    );
}

// ─── M7: retry-matching (session grants, once-scope claim) ─────────────────

impl TestDb {
    /// Resolve a pending `tool_approval` row exactly the way `POST
    /// /api/hitl/{id}/resolve` (`oss/server/src/router/hitl.rs`) would —
    /// this crate has no HTTP layer of its own, so the resolve step is
    /// driven directly through `nasiko_hitl::repo`, matching the `decision`/
    /// `scope` shape the real resolve handler writes into `human_response`.
    ///
    /// `decision` is serialised through `ResolveDecision::as_str()`, the same call the real
    /// handler makes, rather than echoing this function's own `&str` argument. That difference is
    /// the whole point: with the literal echoed back, this fixture was its own producer, so every
    /// approve test passed no matter what spelling the *gateway* looked for
    /// (`protocol.rs::resolve_tool_approval_retry`, which hardcoded `"approve"` until it was moved
    /// to `DECISION_APPROVE`). Going through `as_str()` makes producer and consumer share one
    /// definition, so a drift between them fails these tests instead of shipping.
    async fn resolve_tool_approval(
        &self,
        hitl_request_id: Uuid,
        decision: &str,
        scope: Option<&str>,
    ) {
        let resolve_decision = match decision {
            "approve" => nasiko_hitl::ResolveDecision::Approve,
            "reject" => nasiko_hitl::ResolveDecision::Reject,
            other => panic!("unexpected decision {other}"),
        };
        let row = nasiko_hitl::repo::resolve(
            &self.state.db,
            hitl_request_id,
            resolve_decision,
            self.owner_user_id,
            json!({ "decision": resolve_decision.as_str(), "scope": scope }),
        )
        .await
        .expect("resolve")
        .expect("row was pending");

        if decision == "approve" && scope == Some("session") {
            nasiko_hitl::repo::create_session_grant(
                &self.state.db,
                nasiko_hitl::NewSessionGrant {
                    agent_id: row.agent_id,
                    connector_id: row.connector_id.expect("tool_approval has connector_id"),
                    tool_name: row.tool_name.clone().expect("tool_approval has tool_name"),
                    context_id: row
                        .context_id
                        .clone()
                        .expect("tool_approval has context_id"),
                    granted_by: self.owner_user_id,
                    hitl_request_id: Some(row.id),
                },
            )
            .await
            .expect("create_session_grant");
        }
    }
}

#[tokio::test]
async fn once_scope_approval_lets_the_retry_reach_the_backend_without_asking_again() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "80f7651916cd43dd8448eb211c8031a4";
    let session_id = "ses_once_retry";
    db.seed_session_trace(session_id, trace_id).await;

    let mut backend = mockito::Server::new_async().await;
    let hit = backend
        .mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
        .expect(1)
        .create_async()
        .await;

    let resolved = ResolvedSession {
        servers: vec![MCPServerConfig {
            connector_id,
            kind: ServerType::Mcp,
            name: "test-connector".into(),
            url: format!("{}/mcp", backend.url()),
            headers: HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
            system: false,
            tool_names: vec![],
            instructions: None,
        }],
        connected_toolkits: vec![],
        toolkit_to_connector: HashMap::new(),
        unusable_connectors: HashMap::new(),
    };
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let tool = connector_tool_name(connector_id, "delete_repo");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(first["error"]["code"], json!(codes::TOOL_ASK), "{first}");
    let hitl_request_id: Uuid = first["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");

    db.resolve_tool_approval(hitl_request_id, "approve", Some("once"))
        .await;

    let retried = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        retried["result"]["ok"],
        json!(true),
        "the retry must reach the backend without a second ASK: {retried}"
    );
    hit.assert_async().await;

    let consumed_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT consumed_at FROM hitl_requests WHERE id = $1")
            .bind(hitl_request_id)
            .fetch_one(&db.state.db)
            .await
            .expect("fetch consumed_at");
    assert!(
        consumed_at.is_some(),
        "the once-scope approval must be marked consumed after being claimed"
    );
}

#[tokio::test]
async fn once_scope_approval_is_denied_on_a_second_retry() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "91f7651916cd43dd8448eb211c8031a5";
    let session_id = "ses_once_duplicate";
    db.seed_session_trace(session_id, trace_id).await;

    let mut backend = mockito::Server::new_async().await;
    backend
        .mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
        .expect(1)
        .create_async()
        .await;

    let resolved = ResolvedSession {
        servers: vec![MCPServerConfig {
            connector_id,
            kind: ServerType::Mcp,
            name: "test-connector".into(),
            url: format!("{}/mcp", backend.url()),
            headers: HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
            system: false,
            tool_names: vec![],
            instructions: None,
        }],
        connected_toolkits: vec![],
        toolkit_to_connector: HashMap::new(),
        unusable_connectors: HashMap::new(),
    };
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let tool = connector_tool_name(connector_id, "delete_repo");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let hitl_request_id: Uuid = first["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");
    db.resolve_tool_approval(hitl_request_id, "approve", Some("once"))
        .await;

    // First retry consumes the once-scope approval and succeeds.
    let retried_once = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(retried_once["result"]["ok"], json!(true), "{retried_once}");

    // A second, duplicate retry must not reuse the already-consumed approval.
    let retried_twice = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(3),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        retried_twice["error"]["code"],
        json!(codes::TOOL_ASK),
        "a duplicate retry after the once-scope approval was consumed must re-ask, not silently \
         re-execute: {retried_twice}"
    );
}

#[tokio::test]
async fn session_scope_approval_lets_a_second_call_succeed_without_reapproval() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = db.seed_connector("session-scope-approval").await;
    let trace_id = "a2f7651916cd43dd8448eb211c8031a6";
    let session_id = "ses_session_scope";
    db.seed_session_trace(session_id, trace_id).await;

    let mut backend = mockito::Server::new_async().await;
    backend
        .mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
        .expect(2)
        .create_async()
        .await;

    let resolved = ResolvedSession {
        servers: vec![MCPServerConfig {
            connector_id,
            kind: ServerType::Mcp,
            name: "test-connector".into(),
            url: format!("{}/mcp", backend.url()),
            headers: HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
            system: false,
            tool_names: vec![],
            instructions: None,
        }],
        connected_toolkits: vec![],
        toolkit_to_connector: HashMap::new(),
        unusable_connectors: HashMap::new(),
    };
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let hitl_request_id: Uuid = first["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");
    db.resolve_tool_approval(hitl_request_id, "approve", Some("session"))
        .await;

    let retry_a = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(retry_a["result"]["ok"], json!(true), "{retry_a}");

    // A second, later call for the SAME tool/conversation must also succeed —
    // the session grant is reusable, unlike once-scope's single-use claim.
    let retry_b = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(3),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        retry_b["result"]["ok"],
        json!(true),
        "a session-scoped grant must allow a second call without a second approval: {retry_b}"
    );
}

#[tokio::test]
async fn rejected_retry_is_denied_without_a_backend_call() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = Uuid::new_v4();
    let trace_id = "b3f7651916cd43dd8448eb211c8031a7";
    let session_id = "ses_reject_retry";
    db.seed_session_trace(session_id, trace_id).await;

    // No mock registered — a backend call here would fail the test via a
    // connection error surfaced as INTERNAL_ERROR, not TOOL_BLOCKED.
    let resolved = mcp_session(connector_id);
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let tool = connector_tool_name(connector_id, "delete_repo");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let hitl_request_id: Uuid = first["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");
    db.resolve_tool_approval(hitl_request_id, "reject", None)
        .await;

    let retried = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &json!({ "name": tool, "arguments": {} }),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        retried["error"]["code"],
        json!(codes::TOOL_BLOCKED),
        "a rejected retry must fail cleanly, never reach the backend: {retried}"
    );
}

// ─── M8: COMPOSIO_MULTI_EXECUTE_TOOL batch retry-matching ──────────────────

/// A resolved session with one Composio backend at `url` and the given
/// `toolkit -> connector_id` mappings — the batch-path analogue of
/// `mcp_session`/`gmail_session` above (this crate has no access to
/// `protocol.rs`'s private unit-test-only `gmail_session`).
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

fn multi_execute_args(slugs: &[&str]) -> serde_json::Value {
    json!({
        "name": "COMPOSIO_MULTI_EXECUTE_TOOL",
        "arguments": {
            "tools": slugs.iter().map(|s| json!({ "tool_slug": s })).collect::<Vec<_>>(),
        },
    })
}

#[tokio::test]
async fn batch_multi_execute_asks_for_every_unresolved_slug_and_persists_one_row_each() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let gmail_cid = Uuid::new_v4();
    let slack_cid = Uuid::new_v4();
    let trace_id = "c4f7651916cd43dd8448eb211c8031a8";
    let session_id = "ses_batch_ask";
    db.seed_session_trace(session_id, trace_id).await;

    // Unreachable URL is fine — an all-ask batch never reaches the backend.
    let resolved = composio_session(
        "http://127.0.0.1:9/mcp",
        HashMap::from([
            ("gmail".to_string(), gmail_cid),
            ("slack".to_string(), slack_cid),
        ]),
    );
    let perms = db.perms(
        &[gmail_cid, slack_cid],
        vec![
            rule(gmail_cid, "*", Stance::Ask),
            rule(slack_cid, "*", Stance::Ask),
        ],
    );
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let res = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &multi_execute_args(&["GMAIL_SEND_EMAIL", "SLACK_SEND_MESSAGE"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_eq!(res["error"]["code"], json!(codes::TOOL_ASK), "{res}");
    let ids = res["error"]["data"]["hitl_request_ids"]
        .as_array()
        .expect("hitl_request_ids array");
    assert_eq!(ids.len(), 2, "one pending row per asked slug: {res}");

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hitl_requests WHERE kind = 'tool_approval' AND agent_id = $1",
    )
    .bind(db.agent_id)
    .fetch_one(&db.state.db)
    .await
    .expect("count rows");
    assert_eq!(count, 2, "both asked slugs must persist their own row");
}

#[tokio::test]
async fn batch_multi_execute_once_scope_approval_lets_that_slug_through_while_the_other_still_asks()
{
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let gmail_cid = Uuid::new_v4();
    let slack_cid = Uuid::new_v4();
    let trace_id = "d5f7651916cd43dd8448eb211c8031a9";
    let session_id = "ses_batch_once";
    db.seed_session_trace(session_id, trace_id).await;

    let mut backend = mockito::Server::new_async().await;
    let hit = backend
        .mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
        .expect(1)
        .create_async()
        .await;

    let resolved = composio_session(
        &format!("{}/mcp", backend.url()),
        HashMap::from([
            ("gmail".to_string(), gmail_cid),
            ("slack".to_string(), slack_cid),
        ]),
    );
    let perms = db.perms(
        &[gmail_cid, slack_cid],
        vec![
            rule(gmail_cid, "*", Stance::Ask),
            rule(slack_cid, "*", Stance::Ask),
        ],
    );
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &multi_execute_args(&["GMAIL_SEND_EMAIL", "SLACK_SEND_MESSAGE"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let ids: Vec<Uuid> = first["error"]["data"]["hitl_request_ids"]
        .as_array()
        .expect("hitl_request_ids array")
        .iter()
        .map(|v| v.as_str().expect("id string").parse().expect("valid uuid"))
        .collect();
    assert_eq!(ids.len(), 2, "{first}");

    // Approve only the gmail slug's row via `once`; leave slack's pending.
    let mut gmail_hitl_id = None;
    let mut slack_hitl_id = None;
    for id in &ids {
        let row = nasiko_hitl::repo::get_by_id(&db.state.db, *id)
            .await
            .expect("get_by_id")
            .expect("row must exist");
        match row.tool_name.as_deref() {
            Some("GMAIL_SEND_EMAIL") => gmail_hitl_id = Some(*id),
            Some("SLACK_SEND_MESSAGE") => slack_hitl_id = Some(*id),
            other => panic!("unexpected tool_name {other:?}"),
        }
    }
    let gmail_hitl_id = gmail_hitl_id.expect("gmail row must exist");
    let slack_hitl_id = slack_hitl_id.expect("slack row must exist");
    db.resolve_tool_approval(gmail_hitl_id, "approve", Some("once"))
        .await;

    let retried = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &multi_execute_args(&["GMAIL_SEND_EMAIL", "SLACK_SEND_MESSAGE"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;

    assert_eq!(
        retried["result"]["ok"],
        json!(true),
        "the approved slug must reach the backend even though the other still needs approval: {retried}"
    );
    hit.assert_async().await;

    let gmail_consumed: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT consumed_at FROM hitl_requests WHERE id = $1")
            .bind(gmail_hitl_id)
            .fetch_one(&db.state.db)
            .await
            .expect("fetch gmail consumed_at");
    assert!(
        gmail_consumed.is_some(),
        "the approved slug's row must be atomically claimed"
    );

    let slack_status: String = sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(slack_hitl_id)
        .fetch_one(&db.state.db)
        .await
        .expect("fetch slack status");
    assert_eq!(
        slack_status, "pending",
        "the still-unresolved slug's row must be left untouched, not consumed or re-created"
    );
}

#[tokio::test]
async fn batch_multi_execute_session_grant_lets_the_same_slug_succeed_repeatedly() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let gmail_cid = db.seed_connector("batch-session-grant-gmail").await;
    let trace_id = "e6f7651916cd43dd8448eb211c8031aa";
    let session_id = "ses_batch_session";
    db.seed_session_trace(session_id, trace_id).await;

    let mut backend = mockito::Server::new_async().await;
    backend
        .mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
        .expect(2)
        .create_async()
        .await;

    let resolved = composio_session(
        &format!("{}/mcp", backend.url()),
        HashMap::from([("gmail".to_string(), gmail_cid)]),
    );
    let perms = db.perms(&[gmail_cid], vec![rule(gmail_cid, "*", Stance::Ask)]);
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &multi_execute_args(&["GMAIL_SEND_EMAIL"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let hitl_request_id: Uuid = first["error"]["data"]["hitl_request_ids"][0]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");
    db.resolve_tool_approval(hitl_request_id, "approve", Some("session"))
        .await;

    let retry_a = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &multi_execute_args(&["GMAIL_SEND_EMAIL"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(retry_a["result"]["ok"], json!(true), "{retry_a}");

    let retry_b = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(3),
        &multi_execute_args(&["GMAIL_SEND_EMAIL"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        retry_b["result"]["ok"],
        json!(true),
        "a session-scoped grant must allow a second batch call without a second approval: {retry_b}"
    );
}

#[tokio::test]
async fn batch_multi_execute_all_rejected_slugs_are_denied_without_a_backend_call() {
    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let gmail_cid = Uuid::new_v4();
    let trace_id = "f7f7651916cd43dd8448eb211c8031ab";
    let session_id = "ses_batch_reject";
    db.seed_session_trace(session_id, trace_id).await;

    // No mock registered — a backend call here would fail the test via a
    // connection error surfaced as INTERNAL_ERROR, not TOOL_BLOCKED.
    let resolved = composio_session(
        "http://127.0.0.1:9/mcp",
        HashMap::from([("gmail".to_string(), gmail_cid)]),
    );
    let perms = db.perms(&[gmail_cid], vec![rule(gmail_cid, "*", Stance::Ask)]);
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

    let first = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &multi_execute_args(&["GMAIL_SEND_EMAIL"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    let hitl_request_id: Uuid = first["error"]["data"]["hitl_request_ids"][0]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");
    db.resolve_tool_approval(hitl_request_id, "reject", None)
        .await;

    let retried = handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(2),
        &multi_execute_args(&["GMAIL_SEND_EMAIL"]),
        &resolved,
        &EffectiveAccess::unrestricted(&perms),
        Some(&traceparent),
        Some(trace_id),
    )
    .await;
    assert_eq!(
        retried["error"]["code"],
        json!(codes::TOOL_BLOCKED),
        "a batch of entirely rejected slugs must fail cleanly, never reach the backend: {retried}"
    );
    assert!(
        retried["error"]["message"]
            .as_str()
            .unwrap()
            .contains("denied"),
        "the message must distinguish a human denial from a stance-blocked tool: {retried}"
    );
}

// #[tokio::test]
// async fn zz_demo_print_batch_retry_full_flow() {
//     let db = TestDb::new("mcp_tool_approval_test", std::sync::Arc::new(AllowAllAuthorizer)).await;
//     let gmail_cid = Uuid::new_v4();
//     let slack_cid = Uuid::new_v4();
//     let trace_id = "aa11651916cd43dd8448eb211c8031ff";
//     let session_id = "ses_demo_batch";
//     db.seed_session_trace(session_id, trace_id).await;

//     let mut backend = mockito::Server::new_async().await;
//     backend
//         .mock("POST", "/mcp")
//         .with_status(200)
//         .with_header("content-type", "application/json")
//         .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true,"tool":"GMAIL_SEND_EMAIL"}}"#)
//         .expect(1)
//         .create_async()
//         .await;

//     let resolved = composio_session(
//         &format!("{}/mcp", backend.url()),
//         HashMap::from([
//             ("gmail".to_string(), gmail_cid),
//             ("slack".to_string(), slack_cid),
//         ]),
//     );
//     let perms = db.perms(
//         &[gmail_cid, slack_cid],
//         vec![
//             rule(gmail_cid, "*", Stance::Ask),
//             rule(slack_cid, "*", Stance::Ask),
//         ],
//     );
//     let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");

//     println!("\n=== STEP 1: agent calls tools/call COMPOSIO_MULTI_EXECUTE_TOOL ===");
//     let req1 = json!({
//         "jsonrpc": "2.0", "id": 1, "method": "tools/call",
//         "params": multi_execute_args(&["GMAIL_SEND_EMAIL", "SLACK_SEND_MESSAGE"]),
//     });
//     println!("--> request:\n{}", serde_json::to_string_pretty(&req1).unwrap());

//     let first = handle_tools_call(
//         &db.state,
//         db.owner_user_id,
//         &json!(1),
//         &multi_execute_args(&["GMAIL_SEND_EMAIL", "SLACK_SEND_MESSAGE"]),
//         &resolved,
//         &perms,
//         Some(&traceparent),
//     )
//     .await;
//     println!(
//         "<-- response (TOOL_ASK, both slugs pending):\n{}",
//         serde_json::to_string_pretty(&first).unwrap()
//     );

//     let ids: Vec<Uuid> = first["error"]["data"]["hitl_request_ids"]
//         .as_array()
//         .unwrap()
//         .iter()
//         .map(|v| v.as_str().unwrap().parse().unwrap())
//         .collect();

//     println!("\n=== STEP 2: real DB state — two pending hitl_requests rows ===");
//     for id in &ids {
//         let row = nasiko_hitl::repo::get_by_id(&db.state.db, *id)
//             .await
//             .unwrap()
//             .unwrap();
//         println!(
//             "  id={} tool_name={:?} status={:?} connector_id={:?}",
//             row.id, row.tool_name, row.status, row.connector_id
//         );
//     }

//     let mut gmail_hitl_id = None;
//     for id in &ids {
//         let row = nasiko_hitl::repo::get_by_id(&db.state.db, *id)
//             .await
//             .unwrap()
//             .unwrap();
//         if row.tool_name.as_deref() == Some("GMAIL_SEND_EMAIL") {
//             gmail_hitl_id = Some(*id);
//         }
//     }
//     let gmail_hitl_id = gmail_hitl_id.unwrap();

//     println!(
//         "\n=== STEP 3: human calls POST /api/hitl/{}/resolve {{\"decision\":\"approve\",\"scope\":\"once\"}} ===",
//         gmail_hitl_id
//     );
//     db.resolve_tool_approval(gmail_hitl_id, "approve", Some("once"))
//         .await;
//     let resolved_row = nasiko_hitl::repo::get_by_id(&db.state.db, gmail_hitl_id)
//         .await
//         .unwrap()
//         .unwrap();
//     println!(
//         "  row now: status={:?} human_response={:?}",
//         resolved_row.status, resolved_row.human_response
//     );

//     println!("\n=== STEP 4: agent retries the same batch tools/call ===");
//     let retried = handle_tools_call(
//         &db.state,
//         db.owner_user_id,
//         &json!(2),
//         &multi_execute_args(&["GMAIL_SEND_EMAIL", "SLACK_SEND_MESSAGE"]),
//         &resolved,
//         &perms,
//         Some(&traceparent),
//     )
//     .await;
//     println!(
//         "<-- response (gmail executed against the real mock backend, slack still pending):\n{}",
//         serde_json::to_string_pretty(&retried).unwrap()
//     );

//     println!("\n=== STEP 5: final DB state ===");
//     let gmail_final = nasiko_hitl::repo::get_by_id(&db.state.db, gmail_hitl_id)
//         .await
//         .unwrap()
//         .unwrap();
//     println!(
//         "  gmail row: status={:?} consumed_at={:?}",
//         gmail_final.status, gmail_final.consumed_at
//     );
//     let slack_id = ids.into_iter().find(|id| *id != gmail_hitl_id).unwrap();
//     let slack_final = nasiko_hitl::repo::get_by_id(&db.state.db, slack_id)
//         .await
//         .unwrap()
//         .unwrap();
//     println!(
//         "  slack row: status={:?} consumed_at={:?}",
//         slack_final.status, slack_final.consumed_at
//     );

//     assert_eq!(retried["result"]["ok"], json!(true));
// }

// ─── Per-user entitlements vs approvals ────────────────────────────────────

/// A restricting `ToolEntitlements` fake, standing in for an edition's impl.
struct FakeEntitlements(nasiko_mcp_gateway::UserEntitlement);
#[async_trait::async_trait]
impl nasiko_mcp_gateway::ToolEntitlements for FakeEntitlements {
    async fn for_user(
        &self,
        _user_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<nasiko_mcp_gateway::UserEntitlement> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn session_grant_cannot_bypass_a_user_entitlement() {
    use nasiko_mcp_gateway::{ToolSet, UserEntitlement};

    let db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    let connector_id = db.seed_connector("entitlement-vs-grant").await;
    let trace_id = "b2f7651916cd43dd8448eb211c8031a7";
    db.seed_session_trace("ses_entitlement_grant", trace_id)
        .await;

    let mut backend = mockito::Server::new_async().await;
    let hit = backend
        .mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
        .expect(1)
        .create_async()
        .await;

    let mut resolved = mcp_session(connector_id);
    resolved.servers[0].url = format!("{}/mcp", backend.url());
    let perms = db.perms(&[connector_id], vec![rule(connector_id, "*", Stance::Ask)]);
    let tool = connector_tool_name(connector_id, "list_repos");
    let traceparent = format!("00-{trace_id}-b7ad6b7169203331-01");
    let call = |id: u64, ent: UserEntitlement| {
        let (db, resolved, perms, tool, traceparent) =
            (&db, &resolved, &perms, &tool, &traceparent);
        async move {
            handle_tools_call(
                &db.state,
                db.owner_user_id,
                &json!(id),
                &json!({ "name": tool, "arguments": {} }),
                resolved,
                &EffectiveAccess::new(perms, &ent),
                Some(traceparent),
                Some(trace_id),
            )
            .await
        }
    };

    // Entitled: the call asks, the human approves for the session.
    let entitled = UserEntitlement::Restricted(HashMap::from([(connector_id, ToolSet::All)]));
    let first = call(1, entitled.clone()).await;
    let hitl_request_id: Uuid = first["error"]["data"]["hitl_request_id"]
        .as_str()
        .expect("hitl_request_id present")
        .parse()
        .expect("valid uuid");
    db.resolve_tool_approval(hitl_request_id, "approve", Some("session"))
        .await;

    // Entitlement revoked: the live session grant must not let the call through.
    let revoked = call(2, UserEntitlement::Restricted(HashMap::new())).await;
    assert_eq!(
        revoked["error"]["code"],
        json!(codes::TOOL_BLOCKED),
        "{revoked}"
    );

    // Entitled again: the grant works and the backend is reached exactly once.
    let again = call(3, entitled).await;
    assert_eq!(again["result"]["ok"], json!(true), "{again}");
    hit.assert_async().await;
}

#[tokio::test]
async fn load_user_entitlement_uses_the_installed_impl() {
    use nasiko_mcp_gateway::entitlements::{
        OssToolEntitlements, invalidate_user_entitlements, load_user_entitlement,
    };
    use nasiko_mcp_gateway::{ToolSet, UserEntitlement};

    let mut db = TestDb::new(
        "mcp_tool_approval_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    // The shared test state points Redis at a dead port; the cache behaviour
    // below needs a live one.
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    db.state.redis =
        nasiko_redis::SharedRedis::new(redis::Client::open(redis_url).expect("redis url"));
    let user = Uuid::new_v4();
    assert_eq!(
        load_user_entitlement(&db.state, user).await.unwrap(),
        UserEntitlement::Unrestricted,
        "OSS default is unrestricted"
    );

    // Unrestricted is cached like any answer: a swapped impl is not consulted
    // for this user until the entry is invalidated.
    let restricted = UserEntitlement::Restricted(HashMap::from([(Uuid::new_v4(), ToolSet::All)]));
    db.state.entitlements = std::sync::Arc::new(FakeEntitlements(restricted.clone()));
    assert_eq!(
        load_user_entitlement(&db.state, user).await.unwrap(),
        UserEntitlement::Unrestricted,
        "Unrestricted is served from the cache"
    );
    invalidate_user_entitlements(&db.state, &[user]).await;
    assert_eq!(
        load_user_entitlement(&db.state, user).await.unwrap(),
        restricted
    );

    // A batch larger than one DEL chunk is cleared completely.
    let users: Vec<Uuid> = (0..1200).map(|_| Uuid::new_v4()).collect();
    for u in &users {
        load_user_entitlement(&db.state, *u).await.unwrap();
    }
    db.state.entitlements = std::sync::Arc::new(OssToolEntitlements);
    invalidate_user_entitlements(&db.state, &users).await;
    for u in [users[0], users[499], users[500], users[1199]] {
        assert_eq!(
            load_user_entitlement(&db.state, u).await.unwrap(),
            UserEntitlement::Unrestricted,
            "invalidated entries are recomputed"
        );
    }
}
