//! The tool-list savings layers reach `token_savings` — needs infra up (`just infra`), same
//! convention as `tool_approval.rs`; each test gets its own scratch database, so migrations 0099 and
//! 0100 run on every test.
//!
//! `nasiko_savings::insert` swallows a failed insert by design, so a layer missing from the
//! `token_savings_layer_check` constraint would lose its rows silently. These tests read the rows
//! back instead of trusting the write.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nasiko_mcp_gateway::ConnectorAuthorizer;
use nasiko_mcp_gateway::permissions::EffectiveAccess;
use nasiko_mcp_gateway::protocol::handle_tools_list;
use nasiko_mcp_gateway::repo::{self, McpConnector};
use nasiko_mcp_gateway::search::{Bm25SearchIndex, ToolSearchIndex};
use nasiko_mcp_gateway::session::ResolvedSession;
use nasiko_mcp_gateway::types::{AccessReason, MCPServerConfig, OrgGrantConsumer, ServerType};
use nasiko_savings::{Layer, Method, SavingsRow};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

mod common;
use common::TestDb;

/// Grants the user exactly one connector, so search sees the seeded tools and nothing else.
struct OneConnector(Uuid);

#[async_trait]
impl ConnectorAuthorizer for OneConnector {
    async fn can_access_connector(
        &self,
        _db: &PgPool,
        _user_id: Uuid,
        connector_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<bool> {
        Ok(connector_id == self.0)
    }
    async fn list_accessible_connectors(
        &self,
        db: &PgPool,
        _user_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<Vec<McpConnector>> {
        Ok(repo::get_connector_by_id(db, self.0)
            .await?
            .into_iter()
            .collect())
    }
    async fn list_accessible_mcp_connectors(
        &self,
        db: &PgPool,
        user_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<Vec<McpConnector>> {
        self.list_accessible_connectors(db, user_id).await
    }
    async fn list_access_reasons(
        &self,
        _db: &PgPool,
        _connector: &McpConnector,
    ) -> nasiko_mcp_gateway::Result<Vec<AccessReason>> {
        Ok(vec![])
    }
    async fn list_org_grant_consumers(
        &self,
        _db: &PgPool,
        _connector_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<(Vec<OrgGrantConsumer>, Vec<OrgGrantConsumer>)> {
        Ok((vec![], vec![]))
    }
}

/// Six note-taking tools, each with a schema large enough that listing one instead of all six
/// outweighs the three meta-tools search mode adds.
async fn seed_tools(db: &PgPool, connector_id: Uuid) {
    for name in [
        "create_note",
        "list_notes",
        "delete_note",
        "archive_note",
        "share_folder",
        "list_folders",
    ] {
        let properties: serde_json::Map<String, serde_json::Value> = (0..8)
            .map(|i| {
                (
                    format!("field_{i}"),
                    json!({"type": "string", "description": format!("Field {i} of the {name} request, as the notes API names it.")}),
                )
            })
            .collect();
        sqlx::query(
            "INSERT INTO mcp_connector_tools (connector_id, tool_name, description, input_schema) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(connector_id)
        .bind(name)
        .bind(format!("{} in the notes app", name.replace('_', " ")))
        .bind(json!({"type": "object", "properties": properties}))
        .execute(db)
        .await
        .expect("seed tool");
    }
}

fn session(connector_id: Uuid) -> ResolvedSession {
    ResolvedSession {
        servers: vec![MCPServerConfig {
            connector_id,
            kind: ServerType::Mcp,
            name: "notes".into(),
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

/// `(program, bytes_before, bytes_after, saved_input_tokens, model, flow_id, token_estimated)`.
type SavedRow = (String, i64, i64, i64, Option<String>, String, bool);

/// The spawned insert lands after `tools/list` has answered.
async fn savings_rows(db: &PgPool, layer: &str) -> Vec<SavedRow> {
    for _ in 0..100 {
        let rows: Vec<SavedRow> = sqlx::query_as(
            "SELECT program, bytes_before, bytes_after, saved_input_tokens, model, flow_id, \
                    token_estimated \
               FROM token_savings WHERE layer = $1",
        )
        .bind(layer)
        .fetch_all(db)
        .await
        .expect("read token_savings");
        if !rows.is_empty() {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Vec::new()
}

#[tokio::test]
async fn a_search_listing_writes_what_it_left_out() {
    let mut db = TestDb::new("tool_search_savings", Arc::new(OneConnector(Uuid::nil()))).await;
    let connector_id = db.seed_connector("tool_search_savings").await;
    db.state.authorizer = Arc::new(OneConnector(connector_id));
    seed_tools(&db.state.db, connector_id).await;
    let index = Bm25SearchIndex::new();
    index.rebuild(&db.state.db).await.expect("build index");
    db.state.search_index = Arc::new(index);
    db.state.config.tool_search_tool_limit = 1;

    let flow_id = format!("{:032x}", Uuid::new_v4().as_u128());
    db.seed_session_trace("ctx-tool-search", &flow_id).await;
    sqlx::query("UPDATE flows SET title = 'create a note about the meeting' WHERE flow_id = $1")
        .bind(&flow_id)
        .execute(&db.state.db)
        .await
        .expect("set flow title");

    let res = handle_tools_list(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &session(connector_id),
        &EffectiveAccess::unrestricted(&db.perms(&[connector_id], vec![])),
        None,
        Some(&flow_id),
    )
    .await;
    let listed = res["result"]["tools"].as_array().expect("a tool list");
    assert_eq!(
        listed.len(),
        4,
        "one match plus the three meta-tools: {res}"
    );

    let rows = savings_rows(&db.state.db, "tool_search").await;
    let [(program, before, after, saved, model, row_flow, estimated)] = rows.as_slice() else {
        panic!("expected one tool_search row, got {rows:?}");
    };
    assert_eq!(program, "tool_lists");
    assert!(
        before > after,
        "six tools ({before} B) against one plus meta-tools ({after} B)"
    );
    assert_eq!(*saved, (before - after) / 4);
    assert_eq!(
        model, &None,
        "priced at read time from the flow's first call"
    );
    assert_eq!(row_flow, &flow_id);
    assert!(estimated);
}

#[tokio::test]
async fn a_listing_with_no_flow_query_writes_nothing() {
    let mut db = TestDb::new("tool_search_savings", Arc::new(OneConnector(Uuid::nil()))).await;
    let connector_id = db.seed_connector("tool_search_savings").await;
    db.state.authorizer = Arc::new(OneConnector(connector_id));
    seed_tools(&db.state.db, connector_id).await;
    let index = Bm25SearchIndex::new();
    index.rebuild(&db.state.db).await.expect("build index");
    db.state.search_index = Arc::new(index);

    // Agent startup: no verified flow, so only the meta-tools are listed and nothing is measured.
    handle_tools_list(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &session(connector_id),
        &EffectiveAccess::unrestricted(&db.perms(&[connector_id], vec![])),
        None,
        None,
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM token_savings")
        .fetch_one(&db.state.db)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

/// `context_budget` has no writer in OSS yet, so nothing else inserts one; checked here against the
/// same migrated schema, so a constraint missing it fails a test rather than dropping rows.
/// (`compact_tools` is covered through the real `log_usage` in the server's `token_savings_write`.)
#[tokio::test]
async fn the_context_budget_layer_passes_the_layer_check() {
    let db = TestDb::new("tool_search_savings", Arc::new(OneConnector(Uuid::nil()))).await;
    nasiko_savings::insert(
        &db.state.db,
        &SavingsRow {
            user_id: db.owner_user_id,
            agent_id: Some(db.agent_id),
            flow_id: Some("trace-context-budget".into()),
            session_id: None,
            provider: None,
            model: None,
            layer: Layer::ContextBudget,
            bytes_before: Some(4_800),
            bytes_after: Some(1_200),
            saved_input_tokens: 900,
            saved_output_tokens: 0,
            saved_cost_usd: 0.0,
            method: Method::MeasuredBytes,
            token_estimated: true,
            context_tier: None,
        },
    )
    .await;

    let rows = savings_rows(&db.state.db, "context_budget").await;
    assert_eq!(rows.len(), 1, "the insert was refused");
    assert_eq!(rows[0].0, "context_budget");
}
