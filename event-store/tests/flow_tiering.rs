#![recursion_limit = "256"]
//! Integration tests for flows and flow_participants tiering into Lance,
//! querying across hot/cold tiers, and crash recovery.

use chrono::{Duration, Utc};
use nasiko_event_store::{EventStore, TieringConfig, cold};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into())
}

async fn fresh_db() -> PgPool {
    let admin = admin_url();
    let name = format!("flow_tiering_{}", Uuid::new_v4().simple());
    let mut c = PgConnection::connect(&admin).await.expect("postgres up?");
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&mut c)
        .await
        .unwrap();
    let base = admin.rsplit_once('/').unwrap().0;
    let pool = PgPool::connect(&format!("{base}/{name}")).await.unwrap();
    sqlx::migrate!("../migrations").run(&pool).await.unwrap();
    pool
}

async fn create_user(db: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(id.to_string())
        .bind(format!("{id}@example.com"))
        .execute(db)
        .await
        .unwrap();
    id
}

fn make_store(db: &PgPool, dir: &tempfile::TempDir) -> EventStore {
    let cfg = TieringConfig {
        cold_path: dir.path().to_str().unwrap().into(),
        batch_rows: 2, // force multiple batches
        ..Default::default()
    };
    EventStore::new(db.clone(), cfg)
}

#[tokio::test]
async fn test_tier_flows_and_participants_lifecycle() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(&db, &dir);
    let user_id = create_user(&db).await;
    let session_id = "sess-lifecycle-1";

    let root_agent_id = Uuid::new_v4();
    sqlx::query("INSERT INTO agents (id, owner_id, name) VALUES ($1, $2, 'root-agent')")
        .bind(root_agent_id)
        .bind(user_id)
        .execute(&db)
        .await
        .unwrap();

    let now = Utc::now();
    let old_completed = now - Duration::days(100);
    let recent_completed = now - Duration::days(10);

    // 1. Insert 3 old completed flows (should be tiered)
    for i in 1..=3 {
        let flow_id = format!("flow-old-{i}");
        let worker_agent_id = Uuid::new_v4();
        sqlx::query("INSERT INTO agents (id, owner_id, name) VALUES ($1, $2, $3)")
            .bind(worker_agent_id)
            .bind(user_id)
            .bind(format!("agent-worker-{i}"))
            .execute(&db)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO flows (flow_id, session_id, user_id, root_agent_id, title, status, created_at, completed_at)
             VALUES ($1, $2, $3, $4, $5, 'completed', $6, $6)"
        )
        .bind(&flow_id)
        .bind(session_id)
        .bind(user_id)
        .bind(root_agent_id)
        .bind(format!("Old Flow {i}"))
        .bind(old_completed)
        .execute(&db)
        .await
        .unwrap();

        // Add a participant
        sqlx::query(
            "INSERT INTO flow_participants (flow_id, agent_id, created_at) VALUES ($1, $2, $3)",
        )
        .bind(&flow_id)
        .bind(worker_agent_id)
        .bind(old_completed)
        .execute(&db)
        .await
        .unwrap();
    }

    // 2. Insert 1 recent completed flow (should stay hot)
    let recent_flow_id = "flow-recent-1";
    sqlx::query(
        "INSERT INTO flows (flow_id, session_id, user_id, root_agent_id, title, status, created_at, completed_at)
         VALUES ($1, $2, $3, $4, $5, 'completed', $6, $6)"
    )
    .bind(recent_flow_id)
    .bind(session_id)
    .bind(user_id)
    .bind(root_agent_id)
    .bind("Recent Flow")
    .bind(recent_completed)
    .execute(&db)
    .await
    .unwrap();

    // 3. Insert 1 old running flow (should stay hot because status != completed/failed)
    let running_flow_id = "flow-old-running";
    sqlx::query(
        "INSERT INTO flows (flow_id, session_id, user_id, root_agent_id, title, status, created_at)
         VALUES ($1, $2, $3, $4, $5, 'running', $6)",
    )
    .bind(running_flow_id)
    .bind(session_id)
    .bind(user_id)
    .bind(root_agent_id)
    .bind("Running Flow")
    .bind(old_completed)
    .execute(&db)
    .await
    .unwrap();

    // Run tiering pass
    let report = store.run_once(now).await.unwrap().expect("tier report");
    assert!(
        report.rows >= 3,
        "expected at least 3 rows tiered, got {}",
        report.rows
    );

    // Hot rows are slimmed, never deleted: all 5 stay, 3 are marked tiered.
    let total_hot: i64 = sqlx::query_scalar("SELECT count(*) FROM flows")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(total_hot, 5, "tiering must not delete flows rows");
    let untiered: i64 = sqlx::query_scalar("SELECT count(*) FROM flows WHERE tiered_at IS NULL")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(untiered, 2, "recent and running flows stay untiered");
    let slim_meta: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flows WHERE tiered_at IS NOT NULL AND metadata <> '{}'::jsonb",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(slim_meta, 0, "tiered rows have their heavy columns cleared");

    // Check cold Lance dataset
    let filter = format!("session_id = {}", cold::lit(session_id));
    let cold_flows = store.cold_flows(&filter).await.unwrap();
    assert_eq!(cold_flows.len(), 3, "3 flows must be cold in Lance");

    // Participants stay hot (no cascade, MCP agent auth keeps working).
    let hot_parts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_participants WHERE flow_id LIKE 'flow-old-%'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(
        hot_parts, 3,
        "participants of tiered flows stay in Postgres"
    );

    // A second pass must not re-tier slimmed rows.
    let again = store.run_once(now).await.unwrap().expect("tier report");
    let cold_again = store.cold_flows(&filter).await.unwrap();
    assert_eq!(
        cold_again.len(),
        3,
        "no duplicate cold rows, got report {:?}",
        again.rows
    );

    // Verify hot + cold unified load for the session
    let all_session_flows = store.load_flows_for_session(session_id).await.unwrap();
    assert_eq!(
        all_session_flows.len(),
        5,
        "load_flows_for_session must merge hot and cold (total 5)"
    );

    // Verify single flow load from cold
    let single_cold = store.load_flow("flow-old-1").await.unwrap();
    assert!(single_cold.is_some());
    assert_eq!(single_cold.unwrap().title.as_deref(), Some("Old Flow 1"));

    // Verify single flow load from hot
    let single_hot = store.load_flow(recent_flow_id).await.unwrap();
    assert!(single_hot.is_some());
    assert_eq!(single_hot.unwrap().title.as_deref(), Some("Recent Flow"));
}
