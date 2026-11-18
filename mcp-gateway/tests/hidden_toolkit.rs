//! A Composio toolkit an admin switched off in Settings must not be callable — needs infra
//! up (`just infra`), same convention as `tool_approval.rs`; each test gets its own scratch
//! database.
//!
//! Hiding a toolkit drops it from the user's `toolkit_to_connector`, but their account stays
//! connected on Composio's side and the session resolves accounts by user id. Without an
//! explicit refusal, a slug of a hidden toolkit falls through every per-toolkit permission
//! check and is forwarded.

use std::collections::HashMap;

use nasiko_mcp_gateway::permissions::EffectiveAccess;
use nasiko_mcp_gateway::protocol::handle_tools_call;
use nasiko_mcp_gateway::repo::{self, NewConnector};
use nasiko_mcp_gateway::session::ResolvedSession;
use nasiko_mcp_gateway::types::{MCPServerConfig, ServerType, codes};
use serde_json::{Value, json};
use uuid::Uuid;

mod common;
use common::{AllowAllAuthorizer, TestDb};

/// A resolved session with one Composio backend at a deliberately unreachable URL and no
/// connected toolkit — exactly what a user whose only toolkit was hidden ends up with.
fn composio_session() -> ResolvedSession {
    ResolvedSession {
        servers: vec![MCPServerConfig {
            connector_id: Uuid::nil(),
            kind: ServerType::Composio,
            name: "composio".into(),
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

async fn register_gmail(db: &TestDb, active: bool) {
    let connector = repo::create_connector(
        &db.state.db,
        &NewConnector {
            provider_type: "composio".into(),
            name: "gmail".into(),
            auth_config_id: Some("ac_test".into()),
            use_composio_managed: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("create composio connector");
    repo::set_composio_active(&db.state.db, connector.id, active)
        .await
        .expect("set active");
}

async fn call(db: &TestDb, params: Value) -> Value {
    handle_tools_call(
        &db.state,
        db.owner_user_id,
        &json!(1),
        &params,
        &composio_session(),
        &EffectiveAccess::unrestricted(&db.perms(&[], vec![])),
        None,
        None,
    )
    .await
}

fn multi_execute(slugs: &[&str]) -> Value {
    json!({
        "name": "COMPOSIO_MULTI_EXECUTE_TOOL",
        "arguments": {
            "tools": slugs.iter().map(|s| json!({ "tool_slug": s })).collect::<Vec<_>>(),
        },
    })
}

#[tokio::test]
async fn multi_execute_refuses_a_slug_of_a_hidden_toolkit() {
    let db = TestDb::new(
        "mcp_hidden_toolkit_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    register_gmail(&db, false).await;

    let res = call(&db, multi_execute(&["GMAIL_SEND_EMAIL"])).await;

    assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
}

#[tokio::test]
async fn a_direct_call_to_a_hidden_toolkit_is_refused() {
    let db = TestDb::new(
        "mcp_hidden_toolkit_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    register_gmail(&db, false).await;

    let res = call(&db, json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} })).await;

    assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
}

#[tokio::test]
async fn manage_connections_cannot_connect_a_hidden_toolkit() {
    let db = TestDb::new(
        "mcp_hidden_toolkit_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    register_gmail(&db, false).await;

    let res = call(
        &db,
        json!({
            "name": "COMPOSIO_MANAGE_CONNECTIONS",
            "arguments": { "toolkits": ["gmail"] },
        }),
    )
    .await;

    assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
}

#[tokio::test]
async fn a_visible_toolkit_is_still_forwarded() {
    let db = TestDb::new(
        "mcp_hidden_toolkit_test",
        std::sync::Arc::new(AllowAllAuthorizer),
    )
    .await;
    register_gmail(&db, true).await;

    // Forwarded to the (unreachable) backend: whatever fails, it isn't the hide.
    let res = call(&db, multi_execute(&["GMAIL_SEND_EMAIL"])).await;

    assert_ne!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
}
