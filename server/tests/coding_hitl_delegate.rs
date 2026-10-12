//! The coding-agent HITL permission delegate end to end (NAS-842).
//!
//! A [`HitlPermissionDelegate`] backed by the real `PgHitlStore` records a
//! pending `coding_agent` request and blocks until a human resolves it, then
//! answers the agent: approve → an allow option, reject → `cancelled`. This
//! exercises the create + poll + map path without a container or a model (the
//! full-stack turn is covered by the live proxy tests), so it runs in CI.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::TestServer;
use nasiko_acp_client::sdk::schema::v1::{ToolCallUpdate, ToolCallUpdateFields};
use nasiko_acp_client::{
    PermissionDelegate, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest,
};
use nasiko_hitl::{PgHitlStore, ResolveDecision};
use nasiko_server::coding::permission::HitlPermissionDelegate;
use serial_test::serial;
use uuid::Uuid;

const SESSION_ID: &str = "ses_hitl_delegate";

async fn seed(server: &TestServer) -> (Uuid, Uuid) {
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("hitl-{}", Uuid::new_v4()))
            .bind(format!("hitl-{}@test.local", Uuid::new_v4()))
            .fetch_one(&server.db)
            .await
            .expect("seed user");
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, workload_kind, hitl_enabled) \
         VALUES ($1, $2, 'opencode:0.1.0', 'running', 'acp_coding_agent', true) RETURNING id",
    )
    .bind(format!("hitl-agent-{}", Uuid::new_v4()))
    .bind(user)
    .fetch_one(&server.db)
    .await
    .expect("seed agent");
    (user, agent)
}

fn permission_request() -> RequestPermissionRequest {
    let mut fields = ToolCallUpdateFields::default();
    fields.title = Some("Edit /workspace/hello.txt".to_owned());
    RequestPermissionRequest::new(
        SESSION_ID.to_owned(),
        ToolCallUpdate::new("call_1".to_owned(), fields),
        vec![
            PermissionOption::new("once", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("always", "Always allow", PermissionOptionKind::AllowAlways),
            PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
        ],
    )
}

/// Wait for the delegate to have recorded its pending `coding_agent` row.
async fn wait_for_pending(db: &sqlx::PgPool, agent: Uuid) -> Uuid {
    for _ in 0..100 {
        if let Some(id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM hitl_requests \
             WHERE agent_id = $1 AND origin = 'coding_agent' AND status = 'pending' \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(agent)
        .fetch_optional(db)
        .await
        .expect("query pending")
        {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the delegate never recorded a pending coding_agent request");
}

#[tokio::test]
#[serial]
async fn approve_resolves_to_the_allow_once_option() {
    let server = TestServer::start().await;
    let (user, agent) = seed(&server).await;
    let store: Arc<dyn nasiko_hitl::HitlStore> = Arc::new(PgHitlStore::new(server.db.clone()));
    let delegate = HitlPermissionDelegate::new(store, agent, user, SESSION_ID.to_owned(), None);

    let req = permission_request();
    let decide = tokio::spawn(async move { delegate.decide(&req).await });

    let id = wait_for_pending(&server.db, agent).await;
    nasiko_hitl::repo::resolve(
        &server.db,
        id,
        ResolveDecision::Approve,
        user,
        serde_json::json!({ "decision": "approve" }),
    )
    .await
    .expect("resolve query")
    .expect("row was pending");

    let resp = tokio::time::timeout(Duration::from_secs(10), decide)
        .await
        .expect("delegate answers within 10s")
        .expect("decide task");
    match resp.outcome {
        RequestPermissionOutcome::Selected(selected) => {
            assert_eq!(
                selected.option_id.to_string(),
                "once",
                "approve → allow-once"
            );
        }
        other => panic!("expected an allow option, got {other:?}"),
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn reject_resolves_to_cancelled() {
    let server = TestServer::start().await;
    let (user, agent) = seed(&server).await;
    let store: Arc<dyn nasiko_hitl::HitlStore> = Arc::new(PgHitlStore::new(server.db.clone()));
    let delegate = HitlPermissionDelegate::new(store, agent, user, SESSION_ID.to_owned(), None);

    let req = permission_request();
    let decide = tokio::spawn(async move { delegate.decide(&req).await });

    let id = wait_for_pending(&server.db, agent).await;
    nasiko_hitl::repo::resolve(
        &server.db,
        id,
        ResolveDecision::Reject,
        user,
        serde_json::json!({ "decision": "reject" }),
    )
    .await
    .expect("resolve query")
    .expect("row was pending");

    let resp = tokio::time::timeout(Duration::from_secs(10), decide)
        .await
        .expect("delegate answers within 10s")
        .expect("decide task");
    assert!(
        matches!(resp.outcome, RequestPermissionOutcome::Cancelled),
        "reject → cancelled, got {:?}",
        resp.outcome
    );

    server.cleanup().await;
}
